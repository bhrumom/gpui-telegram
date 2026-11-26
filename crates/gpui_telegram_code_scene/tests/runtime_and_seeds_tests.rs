//! Comprehensive runtime, 18-seed catalog, video seek determinism, and all-6-games tests (`CS-F03`, `CS-F04`).

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use gpui_telegram_code_scene::{
    DrawCmd, OFFICIAL_CODE_SCENE_BOTS, RuntimeError, SceneInputState, VmBudgets, VmInstance,
    compile_and_verify, compile_and_verify_with_budgets, compute_cover_hash, find_official_bot,
    find_seed_by_id, official_seed_assets, validate_to_report,
};
use gpui_telegram_proto::{AssetKind, AssetManifest};

#[test]
fn test_all_18_official_seeds_compile_verify_and_run_30_frames() {
    let seeds = official_seed_assets();
    assert_eq!(seeds.len(), 18, "must provide exactly 18 official seeds");
    assert_eq!(OFFICIAL_CODE_SCENE_BOTS.len(), 5);

    let mut ids = HashSet::new();
    let mut cover_hashes = HashSet::new();
    let mut by_kind: BTreeMap<AssetKind, usize> = BTreeMap::new();
    let mut by_bot_and_kind: BTreeMap<(i64, AssetKind), usize> = BTreeMap::new();

    for asset in &seeds {
        assert!(ids.insert(asset.scene_id.clone()), "duplicate scene_id {}", asset.scene_id);
        assert_eq!(asset.code_format, "cs1");
        assert!(find_official_bot(asset.author_user_id).is_some());
        assert_eq!(asset.creator_bot_id, Some(asset.author_user_id));
        assert!(!asset.viewer_liked && !asset.viewer_bookmarked && !asset.viewer_following_author);
        assert!(asset.tags.len() >= 4, "seed {} needs rich tags", asset.scene_id);

        *by_kind.entry(asset.kind).or_default() += 1;
        let bk = by_bot_and_kind.entry((asset.author_user_id, asset.kind)).or_default();
        *bk += 1;
        assert!(
            *bk <= 2,
            "bot {} has >2 assets in modality {:?} (violates M2 scatter guarantee)",
            asset.author_username,
            asset.kind
        );

        let report = validate_to_report(&asset.source_code, &asset.manifest);
        assert!(
            report.ok,
            "seed {} failed validation: {:?}",
            asset.scene_id, report.diagnostics
        );
        assert_eq!(report.frames_executed, 5);
        assert!(!report.feature_tokens.is_empty());
        let hash = report.cover_hash.expect("cover_hash present");
        assert_eq!(hash.len(), 64);
        assert!(
            cover_hashes.insert(hash.clone()),
            "seed {} collided cover_hash {}",
            asset.scene_id, hash
        );

        // Run 30 real frames through VmInstance
        let verified = compile_and_verify(&asset.source_code, &asset.manifest).unwrap();
        let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
        let mut rich_draws = 0usize;
        let mut saw_pixel = false;
        for f in 0..30 {
            let input = SceneInputState {
                pointer_active: true,
                mouse_x: 100.0 + (f as f64) * 5.0,
                mouse_y: 220.0 + (f as f64) * 4.0,
                clicked: f == 10,
                action: f % 6 == 0,
                ..SceneInputState::default()
            };
            let t0 = Instant::now();
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap_or_else(|e| {
                panic!("seed {} failed on frame {f}: {e}", asset.scene_id)
            });
            let elapsed_ms = t0.elapsed().as_millis();
            assert!(
                out.ops_used < 50_000,
                "seed {} used {} ops (too close to cap)",
                asset.scene_id, out.ops_used
            );
            assert!(
                elapsed_ms < 25,
                "seed {} frame {f} took {elapsed_ms}ms",
                asset.scene_id
            );
            assert!(
                out.draw_cmds.iter().all(DrawCmd::all_floats_finite),
                "seed {} emitted non-finite draw cmd",
                asset.scene_id
            );
            rich_draws = rich_draws.max(out.draw_cmds.len());
            if let Some(ref pf) = out.pixel_bgra {
                assert_eq!(pf.bgra.len(), (pf.width as usize) * (pf.height as usize) * 4);
                saw_pixel = true;
            }
        }
        assert!(
            rich_draws >= 4 || (saw_pixel && rich_draws >= 2),
            "seed {} output too sparse (max draws={rich_draws}, pixel={saw_pixel})",
            asset.scene_id
        );
    }

    assert_eq!(by_kind.get(&AssetKind::Video), Some(&6));
    assert_eq!(by_kind.get(&AssetKind::Animation), Some(&6));
    assert_eq!(by_kind.get(&AssetKind::Game), Some(&6));
    assert!(find_seed_by_id("seed_game_flappy_bird").is_some());
    assert!(find_seed_by_id("nonexistent_seed").is_none());
}

