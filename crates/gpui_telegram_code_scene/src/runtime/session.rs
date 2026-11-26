//! `VmInstance` lifecycle execution for Video, Animation, and Playable Game modalities.

use std::sync::Arc;
use std::time::Instant;

use gpui_telegram_proto::{AssetKind, AssetManifest};

use crate::lang::{CompiledProgram, ExecQuota, Val, VmBudgets, VmHost, VmState};
use crate::model::{
    AudioPulse, GameHudSnapshot, GameSessionTelemetry, RuntimeError, SceneInputState, sanitize_f64,
};
use crate::runtime::frame::{DrawCmd, DrawListBuilder, FrameOutput, sanitize_text};
use crate::runtime::pixel::{DEFAULT_PIXEL_HEIGHT, DEFAULT_PIXEL_WIDTH, PixelBuffer};
use crate::runtime::timeline::{
    MAX_SEEK_REPLAY_STEPS, SEEK_FIXED_DT, SEEK_STEPS_PER_SEC, TimelineCheckpoint,
    TimelineController,
};
use crate::runtime::validator::VerifiedProgram;

/// Per-frame host collecting sanitized `DrawCmd`s, `AudioPulse`s, and BGRA8 `PixelBuffer` mutations.
struct FrameHost<'a> {
    width: u32,
    height: u32,
    fps: u32,
    duration_sec: Option<f64>,
    budgets: VmBudgets,
    draws: DrawListBuilder,
    pixel: &'a mut Option<PixelBuffer>,
    pixel_revision: &'a mut u64,
}

impl<'a> FrameHost<'a> {
    fn new(
        manifest: &AssetManifest,
        budgets: VmBudgets,
        pixel: &'a mut Option<PixelBuffer>,
        pixel_revision: &'a mut u64,
    ) -> Self {
        Self {
            width: manifest.width,
            height: manifest.height,
            fps: manifest.fps,
            duration_sec: manifest.duration_sec,
            budgets,
            draws: DrawListBuilder::new(budgets.max_draw_cmds, budgets.max_audio_pulses),
            pixel,
            pixel_revision,
        }
    }
}

impl VmHost for FrameHost<'_> {
    fn canvas_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn canvas_fps(&self) -> u32 {
        self.fps
    }

    fn canvas_duration(&self) -> Option<f64> {
        self.duration_sec
    }

    fn push_draw_cmd(&mut self, cmd: DrawCmd) {
        self.draws.push(cmd);
    }

    fn record_invalid_draw(&mut self) {
        self.draws.record_invalid_draw();
    }

    fn push_audio_pulse(&mut self, pulse: AudioPulse) {
        self.draws.push_audio(pulse);
    }

    fn pixel_init(&mut self, w: f64, h: f64) -> Result<(), RuntimeError> {
        let prev = (*self.pixel_revision)
            .max(self.pixel.as_ref().map_or(0, PixelBuffer::revision));
        let buf = PixelBuffer::new_checked(
            w,
            h,
            self.budgets.max_pixel_width,
            self.budgets.max_pixel_height,
            self.budgets.max_pixel_area,
            prev,
        )?;
        *self.pixel_revision = buf.revision();
        *self.pixel = Some(buf);
        Ok(())
    }

    fn pixel_ensure_default(&mut self) -> Result<&mut PixelBuffer, RuntimeError> {
        if self.pixel.is_none() {
            self.pixel_init(DEFAULT_PIXEL_WIDTH as f64, DEFAULT_PIXEL_HEIGHT as f64)?;
        }
        Ok(self.pixel.as_mut().expect("pixel initialized"))
    }

    fn pixel_buffer_mut(&mut self) -> Option<&mut PixelBuffer> {
        self.pixel.as_mut()
    }
}

/// Live sandboxed instance of a verified `CodeSceneScript` asset.
#[derive(Debug, Clone)]
pub struct VmInstance {
    program: Arc<CompiledProgram>,
    manifest: AssetManifest,
    budgets: VmBudgets,
    vm: VmState,
    pixel: Option<PixelBuffer>,
    pixel_revision: u64,
    timeline: TimelineController,
    telemetry: GameSessionTelemetry,
    prev_input: SceneInputState,
    prev_game_over: bool,
    hold_input_accum_ms: f64,
    last_draw_cap_hit: bool,
    last_dropped_invalid_draws: usize,
}

