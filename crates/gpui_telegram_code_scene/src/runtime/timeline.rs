//! Deterministic video timeline controller and bounded checkpoint engine.
//!
//! Guarantees that `seek_video(target_sec)` is 100% deterministic regardless
//! of prior variable-dt `step_frame` playback, pause, or speed changes.

use std::collections::BTreeMap;

use crate::lang::VmSnapshot;
use crate::runtime::pixel::PixelBuffer;

pub const MIN_PLAYBACK_SPEED: f64 = 0.5;
pub const MAX_PLAYBACK_SPEED: f64 = 2.0;
pub const MAX_STEP_DT_SEC: f64 = 0.05;
pub const SEEK_STEPS_PER_SEC: u64 = 30;
pub const SEEK_FIXED_DT: f64 = 1.0 / (SEEK_STEPS_PER_SEC as f64);
pub const MAX_SEEK_REPLAY_STEPS: usize = 3_600;
pub const MAX_CHECKPOINT_SECONDS: u32 = 128;
pub const MAX_CHECKPOINT_HEAP_VALUES: usize = 8_192;

#[derive(Debug, Clone)]
pub struct TimelineCheckpoint {
    pub sec: f64,
    pub step_idx: u64,
    pub vm: VmSnapshot,
    pub pixel: Option<PixelBuffer>,
    pub pixel_revision: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct AdvanceOutcome {
    pub eff_dt: f64,
    pub wrapped_loop: bool,
}

#[derive(Debug, Clone)]
pub struct TimelineController {
    timeline_sec: f64,
    duration_sec: Option<f64>,
    playback_speed: f64,
    paused: bool,
    loop_count: u32,
    checkpoint_zero: TimelineCheckpoint,
    seek_checkpoints: BTreeMap<u32, TimelineCheckpoint>,
}

impl TimelineController {
    pub fn new(duration_sec: Option<f64>, checkpoint_zero: TimelineCheckpoint) -> Self {
        Self {
            timeline_sec: 0.0,
            duration_sec,
            playback_speed: 1.0,
            paused: false,
            loop_count: 0,
            checkpoint_zero,
            seek_checkpoints: BTreeMap::new(),
        }
    }

    pub fn timeline_sec(&self) -> f64 {
        self.timeline_sec
    }

    pub fn set_timeline_sec(&mut self, t: f64) {
        self.timeline_sec = if t.is_finite() && t >= 0.0 { t } else { 0.0 };
    }

    pub fn duration_sec(&self) -> Option<f64> {
        self.duration_sec
    }

    pub fn loop_count(&self) -> u32 {
        self.loop_count
    }

    pub fn playback_speed(&self) -> f64 {
        self.playback_speed
    }

    pub fn set_playback_speed(&mut self, speed: f64) {
        if speed.is_finite() {
            self.playback_speed = speed.clamp(MIN_PLAYBACK_SPEED, MAX_PLAYBACK_SPEED);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn checkpoint_zero(&self) -> &TimelineCheckpoint {
        &self.checkpoint_zero
    }

    pub fn reset_with_checkpoint_zero(&mut self, checkpoint_zero: TimelineCheckpoint) {
        self.timeline_sec = 0.0;
        self.loop_count = 0;
        self.paused = false;
        self.checkpoint_zero = checkpoint_zero;
        self.seek_checkpoints.clear();
    }

    /// Clamps `target_sec` safely for `seek_video`.
    pub fn clamp_seek_target(&self, target_sec: f64) -> f64 {
        if !target_sec.is_finite() || target_sec <= 0.0 {
            return 0.0;
        }
        match self.duration_sec {
            Some(dur) if dur.is_finite() && dur > 0.0 => target_sec.clamp(0.0, dur),
            _ => target_sec,
        }
    }

    /// Returns the nearest fixed-dt seek checkpoint at or before `max_sec_key`
    /// whose timestamp satisfies `cp.sec <= target_sec + 1e-9`.
    pub fn nearest_seek_checkpoint(&self, max_sec_key: u32, target_sec: f64) -> &TimelineCheckpoint {
        for (_, cp) in self.seek_checkpoints.range(..=max_sec_key).rev() {
            if cp.sec <= target_sec + 1e-9 {
                return cp;
            }
        }
        &self.checkpoint_zero
    }

    /// Stores an exact integer-second fixed-dt seek checkpoint if within caps.
    pub fn store_seek_checkpoint(&mut self, sec_key: u32, cp: TimelineCheckpoint) {
        if sec_key == 0 || sec_key > MAX_CHECKPOINT_SECONDS {
            return;
        }
        if cp.step_idx != (sec_key as u64) * SEEK_STEPS_PER_SEC {
            return;
        }
        if cp.vm.heap.live_values() > MAX_CHECKPOINT_HEAP_VALUES {
            return;
        }
        self.seek_checkpoints.entry(sec_key).or_insert(cp);
    }

    /// Advances timeline for `step_frame`; handles pause, speed scaling, and Video loop wrap.
    pub fn advance(&mut self, dt_sec: f64) -> AdvanceOutcome {
        if self.paused {
            return AdvanceOutcome {
                eff_dt: 0.0,
                wrapped_loop: false,
            };
        }
        let clean_dt = if dt_sec.is_finite() && dt_sec > 0.0 {
            dt_sec.min(MAX_STEP_DT_SEC)
        } else {
            0.0
        };
        let eff_dt = clean_dt * self.playback_speed;
        self.timeline_sec += eff_dt;
        let mut wrapped_loop = false;
        if eff_dt > 0.0 {
            if let Some(dur) = self.duration_sec {
                if dur.is_finite() && dur > 0.0 && self.timeline_sec >= dur {
                    let wraps = (self.timeline_sec / dur).floor() as u32;
                    self.loop_count = self.loop_count.saturating_add(wraps.max(1));
                    self.timeline_sec = self.timeline_sec.rem_euclid(dur);
                    wrapped_loop = true;
                }
            }
        }
        AdvanceOutcome {
            eff_dt,
            wrapped_loop,
        }
    }
}