#[test]
fn test_video_seek_determinism_pause_speed_and_loop_wrap() {
    // 1. All 6 seeded pure-timeline videos seek deterministically across arbitrary order
    for asset in official_seed_assets()
        .into_iter()
        .filter(|a| a.kind == AssetKind::Video)
    {
        let verified = compile_and_verify(&asset.source_code, &asset.manifest).unwrap();
        let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
        let dur = asset.manifest.duration_sec.unwrap();

        let f_mid_1 = vm.seek_video(dur * 0.5).unwrap();
        let h1 = compute_cover_hash(
            asset.manifest.width,
            asset.manifest.height,
            &f_mid_1.draw_cmds,
            f_mid_1.pixel_bgra.as_ref(),
        );

        // Interleave variable-dt playback, pause, speed changes, and backward seek
        vm.set_playback_speed(2.0);
        for _ in 0..10 {
            vm.step_frame(0.037, &SceneInputState::default()).unwrap();
        }
        let _ = vm.seek_video(1.25).unwrap();
        let f_mid_2 = vm.seek_video(dur * 0.5).unwrap();
        let h2 = compute_cover_hash(
            asset.manifest.width,
            asset.manifest.height,
            &f_mid_2.draw_cmds,
            f_mid_2.pixel_bgra.as_ref(),
        );
        assert_eq!(h1, h2, "video {} seek determinism mismatch", asset.scene_id);
        assert_eq!(f_mid_1.draw_cmds, f_mid_2.draw_cmds);
    }

    // 2. Stateful `on_update` video: seek_video(t) MUST be isolated from variable-dt step_frame!
    let stateful_video = r#"
        fn on_init(state) {
            state.x = 20;
            state.v = 35;
        }
        fn on_update(state, dt, input) {
            state.x += state.v * dt;
        }
        fn on_render(state, t, canvas) {
            clear(rgb(10, 12, 24));
            circle(state.x, 320 + sin(t * 2) * 40, 18, rgb(90, 240, 255));
        }
    "#;
    let manifest = AssetManifest {
        kind: AssetKind::Video,
        duration_sec: Some(6.0),
        ..AssetManifest::default()
    };
    let verified = compile_and_verify(stateful_video, &manifest).unwrap();
    let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();

    // Seek directly to 3.25s
    let s1 = vm.seek_video(3.25).unwrap();
    // Now run irregular variable-dt playback + speed clamp + pause tests
    vm.set_playback_speed(99.0);
    assert_eq!(vm.playback_speed(), 2.0);
    vm.set_playback_speed(0.1);
    assert_eq!(vm.playback_speed(), 0.5);
    vm.set_playback_speed(f64::NAN);
    assert_eq!(vm.playback_speed(), 0.5);

    let before_pause = vm.step_frame(0.04, &SceneInputState::default()).unwrap();
    vm.set_paused(true);
    assert!(vm.is_paused());
    let during_pause = vm.step_frame(0.04, &SceneInputState::default()).unwrap();
    assert_eq!(during_pause.timeline_sec, before_pause.timeline_sec);
    assert!(!during_pause.draw_cmds.is_empty());

    vm.set_paused(false);
    vm.set_playback_speed(2.0);
    // Advance past 6.0s duration to trigger loop wrap
    let mut wrapped = false;
    for _ in 0..80 {
        let out = vm.step_frame(0.05, &SceneInputState::default()).unwrap();
        if out.loop_count >= 1 {
            wrapped = true;
            break;
        }
    }
    assert!(wrapped, "video must wrap loop_count when timeline exceeds duration");

    // Seeking back to 3.25s after variable-dt playback and loop wrap MUST match s1 bit-for-bit!
    let s2 = vm.seek_video(3.25).unwrap();
    assert_eq!(s1.draw_cmds, s2.draw_cmds);

    // Out-of-range seek clamps cleanly without panic
    let clamped_hi = vm.seek_video(999.0).unwrap();
    assert_eq!(clamped_hi.timeline_sec, 6.0);
    let clamped_lo = vm.seek_video(-50.0).unwrap();
    assert_eq!(clamped_lo.timeline_sec, 0.0);
}

fn load_game_vm(scene_id: &str) -> VmInstance {
    let asset = find_seed_by_id(scene_id).unwrap_or_else(|| panic!("missing seed {scene_id}"));
    assert_eq!(asset.kind, AssetKind::Game);
    let verified = compile_and_verify(&asset.source_code, &asset.manifest).unwrap();
    VmInstance::new(verified, VmBudgets::default()).unwrap()
}

fn verify_game_restart_preserves_high_score(vm: &mut VmInstance, expected_min_best: i64, name: &str) {
    let tel = vm.game_telemetry();
    assert!(tel.play_ms > 0, "{name}: play_ms should be > 0");
    assert!(tel.best_score >= expected_min_best, "{name}: best_score {} < {}", tel.best_score, expected_min_best);
    assert!(tel.game_over_count >= 1, "{name}: game_over_count should be >= 1");

    vm.restart().unwrap();
    let after_restart = vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
    let hud = after_restart.game_hud.expect("game hud present");
    assert_eq!(hud.score, 0, "{name}: score should reset to 0 after restart");
    assert!(!hud.game_over, "{name}: game_over should reset to false after restart");
    assert!(
        hud.high_score >= expected_min_best,
        "{name}: high_score {} must preserve best_score >= {}",
        hud.high_score, expected_min_best
    );
    assert_eq!(vm.game_telemetry().restarts, 1, "{name}: restarts counter must increment");
}