impl VmInstance {
    /// Creates a new running instance from a verified program and executes `on_init(state)`.
    pub fn new(program: VerifiedProgram, budgets: VmBudgets) -> Result<Self, RuntimeError> {
        Self::from_compiled(program.compiled, program.manifest, budgets)
    }

    /// Internal constructor used by `runtime::validator` prior to `VerifiedProgram` construction.
    pub(crate) fn from_compiled(
        compiled: Arc<CompiledProgram>,
        manifest: AssetManifest,
        budgets: VmBudgets,
    ) -> Result<Self, RuntimeError> {
        let manifest = manifest.clamped();
        let mut vm = VmState::new(Arc::clone(&compiled), budgets, manifest.seed)?;
        let mut pixel = None;
        let mut pixel_revision = 0u64;
        Self::run_init_sequence(&mut vm, &manifest, budgets, &mut pixel, &mut pixel_revision)?;
        pixel_revision = pixel_revision.max(pixel.as_ref().map_or(0, PixelBuffer::revision));
        let cp0 = TimelineCheckpoint {
            sec: 0.0,
            step_idx: 0,
            vm: vm.snapshot(),
            pixel: pixel.clone(),
            pixel_revision,
        };
        let duration_sec = if manifest.kind == AssetKind::Video {
            manifest.duration_sec
        } else {
            None
        };
        let timeline = TimelineController::new(duration_sec, cp0);
        Ok(Self {
            program: compiled,
            manifest,
            budgets,
            vm,
            pixel,
            pixel_revision,
            timeline,
            telemetry: GameSessionTelemetry::default(),
            prev_input: SceneInputState::default(),
            prev_game_over: false,
            hold_input_accum_ms: 0.0,
            last_draw_cap_hit: false,
            last_dropped_invalid_draws: 0,
        })
    }

    fn run_init_sequence(
        vm: &mut VmState,
        manifest: &AssetManifest,
        budgets: VmBudgets,
        pixel: &mut Option<PixelBuffer>,
        pixel_revision: &mut u64,
    ) -> Result<u64, RuntimeError> {
        let started = Instant::now();
        let quota = ExecQuota::for_frame(&budgets);
        let mut ops_used = 0u64;
        let mut host = FrameHost::new(manifest, budgets, pixel, pixel_revision);
        vm.run_global_inits(&mut host, &mut ops_used, started, quota)?;
        if vm.program().has_function("on_init") {
            let state_val = vm.state_val();
            vm.call_by_name("on_init", &[state_val], &mut host, &mut ops_used, started, quota)?;
        }
        Ok(ops_used)
    }

    pub fn manifest(&self) -> &AssetManifest {
        &self.manifest
    }

    pub fn set_playback_speed(&mut self, speed: f64) {
        self.timeline.set_playback_speed(speed);
    }

    pub fn playback_speed(&self) -> f64 {
        self.timeline.playback_speed()
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.timeline.set_paused(paused);
    }

    pub fn is_paused(&self) -> bool {
        self.timeline.is_paused()
    }

    pub fn game_telemetry(&self) -> GameSessionTelemetry {
        self.telemetry.clone()
    }

    pub(crate) fn last_draw_cap_hit(&self) -> bool {
        self.last_draw_cap_hit
    }

    pub fn last_dropped_invalid_draws(&self) -> usize {
        self.last_dropped_invalid_draws
    }

    pub(crate) fn used_builtins(&self) -> Vec<String> {
        self.vm.used_builtins()
    }
}

impl VmInstance {
    /// Returns a self-contained pixel snapshot if a pixel buffer has been initialized.
    pub fn pixel_snapshot(&self) -> Option<crate::runtime::pixel::PixelFrame> {
        self.pixel.as_ref().map(PixelBuffer::snapshot)
    }