#[test]
fn test_all_6_playable_games_input_score_game_over_telemetry_and_restart() {
    // 1. Flappy Cyber-Bird: rhythmic flap clears pipe 1 (score >= 1), then idle crashes (game_over)
    {
        let mut vm = load_game_vm("seed_game_flappy_bird");
        let mut scored = false;
        let mut died = false;
        for f in 0..240 {
            let flap = !scored && (f % 44 == 0);
            let input = SceneInputState { action: flap, ..Default::default() };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            let hud = out.game_hud.unwrap();
            if hud.score >= 1 { scored = true; }
            if scored && hud.game_over { died = true; break; }
        }
        assert!(scored, "flappy bird should score >= 1 with periodic flaps");
        assert!(died, "flappy bird should reach game_over when idle");
        assert!(vm.game_telemetry().input_count > 0);
        verify_game_restart_preserves_high_score(&mut vm, 1, "flappy_bird");
    }

    // 2. Starfield Space Shooter: fire kills first enemy (+10), then idle lets enemies breach -> game_over
    {
        let mut vm = load_game_vm("seed_game_space_shooter");
        let mut scored = false;
        let mut died = false;
        for _ in 0..420 {
            let input = SceneInputState { action: !scored, ..Default::default() };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            let hud = out.game_hud.unwrap();
            if hud.score >= 10 { scored = true; }
            if scored && hud.game_over { died = true; break; }
        }
        assert!(scored, "space shooter should score >= 10 firing at enemy");
        assert!(died, "space shooter should reach game_over when idle");
        assert!(vm.game_telemetry().input_count > 0);
        verify_game_restart_preserves_high_score(&mut vm, 10, "space_shooter");
    }

    // 3. Cyberpunk Brick Breaker: opening shot breaks brick (+10), steer paddle left drains 2 lives -> game_over
    {
        let mut vm = load_game_vm("seed_game_brick_breaker");
        let mut scored = false;
        let mut died = false;
        for _ in 0..360 {
            let input = SceneInputState { left: true, ..Default::default() };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            let hud = out.game_hud.unwrap();
            if hud.score >= 10 { scored = true; }
            if hud.game_over { died = true; break; }
        }
        assert!(scored, "brick breaker should break brick and score >= 10");
        assert!(died, "brick breaker should reach game_over after steering paddle away");
        assert!(vm.game_telemetry().input_count > 0);
        verify_game_restart_preserves_high_score(&mut vm, 10, "brick_breaker");
    }

    // 4. Turbo Neon Snake: turbo straight right eats food (+10) then hits right wall -> game_over
    {
        let mut vm = load_game_vm("seed_game_neon_snake");
        let mut scored = false;
        let mut died = false;
        for _ in 0..150 {
            let input = SceneInputState { right: true, action: true, ..Default::default() };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            let hud = out.game_hud.unwrap();
            if hud.score >= 10 { scored = true; }
            if hud.game_over { died = true; break; }
        }
        assert!(scored, "neon snake should eat initial food and score >= 10");
        assert!(died, "neon snake should hit right wall and reach game_over");
        assert!(vm.game_telemetry().input_count > 0);
        verify_game_restart_preserves_high_score(&mut vm, 10, "neon_snake");
    }

    // 5. Gravity Orbit Catcher: catches initial star (+15), then takes 2 meteors -> game_over
    {
        let mut vm = load_game_vm("seed_game_orbit_catcher");
        let mut scored = false;
        let mut died = false;
        for f in 0..240 {
            // Brief tap after scoring to exercise input telemetry, keep angle aligned
            let input = SceneInputState {
                left: f == 1,
                right: f == 2,
                ..Default::default()
            };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            let hud = out.game_hud.unwrap();
            if hud.score >= 15 { scored = true; }
            if hud.game_over { died = true; break; }
        }
        assert!(scored, "orbit catcher should catch initial star and score >= 15");
        assert!(died, "orbit catcher should reach game_over after 2 meteors");
        assert!(vm.game_telemetry().input_count > 0);
        verify_game_restart_preserves_high_score(&mut vm, 15, "orbit_catcher");
    }

    // 6. Neon Paddle Pong: center paddle returns serve (+10), then steer left misses 3 -> game_over
    {
        let mut vm = load_game_vm("seed_game_neon_pong");
        let mut scored = false;
        let mut died = false;
        for _ in 0..420 {
            let input = SceneInputState {
                action: !scored,
                left: scored,
                ..Default::default()
            };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            let hud = out.game_hud.unwrap();
            if hud.score >= 10 { scored = true; }
            if scored && hud.game_over { died = true; break; }
        }
        assert!(scored, "neon pong should return opening serve and score >= 10");
        assert!(died, "neon pong should reach game_over after 3 misses");
        assert!(vm.game_telemetry().input_count > 0);
        verify_game_restart_preserves_high_score(&mut vm, 10, "neon_pong");
    }
}

#[test]
fn test_validator_rejects_empty_invisible_and_syntax_error_scripts() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);

    // Empty render (0 draw commands) -> E_NO_VISIBLE_OUTPUT
    let empty_render = "fn on_render(state, t, canvas) { let x = 1 + 2; }";
    let r1 = validate_to_report(empty_render, &manifest);
    assert!(!r1.ok);
    assert_eq!(r1.diagnostics[0].code, "E_NO_VISIBLE_OUTPUT");
    assert_eq!(r1.diagnostics[0].stage.as_deref(), Some("dry_run"));

    // Fully transparent alpha=0 draw -> E_NO_VISIBLE_OUTPUT
    let alpha_zero = "fn on_render(state, t, canvas) { clear(rgba(255, 255, 255, 0)); }";
    let r2 = validate_to_report(alpha_zero, &manifest);
    assert!(!r2.ok);
    assert_eq!(r2.diagnostics[0].code, "E_NO_VISIBLE_OUTPUT");

    // Missing on_render hook -> E_MISSING_ON_RENDER
    let no_render = "fn on_init(state) { state.x = 1; }";
    let r3 = validate_to_report(no_render, &manifest);
    assert!(!r3.ok);
    assert_eq!(r3.diagnostics[0].code, "E_MISSING_ON_RENDER");
    assert_eq!(r3.diagnostics[0].stage.as_deref(), Some("compile"));

    // Syntax error surfaces line/col and stage="parse"
    let bad_syntax = "fn on_render(s, t, c) {\n  let x = ;\n}";
    let r4 = validate_to_report(bad_syntax, &manifest);
    assert!(!r4.ok);
    assert_eq!(r4.diagnostics[0].stage.as_deref(), Some("parse"));
    assert_eq!(r4.diagnostics[0].line, Some(2));
}

#[test]
fn test_pixel_init_error_variant_direct() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);
    let src = r#"
        fn on_render(s, t, c) {
            if t > 0.02 { pixel_init(500, 500); }
            clear(rgb(20, 20, 40));
        }
    "#;
    let verified = compile_and_verify(src, &manifest).unwrap_err();
    assert!(verified.message.contains("pixel"));

    // Direct runtime PixelBudgetExceeded check on step_frame
    let deferred = r#"
        fn on_render(s, t, c) {
            if t > 0.2 { pixel_init(500, 500); }
            clear(rgb(20, 20, 40));
        }
    "#;
    let ok = compile_and_verify(deferred, &manifest).unwrap();
    let mut vm = VmInstance::new(ok, VmBudgets::default()).unwrap();
    let mut hit = false;
    for _ in 0..20 {
        if let Err(RuntimeError::PixelBudgetExceeded { .. }) =
            vm.step_frame(1.0 / 60.0, &SceneInputState::default())
        {
            hit = true;
            break;
        }
    }
    assert!(hit, "must fail closed with RuntimeError::PixelBudgetExceeded");
}

#[test]
fn test_endurance_3500_frames_benign_game_and_official_seeds_zero_heap_leak() {
    // 1. Benign zero-alloc Game stepped 3,500 frames (~58.3s @ 60fps) with default heap cap
    let benign_game = r#"
        fn on_init(state) { state.ticks = 0; state.score = 0; }
        fn on_update(state, dt, input) { state.ticks += 1; state.score = state.ticks; }
        fn on_render(state, t, c) {
            clear(rgb(12, 14, 24));
            circle(180, 320, 20, rgb(80, 220, 255));
        }
    "#;
    let game_manifest = AssetManifest::for_kind(AssetKind::Game);
    let verified = compile_and_verify(benign_game, &game_manifest).unwrap();
    let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
    let mut last_score = 0;
    for _ in 0..3_500 {
        let out = vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
        last_score = out.game_hud.unwrap().score;
    }
    assert_eq!(last_score, 3_500);

    // 2. ALL 12 official Animations & Games run cleanly for 3,500 frames (~58.3s @ 60fps)
    //    with ZERO MemoryBudgetExceeded under the exact 32,768 heap value cap!
    let endurance_budgets = VmBudgets {
        max_frame_wall_ms: 500,
        ..VmBudgets::default()
    };
    let anims_and_games: Vec<_> = official_seed_assets()
        .into_iter()
        .filter(|a| matches!(a.kind, AssetKind::Animation | AssetKind::Game))
        .collect();
    assert_eq!(anims_and_games.len(), 12);
    for asset in &anims_and_games {
        let verified = compile_and_verify_with_budgets(
            &asset.source_code,
            &asset.manifest,
            endurance_budgets,
        )
        .unwrap();
        let mut vm = VmInstance::new(verified, endurance_budgets).unwrap();
        for f in 0..3_500 {
            let input = SceneInputState {
                pointer_active: true,
                mouse_x: 180.0,
                mouse_y: 320.0,
                action: f % 30 == 0,
                ..SceneInputState::default()
            };
            vm.step_frame(1.0 / 60.0, &input).unwrap_or_else(|e| {
                panic!("endurance regression: {} failed on frame {f}: {e}", asset.scene_id)
            });
        }
    }
}