    /// Executes one frame (`on_update` when unpaused + `on_render`) under a shared frame budget.
    pub fn step_frame(
        &mut self,
        dt_sec: f64,
        input: &SceneInputState,
    ) -> Result<FrameOutput, RuntimeError> {
        let started = Instant::now();
        let quota = ExecQuota::for_frame(&self.budgets);
        let mut ops_used = 0u64;
        let sanitized_input = input.sanitized();

        let advance = self.timeline.advance(dt_sec);
        let has_update = self.program.has_function("on_update");

        // On Video loop wrap for stateful scripts, restore Checkpoint 0 to avoid drift
        // while rebasing PixelBuffer.revision strictly above the prior frame's revision.
        if advance.wrapped_loop && has_update {
            let prev_rev = self
                .pixel_revision
                .max(self.pixel.as_ref().map_or(0, PixelBuffer::revision));
            let cp0 = self.timeline.checkpoint_zero().clone();
            self.vm.restore(&cp0.vm);
            self.pixel = cp0.pixel;
            if let Some(buf) = self.pixel.as_mut() {
                buf.rebase_revision_above(prev_rev);
                self.pixel_revision = buf.revision();
            } else {
                self.pixel_revision = prev_rev;
            }
        }

        let run_update = !self.timeline.is_paused() && has_update;
        if run_update {
            self.vm.update_input_state(&sanitized_input);
        }

        let (draw_cmds, audio_pulses, draw_cap_hit, dropped_invalid) = {
            let mut host = FrameHost::new(
                &self.manifest,
                self.budgets,
                &mut self.pixel,
                &mut self.pixel_revision,
            );
            if run_update {
                let state_val = self.vm.state_val();
                let dt_val = Val::Num(advance.eff_dt);
                let in_val = self.vm.input_val();
                self.vm.call_by_name(
                    "on_update",
                    &[state_val, dt_val, in_val],
                    &mut host,
                    &mut ops_used,
                    started,
                    quota,
                )?;
            }
            let state_val = self.vm.state_val();
            let t_val = Val::Num(self.timeline.timeline_sec());
            self.vm.call_by_name(
                "on_render",
                &[state_val, t_val, Val::Canvas],
                &mut host,
                &mut ops_used,
                started,
                quota,
            )?;
            host.draws.finish()
        };

        self.last_draw_cap_hit = draw_cap_hit;
        self.last_dropped_invalid_draws = dropped_invalid;

        let game_hud = if self.manifest.kind == AssetKind::Game {
            let hud = self.extract_and_sync_game_hud(advance.eff_dt, &sanitized_input)?;
            Some(hud)
        } else {
            None
        };
        self.prev_input = sanitized_input;

        if let Some(buf) = self.pixel.as_ref() {
            self.pixel_revision = self.pixel_revision.max(buf.revision());
        }
        let pixel_bgra = self.pixel.as_mut().and_then(PixelBuffer::take_if_dirty);

        // Keep inter-frame heap occupancy bounded outside the per-frame watchdog
        // so benign allocating scenes avoid sudden stop-the-world mid-frame GC pauses.
        let idle_gc_threshold = (self.budgets.max_heap_values / 4).max(1_024);
        if self.vm.heap().live_values() > idle_gc_threshold {
            self.vm.collect_garbage_idle();
        }

        Ok(FrameOutput {
            draw_cmds,
            pixel_bgra,
            timeline_sec: self.timeline.timeline_sec(),
            duration_sec: self.timeline.duration_sec(),
            loop_count: self.timeline.loop_count(),
            game_hud,
            audio_pulses,
            ops_used,
        })
    }