#[test]
fn test_stateful_video_seek_integer_grid_determinism_pixels_and_120s() {
    // 1. RNG & tick-counting stateful video: immune to future-checkpoint & partial-step poisoning
    let rng_video = r#"
        fn on_init(state) {
            state.ticks = 0;
            state.acc_r = 0;
            state.cx = 40;
            state.cy = 200;
        }
        fn on_update(state, dt, input) {
            state.ticks += 1;
            state.acc_r += floor(rand() * 1000);
            state.cx = 40 + (state.ticks * 1.3) % 280;
            state.cy = 120 + sin(state.acc_r * 0.01) * 180;
        }
        fn on_render(state, t, canvas) {
            clear(rgb(10, 12, 26));
            circle(state.cx, state.cy, 16, rgb(100, 240, 255));
            text(10, 20, str(state.ticks) + ":" + str(state.acc_r), 12, rgb(255, 255, 255), "left");
        }
    "#;
    let m15 = AssetManifest {
        kind: AssetKind::Video,
        duration_sec: Some(15.0),
        seed: 4242,
        ..AssetManifest::default()
    };
    let v15 = compile_and_verify(rng_video, &m15).unwrap();

    // seek(11.3) -> seek(1.0) vs direct seek(1.0)
    let mut vm_warm = VmInstance::new(v15.clone(), VmBudgets::default()).unwrap();
    let _ = vm_warm.seek_video(11.3).unwrap();
    let warm_1_0 = vm_warm.seek_video(1.0).unwrap();
    let mut vm_cold = VmInstance::new(v15.clone(), VmBudgets::default()).unwrap();
    let cold_1_0 = vm_cold.seek_video(1.0).unwrap();
    assert_eq!(warm_1_0.draw_cmds, cold_1_0.draw_cmds);

    // seek(1.01) -> seek(2.7) vs direct seek(2.7)
    let mut vm_p1 = VmInstance::new(v15.clone(), VmBudgets::default()).unwrap();
    let _ = vm_p1.seek_video(1.01).unwrap();
    let warm_2_7 = vm_p1.seek_video(2.7).unwrap();
    let mut vm_c2 = VmInstance::new(v15.clone(), VmBudgets::default()).unwrap();
    let cold_2_7 = vm_c2.seek_video(2.7).unwrap();
    assert_eq!(warm_2_7.draw_cmds, cold_2_7.draw_cmds);

    // seek(3.01) -> seek(3.0) vs direct seek(3.0)
    let mut vm_p3 = VmInstance::new(v15.clone(), VmBudgets::default()).unwrap();
    let _ = vm_p3.seek_video(3.01).unwrap();
    let warm_3_0 = vm_p3.seek_video(3.0).unwrap();
    let mut vm_c3 = VmInstance::new(v15, VmBudgets::default()).unwrap();
    let cold_3_0 = vm_c3.seek_video(3.0).unwrap();
    assert_eq!(warm_3_0.draw_cmds, cold_3_0.draw_cmds);

    // 2. Stateful pixel-mutating Video: seek(5.0) -> seek(1.0) vs direct seek(1.0)
    let px_stateful = r#"
        fn on_init(state) {
            pixel_init(32, 32);
            state.step = 0;
        }
        fn on_update(state, dt, input) {
            state.step += 1;
            let x = state.step % 32;
            let y = floor(state.step / 32) % 32;
            pixel_set(x, y, rgb((state.step * 7) % 255, 180, 240));
        }
        fn on_render(state, t, c) {
            rect(4, 4, 20 + state.step % 40, 12, rgb(255, 220, 80));
        }
    "#;
    let m8 = AssetManifest {
        kind: AssetKind::Video,
        duration_sec: Some(8.0),
        ..AssetManifest::default()
    };
    let v_px = compile_and_verify(px_stateful, &m8).unwrap();
    let mut vm_px_warm = VmInstance::new(v_px.clone(), VmBudgets::default()).unwrap();
    let _ = vm_px_warm.seek_video(5.0).unwrap();
    let px_warm_1 = vm_px_warm.seek_video(1.0).unwrap();
    let mut vm_px_cold = VmInstance::new(v_px, VmBudgets::default()).unwrap();
    let px_cold_1 = vm_px_cold.seek_video(1.0).unwrap();
    assert_eq!(px_warm_1.draw_cmds, px_cold_1.draw_cmds);
    assert_eq!(px_warm_1.pixel_bgra, px_cold_1.pixel_bgra);

    // 3. Pure-timeline video calling pixel_init in on_render: repeated seek_video bit-identical incl. revision
    let pure_px = r#"
        fn on_render(state, t, c) {
            pixel_init(32, 32);
            pixel_rect(0, 0, 32, 32, rgb(20, 120, 220));
            circle(180, 320, 24, rgb(255, 255, 255));
        }
    "#;
    let v_pure = compile_and_verify(pure_px, &m8).unwrap();
    let mut vm_pure = VmInstance::new(v_pure, VmBudgets::default()).unwrap();
    let r1 = vm_pure.seek_video(3.0).unwrap();
    let r2 = vm_pure.seek_video(3.0).unwrap();
    assert_eq!(r1, r2, "pure-timeline repeated seek must be bit-identical including pixel revision");

    // 4. 120s stateful Video: compile_and_verify (60s midpoint seek) + seek(105.0) + seek(120.0)
    //    all succeed cleanly within default VmBudgets (max_seek_wall_ms = 100ms, max_heap_values = 32_768)
    let m120 = AssetManifest {
        kind: AssetKind::Video,
        duration_sec: Some(120.0),
        seed: 77,
        ..AssetManifest::default()
    };
    let v120 = compile_and_verify(rng_video, &m120).expect("120s stateful video verify must pass");
    let mut vm120 = VmInstance::new(v120, VmBudgets::default()).unwrap();
    let s105 = vm120.seek_video(105.0).expect("seek(105.0) must succeed");
    assert!((s105.timeline_sec - 105.0).abs() < 1e-6);
    let s120 = vm120.seek_video(120.0).expect("seek(120.0) must succeed");
    assert!((s120.timeline_sec - 120.0).abs() < 1e-6);

    // 5. Allocating stateful Video exercises pre-checkpoint GC (> 2_048 live_values) and stays deterministic
    let alloc_video = r#"
        fn on_init(state) { state.n = 0; }
        fn on_update(state, dt, input) {
            let tmp = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];
            state.n += len(tmp);
        }
        fn on_render(state, t, c) {
            clear(rgb(10, 10, 20));
            text(10, 20, str(state.n), 12, rgb(255, 255, 255), "left");
        }
    "#;
    let v_alloc = compile_and_verify(alloc_video, &m15).unwrap();
    let mut vm_a1 = VmInstance::new(v_alloc.clone(), VmBudgets::default()).unwrap();
    let _ = vm_a1.seek_video(12.0).unwrap();
    let a1_4 = vm_a1.seek_video(4.0).unwrap();
    let mut vm_a2 = VmInstance::new(v_alloc, VmBudgets::default()).unwrap();
    let a2_4 = vm_a2.seek_video(4.0).unwrap();
    assert_eq!(a1_4.draw_cmds, a2_4.draw_cmds);
}

#[test]
fn test_video_zero_dt_loop_guard_and_cover_hash_temporal_consistency() {
    // 1. seek_video(dur) followed by step_frame(0.0 | NaN | -1.0) MUST NOT wrap loop_count
    let vid = r#"
        fn on_render(state, t, c) {
            clear(rgb(10, 10, 20));
            circle(100 + t * 20, 320, 20, rgb(90, 220, 255));
        }
    "#;
    let m5 = AssetManifest {
        kind: AssetKind::Video,
        duration_sec: Some(5.0),
        ..AssetManifest::default()
    };
    let v5 = compile_and_verify(vid, &m5).unwrap();
    let mut vm = VmInstance::new(v5, VmBudgets::default()).unwrap();
    let at_end = vm.seek_video(5.0).unwrap();
    assert_eq!(at_end.timeline_sec, 5.0);
    assert_eq!(at_end.loop_count, 0);
    for dt in [0.0, f64::NAN, -1.0] {
        let out = vm.step_frame(dt, &SceneInputState::default()).unwrap();
        assert_eq!(out.loop_count, 0, "dt={dt} after seek(dur) must not wrap loop_count");
        assert_eq!(out.timeline_sec, 5.0, "dt={dt} after seek(dur) must preserve timeline_sec");
    }
    let wrapped = vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
    assert_eq!(wrapped.loop_count, 1, "positive dt past dur must wrap loop_count");

    // 2. cover_hash temporal consistency: pixel buffer painted BLUE in on_init (untouched on frame 4),
    //    mutated to RED during midpoint seek (t > 2.0). cover_hash MUST hash frame 4's BLUE pixels!
    let cover_vid = r#"
        fn on_init(state) {
            pixel_init(16, 16);
            pixel_rect(0, 0, 16, 16, rgb(0, 40, 255));
        }
        fn on_render(state, t, c) {
            if t > 2.0 {
                pixel_rect(0, 0, 16, 16, rgb(255, 20, 0));
            }
            rect(10, 10, 40, 40, rgb(255, 255, 255));
        }
    "#;
    let m10 = AssetManifest {
        kind: AssetKind::Video,
        duration_sec: Some(10.0),
        ..AssetManifest::default()
    };
    let verified = compile_and_verify(cover_vid, &m10).unwrap();
    let mut check_vm = VmInstance::new(verified.clone(), VmBudgets::default()).unwrap();
    let mut frame4 = None;
    for _ in 0..5 {
        frame4 = Some(check_vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap());
    }
    let f4 = frame4.unwrap();
    assert!(f4.pixel_bgra.is_none(), "frame 4 should have clean (None) pixel_bgra");
    let blue_snap = check_vm.pixel_snapshot().expect("blue snapshot present");
    let expected_hash = compute_cover_hash(m10.width, m10.height, &f4.draw_cmds, Some(&blue_snap));
    let mid_frame = check_vm.seek_video(5.0).unwrap();
    let red_snap = mid_frame.pixel_bgra.expect("red midpoint snapshot");
    let wrong_mixed_hash = compute_cover_hash(m10.width, m10.height, &f4.draw_cmds, Some(&red_snap));
    assert_ne!(expected_hash, wrong_mixed_hash);
    assert_eq!(
        verified.cover_hash, expected_hash,
        "validator cover_hash must use frame 4 pixel buffer, not midpoint seek pixel buffer"
    );
}