    /// Deterministic video seek: O(1) for pure-timeline videos, or isolated fixed-dt `1/30s`
    /// checkpoint + bounded replay for stateful videos.
    pub fn seek_video(&mut self, target_sec: f64) -> Result<FrameOutput, RuntimeError> {
        let target = self.timeline.clamp_seek_target(target_sec);
        let started = Instant::now();
        let seek_quota = ExecQuota::for_seek(&self.budgets);
        let mut ops_used = 0u64;

        if !self.program.has_function("on_update") {
            let cp0 = self.timeline.checkpoint_zero().clone();
            self.vm.restore(&cp0.vm);
            self.pixel = cp0.pixel;
            self.pixel_revision = cp0.pixel_revision;
            self.timeline.set_timeline_sec(target);
        } else {
            let full_steps = ((target * (SEEK_STEPS_PER_SEC as f64)) + 1e-9)
                .floor()
                .max(0.0) as u64;
            let max_sec_key = (full_steps / SEEK_STEPS_PER_SEC).min(u32::MAX as u64) as u32;
            let cp = self
                .timeline
                .nearest_seek_checkpoint(max_sec_key, target)
                .clone();
            self.vm.restore(&cp.vm);
            self.pixel = cp.pixel;
            self.pixel_revision = cp.pixel_revision;
            let mut step_idx = cp.step_idx;
            let mut steps = 0usize;
            let default_input = SceneInputState::default();

            while step_idx < full_steps {
                steps += 1;
                if steps > MAX_SEEK_REPLAY_STEPS {
                    return Err(RuntimeError::SeekBudgetExceeded {
                        detail: format!("seek replay exceeded {MAX_SEEK_REPLAY_STEPS} fixed steps"),
                    });
                }
                self.vm.update_input_state(&default_input);
                let in_val = self.vm.input_val();
                let state_val = self.vm.state_val();
                let mut host = FrameHost::new(
                    &self.manifest,
                    self.budgets,
                    &mut self.pixel,
                    &mut self.pixel_revision,
                );
                self.vm.call_by_name(
                    "on_update",
                    &[state_val, Val::Num(SEEK_FIXED_DT), in_val],
                    &mut host,
                    &mut ops_used,
                    started,
                    seek_quota,
                )?;
                step_idx += 1;

                // Cache checkpoint strictly on exact integer-second grid points
                if step_idx % SEEK_STEPS_PER_SEC == 0 {
                    let sec_key = (step_idx / SEEK_STEPS_PER_SEC) as u32;
                    if self.vm.heap().live_values() > 2_048 {
                        self.vm.collect_garbage_idle();
                    }
                    let cp_pixel_rev = self
                        .pixel_revision
                        .max(self.pixel.as_ref().map_or(0, PixelBuffer::revision));
                    self.timeline.store_seek_checkpoint(
                        sec_key,
                        TimelineCheckpoint {
                            sec: sec_key as f64,
                            step_idx,
                            vm: self.vm.snapshot(),
                            pixel: self.pixel.clone(),
                            pixel_revision: cp_pixel_rev,
                        },
                    );
                }
            }

            let rem = target - (full_steps as f64) / (SEEK_STEPS_PER_SEC as f64);
            if rem > 1e-9 {
                steps += 1;
                if steps > MAX_SEEK_REPLAY_STEPS {
                    return Err(RuntimeError::SeekBudgetExceeded {
                        detail: format!("seek replay exceeded {MAX_SEEK_REPLAY_STEPS} fixed steps"),
                    });
                }
                self.vm.update_input_state(&default_input);
                let in_val = self.vm.input_val();
                let state_val = self.vm.state_val();
                let mut host = FrameHost::new(
                    &self.manifest,
                    self.budgets,
                    &mut self.pixel,
                    &mut self.pixel_revision,
                );
                self.vm.call_by_name(
                    "on_update",
                    &[state_val, Val::Num(rem), in_val],
                    &mut host,
                    &mut ops_used,
                    started,
                    seek_quota,
                )?;
            }
            self.timeline.set_timeline_sec(target);
        }

        let (draw_cmds, audio_pulses, draw_cap_hit, dropped_invalid) = {
            let mut host = FrameHost::new(
                &self.manifest,
                self.budgets,
                &mut self.pixel,
                &mut self.pixel_revision,
            );
            let state_val = self.vm.state_val();
            self.vm.call_by_name(
                "on_render",
                &[state_val, Val::Num(target), Val::Canvas],
                &mut host,
                &mut ops_used,
                started,
                seek_quota,
            )?;
            host.draws.finish()
        };
        self.last_draw_cap_hit = draw_cap_hit;
        self.last_dropped_invalid_draws = dropped_invalid;

        let game_hud = if self.manifest.kind == AssetKind::Game {
            Some(self.read_game_hud_snapshot())
        } else {
            None
        };

        Ok(FrameOutput {
            draw_cmds,
            pixel_bgra: self.pixel.as_ref().map(PixelBuffer::snapshot),
            timeline_sec: target,
            duration_sec: self.timeline.duration_sec(),
            loop_count: self.timeline.loop_count(),
            game_hud,
            audio_pulses,
            ops_used,
        })
    }