#[test]
fn test_pixel_frame_revision_strictly_monotonic_across_pixel_init_and_restart() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);

    // 1. Frame 1: pixel_init + pixel_set (red pixel); Frame 2: fresh pixel_init (all-black).
    //    Consecutive frames with distinct buffers MUST emit strictly increasing revision!
    let reinit_src = r#"
        fn on_init(state) { state.frame = 0; }
        fn on_update(state, dt, input) { state.frame += 1; }
        fn on_render(state, t, c) {
            pixel_init(16, 16);
            if state.frame == 1 {
                pixel_set(0, 0, rgb(255, 0, 0));
            }
            rect(0, 0, 16, 16, rgb(255, 255, 255));
        }
    "#;
    let verified = compile_and_verify(reinit_src, &manifest).unwrap();
    let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
    let f1 = vm
        .step_frame(1.0 / 60.0, &SceneInputState::default())
        .unwrap();
    let f2 = vm
        .step_frame(1.0 / 60.0, &SceneInputState::default())
        .unwrap();
    let pf1 = f1.pixel_bgra.expect("frame 1 pixel_bgra present");
    let pf2 = f2.pixel_bgra.expect("frame 2 pixel_bgra present");
    assert_ne!(pf1.bgra, pf2.bgra, "frame 1 and frame 2 pixel buffers differ");
    assert!(
        pf2.revision > pf1.revision,
        "pixel_init in frame 2 must advance revision strictly past frame 1 ({} > {})",
        pf2.revision,
        pf1.revision
    );

    // 2. Pre-restart pixel mutations vs post-restart on_init pixels MUST emit strictly increasing revision!
    let restart_src = r#"
        fn on_init(state) {
            pixel_init(16, 16);
            pixel_rect(0, 0, 16, 16, rgb(0, 40, 255));
            state.mutated = false;
        }
        fn on_update(state, dt, input) {
            if !state.mutated {
                pixel_set(0, 0, rgb(255, 0, 0));
                state.mutated = true;
            }
        }
        fn on_render(state, t, c) {
            rect(0, 0, 16, 16, rgb(255, 255, 255));
        }
    "#;
    let verified_r = compile_and_verify(restart_src, &manifest).unwrap();
    let mut vm_r = VmInstance::new(verified_r, VmBudgets::default()).unwrap();
    let pre = vm_r
        .step_frame(1.0 / 60.0, &SceneInputState::default())
        .unwrap();
    let pre_pf = pre.pixel_bgra.expect("pre-restart pixel_bgra present");
    assert_eq!(&pre_pf.bgra[0..4], &[0, 0, 255, 255]); // red pixel in BGRA

    vm_r.restart().unwrap();
    // Pause so on_update doesn't run red pixel mutation; inspect post-restart on_init blue buffer
    vm_r.set_paused(true);
    let post = vm_r
        .step_frame(1.0 / 60.0, &SceneInputState::default())
        .unwrap();
    let post_pf = post.pixel_bgra.expect("post-restart pixel_bgra present");
    assert_eq!(&post_pf.bgra[0..4], &[255, 40, 0, 255]); // blue pixel in BGRA
    assert!(
        post_pf.revision > pre_pf.revision,
        "post-restart pixel revision must strictly exceed pre-restart ({} > {})",
        post_pf.revision,
        pre_pf.revision
    );

    // Second restart continues strictly increasing
    vm_r.restart().unwrap();
    let post2 = vm_r
        .step_frame(1.0 / 60.0, &SceneInputState::default())
        .unwrap();
    let post2_pf = post2.pixel_bgra.expect("second post-restart pixel_bgra present");
    assert!(
        post2_pf.revision > post_pf.revision,
        "second restart pixel revision must strictly exceed first ({} > {})",
        post2_pf.revision,
        post_pf.revision
    );
}