    /// Restarts the scene from `t = 0.0`, preserving `best_score` and incrementing `restarts`.
    pub fn restart(&mut self) -> Result<(), RuntimeError> {
        self.telemetry.restarts = self.telemetry.restarts.saturating_add(1);
        let preserved_best = self.telemetry.best_score;
        self.telemetry.current_score = 0;
        self.prev_game_over = false;
        self.prev_input = SceneInputState::default();
        self.hold_input_accum_ms = 0.0;

        self.vm = VmState::new(Arc::clone(&self.program), self.budgets, self.manifest.seed)?;
        self.pixel_revision = self
            .pixel_revision
            .max(self.pixel.as_ref().map_or(0, PixelBuffer::revision));
        self.pixel = None;
        Self::run_init_sequence(
            &mut self.vm,
            &self.manifest,
            self.budgets,
            &mut self.pixel,
            &mut self.pixel_revision,
        )?;
        self.pixel_revision = self
            .pixel_revision
            .max(self.pixel.as_ref().map_or(0, PixelBuffer::revision));
        if self.manifest.kind == AssetKind::Game && preserved_best > 0 {
            let cur_hi = sanitize_f64(self.vm.get_state_field("high_score").as_num()).round() as i64;
            if preserved_best > cur_hi {
                self.vm.set_state_field("high_score", Val::Num(preserved_best as f64))?;
            }
        }
        let cp0 = TimelineCheckpoint {
            sec: 0.0,
            step_idx: 0,
            vm: self.vm.snapshot(),
            pixel: self.pixel.clone(),
            pixel_revision: self.pixel_revision,
        };
        self.timeline.reset_with_checkpoint_zero(cp0);
        Ok(())
    }

    fn read_game_hud_snapshot(&self) -> GameHudSnapshot {
        let score = sanitize_f64(self.vm.get_state_field("score").as_num()).round() as i64;
        let state_hi = sanitize_f64(self.vm.get_state_field("high_score").as_num()).round() as i64;
        let high_score = state_hi.max(self.telemetry.best_score).max(score);
        let game_over = self.vm.get_state_field("game_over").is_truthy();
        let status_text = match self.vm.get_state_field("status_text") {
            Val::Str(s) => sanitize_text(&s),
            _ => String::new(),
        };
        GameHudSnapshot {
            score,
            high_score,
            game_over,
            status_text,
        }
    }

    fn extract_and_sync_game_hud(
        &mut self,
        eff_dt: f64,
        input: &SceneInputState,
    ) -> Result<GameHudSnapshot, RuntimeError> {
        let mut hud = self.read_game_hud_snapshot();
        self.telemetry.current_score = hud.score;
        if hud.score > self.telemetry.best_score {
            self.telemetry.best_score = hud.score;
        }
        if hud.high_score > self.telemetry.best_score {
            self.telemetry.best_score = hud.high_score;
        }
        hud.high_score = self.telemetry.best_score;
        let cur_state_hi = sanitize_f64(self.vm.get_state_field("high_score").as_num()).round() as i64;
        if hud.high_score > cur_state_hi {
            self.vm.set_state_field("high_score", Val::Num(hud.high_score as f64))?;
        }

        if hud.game_over && !self.prev_game_over {
            self.telemetry.game_over_count = self.telemetry.game_over_count.saturating_add(1);
        }
        self.prev_game_over = hud.game_over;

        if !self.timeline.is_paused() && !hud.game_over && eff_dt > 0.0 {
            let dt_ms = eff_dt * 1000.0;
            self.telemetry.play_ms = self.telemetry.play_ms.saturating_add(dt_ms.round() as u64);

            // Count rising edges of discrete controls
            let rising = (input.left && !self.prev_input.left)
                || (input.right && !self.prev_input.right)
                || (input.up && !self.prev_input.up)
                || (input.down && !self.prev_input.down)
                || (input.action && !self.prev_input.action)
                || (input.clicked && !self.prev_input.clicked)
                || (input.mouse_down && !self.prev_input.mouse_down);
            let held = input.left || input.right || input.up || input.down || input.action || input.mouse_down;

            if rising {
                self.telemetry.input_count = self.telemetry.input_count.saturating_add(1);
                self.hold_input_accum_ms = 0.0;
            } else if held {
                self.hold_input_accum_ms += dt_ms;
                if self.hold_input_accum_ms >= 150.0 {
                    self.telemetry.input_count = self.telemetry.input_count.saturating_add(1);
                    self.hold_input_accum_ms -= 150.0;
                }
            } else {
                self.hold_input_accum_ms = 0.0;
            }
        }
        Ok(hud)
    }
}