#[test]
fn test_stateful_video_loop_wrap_pixel_revision_monotonic_and_seek_deterministic() {
    let m5 = AssetManifest {
        kind: AssetKind::Video,
        duration_sec: Some(5.0),
        ..AssetManifest::default()
    };

    // Case 1 (F03.11a): Distinct pixel bytes across stateful Video loop wrap MUST NOT collide revision
    let collision_src = r#"
        fn on_init(state) {
            pixel_init(16, 16);
            pixel_rect(0, 0, 16, 16, rgb(0, 40, 255));
            state.frame = 0;
        }
        fn on_update(state, dt, input) {
            state.frame += 1;
        }
        fn on_render(state, t, c) {
            if state.frame == 1 {
                pixel_set(0, 0, rgb(floor(t * 2000) % 255, 20, 20));
            }
            rect(4, 4, 16, 16, rgb(255, 255, 255));
        }
    "#;
    let v_coll = compile_and_verify(collision_src, &m5).unwrap();
    let mut vm_coll = VmInstance::new(v_coll.clone(), VmBudgets::default()).unwrap();
    let f0 = vm_coll.step_frame(0.03, &SceneInputState::default()).unwrap();
    assert_eq!(f0.loop_count, 0);
    let pf0 = f0.pixel_bgra.expect("frame 0 pixel_bgra present");
    let mut wrapped_pf = None;
    for _ in 0..200 {
        let out = vm_coll.step_frame(0.03, &SceneInputState::default()).unwrap();
        if out.loop_count == 1 {
            wrapped_pf = out.pixel_bgra;
            break;
        }
    }
    let pf_wrap = wrapped_pf.expect("wrapped frame must emit fresh pixel_bgra");
    assert_ne!(
        &pf_wrap.bgra[0..4],
        &pf0.bgra[0..4],
        "wrapped frame paints distinct pixel bytes at (0,0)"
    );
    assert!(
        pf_wrap.revision > pf0.revision,
        "wrapped frame revision must strictly exceed frame 0 ({} > {})",
        pf_wrap.revision,
        pf0.revision
    );

    // Case 2 (F03.11b): Stateful shader Video (cp0.pixel == None) stays strictly monotonic across loop wrap
    let shader_vid = r#"
        fn on_init(state) { state.phase = 0.0; }
        fn on_update(state, dt, input) { state.phase += dt; }
        fn on_render(state, t, c) {
            shader_plasma(t + state.phase, 2.0, 0);
            rect(4, 4, 10, 10, rgb(255, 255, 255));
        }
    "#;
    let v_sh = compile_and_verify(shader_vid, &m5).unwrap();
    let mut vm_sh = VmInstance::new(v_sh, VmBudgets::default()).unwrap();
    let mut prev_rev = 0u64;
    let mut saw_wrap = false;
    for _ in 0..125 {
        let out = vm_sh.step_frame(0.05, &SceneInputState::default()).unwrap();
        let pf = out.pixel_bgra.expect("shader video emits pixel_bgra every frame");
        assert!(
            pf.revision > prev_rev,
            "stateful shader video revision must stay strictly monotonic across loop wrap ({} > {}, loop={})",
            pf.revision,
            prev_rev,
            out.loop_count
        );
        prev_rev = pf.revision;
        if out.loop_count >= 1 {
            saw_wrap = true;
        }
    }
    assert!(saw_wrap, "shader video must wrap at least once within 125 frames");

    // Case 3 (F03.11c): cp0.pixel restored (BLUE -> RED -> wrap BLUE without new mutation) emits monotonic rev
    let restore_vid = r#"
        fn on_init(state) {
            pixel_init(16, 16);
            pixel_rect(0, 0, 16, 16, rgb(0, 40, 255));
            state.frame = 0;
        }
        fn on_update(state, dt, input) {
            state.frame += 1;
            if state.frame == 2 {
                pixel_set(0, 0, rgb(255, 0, 0));
            }
        }
        fn on_render(state, t, c) {
            rect(4, 4, 16, 16, rgb(255, 255, 255));
        }
    "#;
    let v_res = compile_and_verify(restore_vid, &m5).unwrap();
    let mut vm_res = VmInstance::new(v_res.clone(), VmBudgets::default()).unwrap();
    let r0 = vm_res.step_frame(0.05, &SceneInputState::default()).unwrap();
    let pf_blue0 = r0.pixel_bgra.expect("frame 0 initial BLUE pixel_bgra");
    assert_eq!(&pf_blue0.bgra[0..4], &[255, 40, 0, 255]);
    let r1 = vm_res.step_frame(0.05, &SceneInputState::default()).unwrap();
    let pf_red = r1.pixel_bgra.expect("frame 1 RED pixel_bgra");
    assert_eq!(&pf_red.bgra[0..4], &[0, 0, 255, 255]);
    assert!(pf_red.revision > pf_blue0.revision);

    let mut wrap_blue = None;
    for _ in 0..120 {
        let out = vm_res.step_frame(0.05, &SceneInputState::default()).unwrap();
        if out.loop_count == 1 {
            wrap_blue = out.pixel_bgra;
            break;
        }
    }
    let pf_blue_wrap = wrap_blue.expect("loop wrap restoring cp0 BLUE buffer must emit dirty pixel_bgra");
    assert_eq!(&pf_blue_wrap.bgra[0..4], &[255, 40, 0, 255]);
    assert!(
        pf_blue_wrap.revision > pf_red.revision,
        "loop wrap restoring cp0 BLUE must strictly exceed prior RED revision ({} > {})",
        pf_blue_wrap.revision,
        pf_red.revision
    );

    // Verify seek_video determinism (including bit-exact PixelFrame.revision) stays 100% intact after loop wrap!
    let seek_after_wrap = vm_res.seek_video(1.0).unwrap();
    let mut vm_fresh = VmInstance::new(v_res, VmBudgets::default()).unwrap();
    let seek_fresh = vm_fresh.seek_video(1.0).unwrap();
    assert_eq!(seek_after_wrap.draw_cmds, seek_fresh.draw_cmds);
    assert_eq!(seek_after_wrap.pixel_bgra, seek_fresh.pixel_bgra);
}

