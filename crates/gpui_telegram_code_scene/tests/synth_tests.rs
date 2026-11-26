//! Comprehensive integration tests for Dual-Engine AI Code Scene Synthesizer (`CS-F08`).

use std::collections::HashSet;

use gpui_telegram_code_scene::lang::stdlib::BuiltinId;
use gpui_telegram_code_scene::{
    ColorPaletteTheme, CompileDiagnostic, DeterministicProceduralSynthesizer, DiagnosticStage,
    FEW_SHOT_ANIMATION_CS1, FEW_SHOT_GAME_CS1, FEW_SHOT_VIDEO_CS1, IntentAnalyzer, LlmParseError,
    LlmPromptBuilder, LlmResponseParser, SceneArchetype, SceneInputState, SynthRequest, VmBudgets,
    VmInstance, compile_and_verify, official_seed_assets, validate_to_report,
};
use gpui_telegram_proto::{AssetKind, AssetManifest};

#[test]
fn test_bilingual_intent_analyzer_create_and_remix_and_injection_safety() {
    let pinball = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "赛博朋克霓虹弹球游戏".to_string(),
        ..Default::default()
    });
    assert_eq!(pinball.kind, AssetKind::Game);
    assert_eq!(pinball.archetype, SceneArchetype::GameNeonPinballBreakout);
    assert_eq!(pinball.palette, ColorPaletteTheme::CyberpunkNeon);
    assert!(pinball.is_chinese);

    let purple_fluid = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "紫色流体动画".to_string(),
        ..Default::default()
    });
    assert_eq!(purple_fluid.kind, AssetKind::Animation);
    assert_eq!(purple_fluid.archetype, SceneArchetype::AnimFluidPlasma);
    assert_eq!(purple_fluid.palette, ColorPaletteTheme::AuroraPurple);

    let star_video = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "星空穿梭代码视频 20秒".to_string(),
        ..Default::default()
    });
    assert_eq!(star_video.kind, AssetKind::Video);
    assert_eq!(star_video.archetype, SceneArchetype::VideoStarWarpReel);
    assert_eq!(star_video.duration_sec, Some(20.0));

    let shooter = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "retro space shooter game".to_string(),
        ..Default::default()
    });
    assert_eq!(shooter.kind, AssetKind::Game);
    assert_eq!(shooter.archetype, SceneArchetype::GameSpaceShooter);

    let aurora = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "neon particle aurora".to_string(),
        ..Default::default()
    });
    assert_eq!(aurora.kind, AssetKind::Animation);

    // Deterministic seed reproducibility
    let s1 = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "neon space shooter".to_string(),
        seed: Some(77),
        ..Default::default()
    });
    let s2 = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "neon space shooter".to_string(),
        seed: Some(77),
        ..Default::default()
    });
    assert_eq!(s1.seed, s2.seed);

    // Remix modifiers (zh-CN & en)
    let r_fast = IntentAnalyzer::analyze_remix("加快速度", AssetKind::Game);
    assert!(r_fast.speed_mul > 1.2 && r_fast.speed_explicit);

    let r_switch = IntentAnalyzer::analyze_remix("改成紫色流体动画", AssetKind::Game);
    assert_eq!(r_switch.kind_switch, Some(AssetKind::Animation));
    assert_eq!(r_switch.archetype_switch, Some(SceneArchetype::AnimFluidPlasma));
    assert_eq!(r_switch.palette_override, Some(ColorPaletteTheme::AuroraPurple));

    let r_obs = IntentAnalyzer::analyze_remix("增加障碍物和粒子特效", AssetKind::Game);
    assert!(r_obs.obstacle_delta > 0 && r_obs.particle_boost > 0.0);

    let r_en = IntentAnalyzer::analyze_remix(
        "make it faster with golden neon and more obstacles",
        AssetKind::Game,
    );
    assert!(r_en.speed_mul > 1.2);
    assert_eq!(r_en.palette_override, Some(ColorPaletteTheme::SolarGold));
    assert!(r_en.obstacle_delta > 0);

    // Adversarial injection & empty prompts still synthesize valid cs1!
    let synth = DeterministicProceduralSynthesizer::new();
    for evil in [
        "",
        "   ",
        "\"; fn hack() { while true {} } //",
        &"赛博霓虹".repeat(300),
    ] {
        let draft = synth.synthesize_prompt(evil, None);
        assert!(
            draft.validation.ok,
            "adversarial prompt failed validation: {:?}",
            draft.validation.diagnostics
        );
    }
}

#[test]
fn test_all_15_archetypes_compile_verify_distinct_covers_and_runtime_modes() {
    let synth = DeterministicProceduralSynthesizer::new();
    let mut cover_hashes = HashSet::new();

    for (idx, arch) in SceneArchetype::ALL.into_iter().enumerate() {
        let mut intent = IntentAnalyzer::analyze_create(&SynthRequest {
            prompt: format!("test archetype {}", arch.as_str()),
            kind: Some(arch.kind()),
            seed: Some(100 + idx as u64),
            ..Default::default()
        });
        intent.archetype = arch;
        intent.kind = arch.kind();
        intent.duration_sec = arch.default_duration_sec();

        let draft = synth.synthesize_from_intent(&intent);
        assert!(
            draft.validation.ok,
            "archetype {:?} failed validation: {:?}",
            arch, draft.validation.diagnostics
        );
        assert!(
            draft.validation.peak_ops_used < 50_000,
            "archetype {:?} peak_ops_used={} >= 50_000",
            arch, draft.validation.peak_ops_used
        );
        let cover = draft
            .validation
            .cover_hash
            .as_ref()
            .expect("cover_hash missing");
        assert_eq!(cover.len(), 64);
        assert!(
            cover_hashes.insert(cover.clone()),
            "duplicate cover_hash for archetype {:?}",
            arch
        );

        let verified = compile_and_verify(&draft.source_code, &draft.manifest)
            .unwrap_or_else(|e| panic!("archetype {:?} compile_and_verify failed: {:?}", arch, e));
        let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();

        // Execute 18 frames verifying clean runtime execution and non-empty visuals
        for f in 0..18 {
            let input = SceneInputState {
                pointer_active: true,
                mouse_x: 120.0 + (f as f64) * 6.0,
                mouse_y: 260.0 + (f as f64) * 5.0,
                clicked: f == 4,
                action: f == 4,
                ..Default::default()
            };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            assert!(
                !out.draw_cmds.is_empty() || out.pixel_bgra.is_some(),
                "archetype {:?} frame {} had no visual output",
                arch, f
            );
        }

        // All 5 Videos: deterministic O(1) seek_video(dur * 0.5) bit-identical
        if arch.kind() == AssetKind::Video {
            let dur = draft.manifest.duration_sec.unwrap();
            let mid = dur * 0.5;
            let f_a = vm.seek_video(mid).unwrap();
            let _ = vm.seek_video(dur * 0.9).unwrap();
            let f_b = vm.seek_video(mid).unwrap();
            assert_eq!(f_a.draw_cmds.len(), f_b.draw_cmds.len());
            assert_eq!(
                f_a.pixel_bgra.as_ref().map(|p| &p.bgra),
                f_b.pixel_bgra.as_ref().map(|p| &p.bgra),
                "Video {:?} seek_video was not bit-identical!",
                arch
            );
        }
    }
    assert_eq!(cover_hashes.len(), 15);
}

#[test]
fn test_all_5_synthesized_games_score_gameover_and_restart_high_score() {
    let synth = DeterministicProceduralSynthesizer::new();
    let game_archetypes = [
        SceneArchetype::GameNeonPinballBreakout,
        SceneArchetype::GameSpaceShooter,
        SceneArchetype::GameCyberRunnerDodge,
        SceneArchetype::GameFlappyCyber,
        SceneArchetype::GameNeonSnake,
    ];

    for arch in game_archetypes {
        let mut intent = IntentAnalyzer::analyze_create(&SynthRequest {
            prompt: format!("play {}", arch.as_str()),
            kind: Some(AssetKind::Game),
            seed: Some(42),
            ..Default::default()
        });
        intent.archetype = arch;
        intent.kind = AssetKind::Game;
        let draft = synth.synthesize_from_intent(&intent);
        assert!(draft.validation.ok, "{:?} invalid", arch);

        let verified = compile_and_verify(&draft.source_code, &draft.manifest).unwrap();
        let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();

        // Phase 1: Play actively to score > 0
        let mut best_score = 0_i64;
        for f in 0..180 {
            let mut input = SceneInputState::default();
            match arch {
                SceneArchetype::GameNeonPinballBreakout => {
                    input.pointer_active = true;
                    input.mouse_x = 180.0;
                }
                SceneArchetype::GameSpaceShooter => {
                    input.action = f % 4 == 0;
                }
                SceneArchetype::GameCyberRunnerDodge => {
                    // Stay centered to collect first energy orb straight ahead
                    input.pointer_active = true;
                    input.mouse_x = 180.0;
                }
                SceneArchetype::GameFlappyCyber => {
                    // Steady flap rhythm (~0.67s) matching gravity/impulse equilibrium
                    input.action = f % 40 == 1;
                }
                SceneArchetype::GameNeonSnake => {
                    // Keep heading right into food at (12, 14)
                    input.right = f == 0;
                }
                _ => {}
            }
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            if let Some(hud) = out.game_hud {
                best_score = best_score.max(hud.score);
                if best_score > 0 {
                    break;
                }
            }
        }
        assert!(
            best_score > 0,
            "Game {:?} failed to score > 0 under active play",
            arch
        );

        // Phase 2: Play passively / steer into hazard until game_over == true
        let mut hit_game_over = false;
        let mut recorded_high = 0_i64;
        for _ in 0..700 {
            let mut input = SceneInputState::default();
            if arch == SceneArchetype::GameNeonPinballBreakout {
                // Move paddle to far edge so ball misses
                input.left = true;
            }
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            if let Some(hud) = out.game_hud {
                recorded_high = recorded_high.max(hud.high_score);
                if hud.game_over {
                    hit_game_over = true;
                    break;
                }
            }
        }
        assert!(
            hit_game_over,
            "Game {:?} never reached game_over within 700 frames",
            arch
        );
        assert!(recorded_high >= best_score);

        // Phase 3: restart() resets score == 0, game_over == false, preserves high_score!
        vm.restart().unwrap();
        let after = vm
            .step_frame(1.0 / 60.0, &SceneInputState::default())
            .unwrap();
        let hud = after.game_hud.expect("game_hud missing after restart");
        assert_eq!(hud.score, 0, "Game {:?} score not reset on restart", arch);
        assert!(!hud.game_over, "Game {:?} game_over not cleared on restart", arch);
        assert!(
            hud.high_score >= best_score,
            "Game {:?} lost high_score on restart ({} < {})",
            arch, hud.high_score, best_score
        );
    }
}

#[test]
fn test_remix_source_all_3_cases_and_all_18_official_seeds() {
    let synth = DeterministicProceduralSynthesizer::new();

    // Case 2: @cs1-params single & multi-turn chained remix
    let base = synth.synthesize_prompt("赛博朋克霓虹弹球游戏", Some(AssetKind::Game));
    assert!(base.validation.ok);
    let r1 = synth.remix_source(
        &base.source_code,
        &base.manifest,
        "加快速度并增加金色粒子特效",
        Some(&base.title),
        &base.tags,
    );
    assert!(r1.validation.ok, "Case 2 remix 1 failed: {:?}", r1.validation.diagnostics);
    assert_ne!(r1.source_code, base.source_code);
    assert_ne!(r1.validation.cover_hash, base.validation.cover_hash);

    let r2 = synth.remix_source(
        &r1.source_code,
        &r1.manifest,
        "增加障碍物，再酷炫一点",
        Some(&r1.title),
        &r1.tags,
    );
    assert!(r2.validation.ok, "Case 2 chained remix 2 failed: {:?}", r2.validation.diagnostics);
    assert_ne!(r2.source_code, r1.source_code);

    // Case 1: Explicit modality switch (Game -> Purple Fluid Animation)
    let switched = synth.remix_source(
        &base.source_code,
        &base.manifest,
        "改成紫色流体动画",
        Some(&base.title),
        &base.tags,
    );
    assert!(switched.validation.ok);
    assert_eq!(switched.kind, AssetKind::Animation);
    assert_eq!(switched.archetype, SceneArchetype::AnimFluidPlasma);
    assert_eq!(switched.palette, ColorPaletteTheme::AuroraPurple);

    // Case 3: Arbitrary seed remix across ALL 18 official M1 seeds!
    for seed in official_seed_assets() {
        let orig_report = validate_to_report(&seed.source_code, &seed.manifest);
        assert!(orig_report.ok);

        let remixed = synth.remix_source(
            &seed.source_code,
            &seed.manifest,
            "加快速度并增加金色粒子特效和障碍物",
            Some(&seed.title),
            &seed.tags,
        );
        assert!(
            remixed.validation.ok,
            "seed {} remix failed: {:?}",
            seed.scene_id, remixed.validation.diagnostics
        );
        assert_eq!(remixed.kind, seed.kind);
        assert_ne!(
            remixed.validation.cover_hash, orig_report.cover_hash,
            "seed {} cover_hash did not change on remix",
            seed.scene_id
        );

        // Chained remix on an already-wrapped seed stays idempotent (Case 2 on @cs1-remix-params)
        let remixed2 = synth.remix_source(
            &remixed.source_code,
            &remixed.manifest,
            "换成紫色霓虹",
            Some(&remixed.title),
            &remixed.tags,
        );
        assert!(remixed2.validation.ok, "seed {} 2nd remix failed", seed.scene_id);

        // Video seeds MUST preserve deterministic O(1) seek_video!
        if seed.kind == AssetKind::Video {
            let verified = compile_and_verify(&remixed.source_code, &remixed.manifest).unwrap();
            assert!(
                !verified.compiled.has_function("on_update"),
                "remixed Video {} must NOT inject on_update!",
                seed.scene_id
            );
            let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
            let mid = remixed.manifest.duration_sec.unwrap() * 0.5;
            let a = vm.seek_video(mid).unwrap();
            let _ = vm.seek_video(1.0).unwrap();
            let b = vm.seek_video(mid).unwrap();
            assert_eq!(a.draw_cmds.len(), b.draw_cmds.len());
        }
    }

    // Broken input source remix falls back cleanly to a valid procedural draft
    let broken = synth.remix_source(
        "fn broken( {{{ not valid cs1",
        &AssetManifest::for_kind(AssetKind::Animation),
        "霓虹极光粒子",
        Some("Broken"),
        &[],
    );
    assert!(broken.validation.ok);
}

#[test]
fn test_llm_system_prompt_covers_all_68_builtins_and_verified_few_shots() {
    let sys = LlmPromptBuilder::build_system_prompt();
    assert_eq!(BuiltinId::ALL.len(), 68);
    for builtin in BuiltinId::ALL {
        let name = builtin.canonical_name();
        assert!(
            sys.contains(name),
            "LlmPromptBuilder::build_system_prompt() is missing builtin '{name}'"
        );
    }

    // Verify all 3 few-shot examples pass real compile_and_verify + 10 frames
    let few_shots = [
        ("video", AssetKind::Video, FEW_SHOT_VIDEO_CS1, Some(12.0)),
        ("animation", AssetKind::Animation, FEW_SHOT_ANIMATION_CS1, None),
        ("game", AssetKind::Game, FEW_SHOT_GAME_CS1, None),
    ];
    for (label, kind, src, dur) in few_shots {
        let mut manifest = AssetManifest::for_kind(kind);
        manifest.duration_sec = dur;
        let verified = compile_and_verify(src, &manifest)
            .unwrap_or_else(|e| panic!("few-shot {label} failed compile_and_verify: {e:?}"));
        let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
        for f in 0..10 {
            let input = SceneInputState {
                pointer_active: true,
                mouse_x: 180.0,
                mouse_y: 300.0,
                clicked: f == 2,
                action: f == 2,
                ..Default::default()
            };
            let out = vm.step_frame(1.0 / 60.0, &input).unwrap();
            assert!(!out.draw_cmds.is_empty() || out.pixel_bgra.is_some());
        }
    }

    // Create, remix, and repair prompts
    let create_msg = LlmPromptBuilder::build_create_prompt(
        "霓虹太空射击游戏",
        Some(AssetKind::Game),
        &["neon".to_string(), "arcade".to_string()],
        Some(99),
    );
    assert!(create_msg.user.contains("霓虹太空射击游戏"));
    assert!(create_msg.user.contains("game"));

    let remix_msg = LlmPromptBuilder::build_remix_prompt(
        FEW_SHOT_ANIMATION_CS1,
        &AssetManifest::for_kind(AssetKind::Animation),
        Some("Aurora"),
        "make it faster",
    );
    assert!(remix_msg.user.contains("make it faster"));
    assert!(remix_msg.user.contains("shader_plasma"));

    // Repair prompt from CompileDiagnostic AND ValidationReportDto
    let bad_src = "fn on_init(state) {\n    let x = 1;\n}\nfn helper() {\n    bad_call(1, 2);\n}\n";
    let diag = CompileDiagnostic::new(
        DiagnosticStage::Compile,
        "E_MISSING_ON_RENDER",
        "missing required function `on_render`",
        Some(gpui_telegram_code_scene::Span { line: 4, col: 5 }),
    );
    let rep_from_diag = LlmPromptBuilder::build_repair_from_diagnostic(
        "make neon art",
        bad_src,
        &AssetManifest::for_kind(AssetKind::Animation),
        &diag,
        1,
    );
    assert!(rep_from_diag.user.contains("E_MISSING_ON_RENDER"));
    assert!(rep_from_diag.user.contains(">>>"));
    assert!(rep_from_diag.user.contains("attempt 1 of 2"));

    let val_report = validate_to_report(bad_src, &AssetManifest::for_kind(AssetKind::Animation));
    assert!(!val_report.ok);
    let rep_from_val = LlmPromptBuilder::build_repair_from_report(
        "make neon art",
        bad_src,
        &AssetManifest::for_kind(AssetKind::Animation),
        &val_report,
        2,
    );
    assert!(rep_from_val.user.contains("attempt 2 of 2"));
    assert!(rep_from_val.user.contains("Diagnostics"));
}

#[test]
fn test_llm_response_parser_resilient_extraction_cascade() {
    let valid_code = FEW_SHOT_ANIMATION_CS1;

    // 1. Direct JSON
    let direct_json = serde_json::json!({
        "title": "Direct Aurora",
        "description": "Neon shader",
        "kind": "animation",
        "tags": ["shader", "aurora"],
        "controls_hint": "Move mouse",
        "source_code": valid_code
    })
    .to_string();
    let p1 = LlmResponseParser::parse(&direct_json).unwrap();
    assert_eq!(p1.extraction_mode, "json_direct");
    assert_eq!(p1.title.as_deref(), Some("Direct Aurora"));
    assert_eq!(p1.kind, Some(AssetKind::Animation));
    let (draft1, ok1) = p1.into_validated_draft("aurora", Some(AssetKind::Animation), "live_llm");
    assert!(ok1 && draft1.validation.ok);

    // 2. Fenced JSON wrapped in <think>...</think> + surrounding chatter
    let fenced_think = format!(
        "<think>\nMaybe try fn on_render(s, t, c) {{ broken }}\n</think>\n\
         Sure! Here is your scene:\n\
         ```json\n{}\n```\nEnjoy!",
        direct_json
    );
    let p2 = LlmResponseParser::parse(&fenced_think).unwrap();
    assert_eq!(p2.extraction_mode, "json_fenced");
    assert!(p2.source_code.contains("shader_plasma"));

    // 3. JSON with unescaped raw newlines inside source_code
    let raw_nl_json = "{\n  \"title\": \"Newline Safe\",\n  \"kind\": \"animation\",\n  \"source_code\": \"fn on_render(s, t, c) {\n    clear(0x0A0A1EFF);\n    circle(180, 320, 40, 0x00F5FFFF);\n}\"\n}";
    let p3 = LlmResponseParser::parse(raw_nl_json).unwrap();
    assert_eq!(p3.title.as_deref(), Some("Newline Safe"));
    let (_, ok3) = p3.into_validated_draft("circle", Some(AssetKind::Animation), "live_llm");
    assert!(ok3);

    // 4. Hybrid JSON metadata + separate ```cs1 code fence
    let hybrid = format!(
        "```json\n{{\"title\":\"Hybrid Scene\",\"kind\":\"video\",\"tags\":[\"neon\"]}}\n```\n\
         ```cs1\n{FEW_SHOT_VIDEO_CS1}\n```"
    );
    let p4 = LlmResponseParser::parse(&hybrid).unwrap();
    assert_eq!(p4.extraction_mode, "hybrid_json_and_fence");
    assert_eq!(p4.title.as_deref(), Some("Hybrid Scene"));
    assert_eq!(p4.kind, Some(AssetKind::Video));
    let (_, ok4) = p4.into_validated_draft("neon video", None, "live_llm");
    assert!(ok4);

    // 5. Multiple fences: skips non-compilable pseudo-snippet and selects compilable on_render
    let multi = format!(
        "```js\nfunction fake() {{ const x = 1; }}\n```\n\
         ```cs1\n{FEW_SHOT_GAME_CS1}\n```"
    );
    let p5 = LlmResponseParser::parse(&multi).unwrap();
    assert_eq!(p5.extraction_mode, "code_fence");
    assert!(p5.source_code.contains("NEON CATCH"));

    // 6. Unclosed trailing fence truncated at EOF
    let unclosed = format!("Here is the code:\n```cs1\n{FEW_SHOT_ANIMATION_CS1}");
    let p6 = LlmResponseParser::parse(&unclosed).unwrap();
    assert_eq!(p6.extraction_mode, "code_fence");
    let (_, ok6) = p6.into_validated_draft("aurora", Some(AssetKind::Animation), "live_llm");
    assert!(ok6);

    // 7. Raw unfenced cs1 with leading & trailing prose
    let raw_prose = format!(
        "Sure, here is a clean cs1 animation:\n\n{FEW_SHOT_ANIMATION_CS1}\n\nHope this helps you build awesome scenes!"
    );
    let p7 = LlmResponseParser::parse(&raw_prose).unwrap();
    assert_eq!(p7.extraction_mode, "raw_cs1");
    assert!(!p7.source_code.contains("Hope this helps"));
    let (_, ok7) = p7.into_validated_draft("aurora", Some(AssetKind::Animation), "live_llm");
    assert!(ok7);

    // 8. Error cases: empty & non-code prose
    assert!(matches!(LlmResponseParser::parse("   \n\t "), Err(LlmParseError::EmptyResponse)));
    assert!(matches!(
        LlmResponseParser::parse("I am sorry, I cannot write code right now."),
        Err(LlmParseError::NoScriptFound { .. })
    ));
}

#[test]
fn test_iter2_remix_switch_clauses_palette_safety_pinball_cap_and_nan_modifiers() {
    let synth = DeterministicProceduralSynthesizer::new();

    // Item 14a: All 12 natural full-sentence zh/en modality switch directions + dedicated subject switches
    let video_base = synth.synthesize_prompt("星空穿梭代码视频", Some(AssetKind::Video));
    let anim_base = synth.synthesize_prompt("紫色流体动画", Some(AssetKind::Animation));
    let game_base = synth.synthesize_prompt("赛博朋克霓虹弹球游戏", Some(AssetKind::Game));

    let switch_cases: [(&gpui_telegram_code_scene::SynthesizedDraft, &str, AssetKind); 12] = [
        (&video_base, "把这个视频改成互动动画", AssetKind::Animation),
        (&video_base, "convert this video into an animation", AssetKind::Animation),
        (&video_base, "把这个视频改成游戏", AssetKind::Game),
        (&video_base, "turn this video into a game", AssetKind::Game),
        (&anim_base, "把这个动画改成代码视频", AssetKind::Video),
        (&anim_base, "turn this animation into a video", AssetKind::Video),
        (&anim_base, "把这个动画改成小游戏", AssetKind::Game),
        (&anim_base, "convert this animation into a game", AssetKind::Game),
        (&game_base, "把这个游戏改成代码视频", AssetKind::Video),
        (&game_base, "turn this game into a video", AssetKind::Video),
        (&game_base, "把这个游戏改成流体动画", AssetKind::Animation),
        (&game_base, "turn this game into an animation", AssetKind::Animation),
    ];
    for (src_draft, instr, expected_kind) in switch_cases {
        let out = synth.remix_source(
            &src_draft.source_code,
            &src_draft.manifest,
            instr,
            Some(&src_draft.title),
            &src_draft.tags,
        );
        assert!(out.validation.ok, "switch '{instr}' failed validation");
        assert_eq!(
            out.kind, expected_kind,
            "switch '{instr}' failed to switch from {:?} to {:?} (got {:?})",
            src_draft.kind, expected_kind, out.kind
        );
    }

    let snake_sw = IntentAnalyzer::analyze_remix("改成贪吃蛇", AssetKind::Video);
    assert_eq!(snake_sw.archetype_switch, Some(SceneArchetype::GameNeonSnake));
    assert_eq!(snake_sw.kind_switch, Some(AssetKind::Game));
    let flappy_sw = IntentAnalyzer::analyze_remix("switch to flappy bird", AssetKind::Animation);
    assert_eq!(flappy_sw.archetype_switch, Some(SceneArchetype::GameFlappyCyber));
    assert_eq!(flappy_sw.kind_switch, Some(AssetKind::Game));

    // Item 14b: Palette-only / style-only / modifier-only remixes MUST NOT hijack Video or Game modality
    let palette_prompts = [
        "放慢速度并换成深海青蓝",
        "加快速度并换成金色霓虹增加粒子",
        "换成深海青蓝",
        "换成极光紫",
        "换成落日合成波",
        "换成极简冰霜",
        "换成合成波配色",
        "改成深海风格",
        "change to matrix green",
        "switch to aurora purple",
        "switch to sunset synthwave",
        "switch to ocean teal",
        "change to frost minimal",
    ];
    for prompt in palette_prompts {
        let mods_v = IntentAnalyzer::analyze_remix(prompt, AssetKind::Video);
        assert!(
            mods_v.kind_switch.is_none() && mods_v.archetype_switch.is_none(),
            "palette prompt '{prompt}' hijacked Video: kind={:?} arch={:?}",
            mods_v.kind_switch, mods_v.archetype_switch
        );
        let r_v = synth.remix_source(
            &video_base.source_code,
            &video_base.manifest,
            prompt,
            Some(&video_base.title),
            &video_base.tags,
        );
        assert!(r_v.validation.ok, "Video palette remix '{prompt}' invalid");
        assert_eq!(r_v.kind, AssetKind::Video, "Video hijacked on '{prompt}'");
        assert!(
            !r_v.source_code.contains("fn on_update"),
            "Video palette remix '{prompt}' injected on_update"
        );

        let r_g = synth.remix_source(
            &game_base.source_code,
            &game_base.manifest,
            prompt,
            Some(&game_base.title),
            &game_base.tags,
        );
        assert!(r_g.validation.ok, "Game palette remix '{prompt}' invalid");
        assert_eq!(r_g.kind, AssetKind::Game, "Game hijacked on '{prompt}'");
    }

    // Item 15: Sustained input.action hold in GameNeonPinballBreakout must keep velocity bounded (no tunneling)
    let verified_pinball = compile_and_verify(&game_base.source_code, &game_base.manifest).unwrap();
    let mut vm = VmInstance::new(verified_pinball, VmBudgets::default()).unwrap();
    let mut prev_ball_y: Option<f32> = None;
    let mut max_dy = 0.0_f32;
    let mut ball_samples = 0usize;
    for _ in 0..320 {
        let out = vm
            .step_frame(
                1.0 / 60.0,
                &SceneInputState {
                    action: true,
                    pointer_active: true,
                    mouse_x: 180.0,
                    ..Default::default()
                },
            )
            .unwrap();
        // Track white ball circle center (`radius == 7.0`) to confirm bounded per-frame displacement
        for cmd in &out.draw_cmds {
            if let gpui_telegram_code_scene::DrawCmd::FillCircle { cy, radius, .. } = cmd {
                if (*radius - 7.0).abs() < 0.25 {
                    ball_samples += 1;
                    if let Some(py) = prev_ball_y {
                        let dy = (*cy - py).abs();
                        if dy < 120.0 {
                            max_dy = max_dy.max(dy);
                        }
                    }
                    prev_ball_y = Some(*cy);
                }
            }
        }
    }
    assert!(ball_samples >= 100, "failed to observe pinball trajectory");
    assert!(
        max_dy < 16.0,
        "GameNeonPinballBreakout ball velocity exploded under sustained action hold: max_dy={max_dy:.2}"
    );

    // Item 16: Non-finite NaN/Inf modifiers in SceneIntentSpec must sanitize cleanly to valid cs1
    let mut evil_intent = IntentAnalyzer::analyze_create(&SynthRequest {
        prompt: "星空穿梭代码视频".to_string(),
        kind: Some(AssetKind::Video),
        ..Default::default()
    });
    evil_intent.modifiers.speed_mul = f64::NAN;
    evil_intent.modifiers.hue_override = Some(f64::INFINITY);
    evil_intent.modifiers.density_delta = f64::NEG_INFINITY;
    evil_intent.modifiers.difficulty_delta = f64::NAN;
    evil_intent.modifiers.particle_boost = f64::INFINITY;
    evil_intent.modifiers.glow_boost = f64::NAN;
    evil_intent.duration_sec = Some(f64::NAN);
    let safe_draft = synth.synthesize_from_intent(&evil_intent);
    assert!(
        safe_draft.validation.ok,
        "NaN/Inf intent modifiers produced invalid cs1: {:?}",
        safe_draft.validation.diagnostics
    );
    assert!(!safe_draft.source_code.contains("NaN"));
    assert!(!safe_draft.source_code.contains("inf"));
}

#[test]
fn test_iter2_repair_excerpt_parser_edge_cases_and_case2_case3_remix_hardening() {
    // Item 17: Repair excerpt must never be blank for > 24-line source with line == Some(0)
    let mut long_cjk = String::from("fn on_init(state) {\n    // 初始化中文场景\n");
    for i in 0..32 {
        long_cjk.push_str(&format!("    let v_{i} = {i};\n"));
    }
    long_cjk.push_str("}\nfn on_render(state, t, canvas) {\n    clear(0x000000FF);\n}\n");
    let diag0 = CompileDiagnostic::new(
        DiagnosticStage::Compile,
        "E_UNDEFINED_VAR",
        "synthetic zero-line diagnostic",
        Some(gpui_telegram_code_scene::Span { line: 0, col: 0 }),
    );
    let rep0 = LlmPromptBuilder::build_repair_from_diagnostic(
        "修复场景",
        &long_cjk,
        &AssetManifest::for_kind(AssetKind::Animation),
        &diag0,
        1,
    );
    assert!(!rep0.user.contains("```cs1\n\n```"), "repair excerpt was blank for line=0");
    assert!(rep0.user.contains("   1 | fn on_init(state)"));

    // Item 18a: Trailing prose containing `{color}` is NOT swallowed into raw unfenced cs1
    let raw_with_brace_prose = format!(
        "好的：\n\n{FEW_SHOT_VIDEO_CS1}\n\n提示：你可以修改 {{color}} 参数来调整配色！"
    );
    let parsed_brace = LlmResponseParser::parse(&raw_with_brace_prose).unwrap();
    assert_eq!(parsed_brace.extraction_mode, "raw_cs1");
    assert!(!parsed_brace.source_code.contains("{color}"));
    let (draft_brace, ok_brace) =
        parsed_brace.into_validated_draft("neon video", Some(AssetKind::Video), "live_llm");
    assert!(ok_brace && draft_brace.validation.ok);

    // Item 18b: Multi-line unclosed <think> scratchpad returns Err(NoScriptFound) (never leaks!)
    let unclosed_think = "<think>\nDraft attempt:\nfn on_render(state, t, canvas) {\n    broken syntax !@#\n}";
    assert!(matches!(
        LlmResponseParser::parse(unclosed_think),
        Err(LlmParseError::NoScriptFound { .. })
    ));
    let before_unclosed_think = format!(
        "```cs1\n{FEW_SHOT_VIDEO_CS1}\n```\n<think>\nlate trailing thought without close"
    );
    let parsed_before = LlmResponseParser::parse(&before_unclosed_think).unwrap();
    assert!(parsed_before.source_code.contains("NEON PULSE REEL"));

    // Item 18c: Hybrid JSON with placeholder "source_code": "see below" preserves title/kind/tags
    let hybrid_placeholder = format!(
        "```json\n{{\"title\":\"Placeholder Meta\",\"kind\":\"game\",\"tags\":[\"arcade\",\"neon\"],\"source_code\":\"see below\"}}\n```\n\
         ```cs1\n{FEW_SHOT_GAME_CS1}\n```"
    );
    let parsed_hybrid = LlmResponseParser::parse(&hybrid_placeholder).unwrap();
    assert_eq!(parsed_hybrid.extraction_mode, "hybrid_json_and_fence");
    assert_eq!(parsed_hybrid.title.as_deref(), Some("Placeholder Meta"));
    assert_eq!(parsed_hybrid.kind, Some(AssetKind::Game));
    assert_eq!(parsed_hybrid.tags, vec!["arcade".to_string(), "neon".to_string()]);

    // Item 19a: Decoy "// @cs1-params fake" inside string literal must fall through to Case 3 and modify output
    let synth = DeterministicProceduralSynthesizer::new();
    let manifest_anim = AssetManifest::for_kind(AssetKind::Animation);
    let spoof_src = r#"fn on_render(state, t, canvas) {
    clear(0x0A0A20FF);
    text(180, 320, "// @cs1-params fake", 16, 0x00F5FFFF, "center");
    circle(180, 260, 36, 0xFF00AAFF);
}"#;
    let orig_spoof_rep = validate_to_report(spoof_src, &manifest_anim);
    assert!(orig_spoof_rep.ok);
    let remixed_spoof = synth.remix_source(
        spoof_src,
        &manifest_anim,
        "加快速度并增加粒子",
        Some("Spoof"),
        &[],
    );
    assert!(remixed_spoof.validation.ok);
    assert_ne!(remixed_spoof.source_code, spoof_src);
    assert_ne!(remixed_spoof.validation.cover_hash, orig_spoof_rep.cover_hash);
    assert!(remixed_spoof.source_code.contains("__cs1_orig_render"));

    // Item 19b: Comment between `fn` and `on_render` renames cleanly in Case 3 without wiping script
    let comment_fn_src = r#"fn /* inline comment */ on_render(state, t, canvas) {
    clear(0x081024FF);
    circle(180, 320, 44, 0x44FFCCFF);
    text(180, 120, "KEEP_MY_SCRIPT", 16, 0xFFFFFFFF, "center");
}"#;
    let remixed_comment = synth.remix_source(
        comment_fn_src,
        &manifest_anim,
        "加快速度",
        Some("CommentFn"),
        &[],
    );
    assert!(remixed_comment.validation.ok);
    assert!(
        remixed_comment.source_code.contains("KEEP_MY_SCRIPT"),
        "fn /* comment */ on_render was wiped instead of wrapped!"
    );

    // Item 19c: User script defining `let __CS1_RMX_SPEED = 2.0;` remixes cleanly across 2 chained turns
    let collision_src = r#"let __CS1_RMX_SPEED = 2.0;
fn on_render(state, t, canvas) {
    clear(0x0C0824FF);
    circle(180, 320, 22 * __CS1_RMX_SPEED, 0x00F5FFFF);
    text(180, 140, "KEEP_GLOBAL_USER", 16, 0xFFFFFFFF, "center");
}"#;
    let remixed_col1 = synth.remix_source(
        collision_src,
        &manifest_anim,
        "加快速度并换成金色",
        Some("GlobalCol"),
        &[],
    );
    assert!(remixed_col1.validation.ok, "Case 3 global collision failed: {:?}", remixed_col1.validation.diagnostics);
    assert!(remixed_col1.source_code.contains("KEEP_GLOBAL_USER"));
    assert!(remixed_col1.source_code.contains("__CS1_RMX_1_SPEED"));

    let remixed_col2 = synth.remix_source(
        &remixed_col1.source_code,
        &remixed_col1.manifest,
        "放慢速度并换成极光紫",
        Some(&remixed_col1.title),
        &remixed_col1.tags,
    );
    assert!(remixed_col2.validation.ok, "Chained Case 2 global collision failed: {:?}", remixed_col2.validation.diagnostics);
    assert!(remixed_col2.source_code.contains("KEEP_GLOBAL_USER"));
    assert_ne!(remixed_col2.source_code, remixed_col1.source_code);
}

#[test]
fn test_iter3_compound_switches_same_modality_preserve_raw_map_globals_and_bom_case3() {
    let synth = DeterministicProceduralSynthesizer::new();
    let video_base = synth.synthesize_prompt("星空穿梭代码视频", Some(AssetKind::Video));
    let anim_base = synth.synthesize_prompt("曼陀罗分形动画", Some(AssetKind::Animation));
    let game_base = synth.synthesize_prompt("赛博朋克霓虹弹球游戏", Some(AssetKind::Game));

    // Fix 5 (C2-ITER2-BUG-1): Compound two-cue prompts (color-before-switch, switch-before-color, negation)
    let compound_cases: [(&gpui_telegram_code_scene::SynthesizedDraft, &str, AssetKind); 8] = [
        (&game_base, "换成金色并把这个游戏改成代码视频", AssetKind::Video),
        (&video_base, "换成紫色，把这个视频改成互动动画", AssetKind::Animation),
        (&game_base, "change color to purple and turn this game into a video", AssetKind::Video),
        (&video_base, "switch colors to gold and convert this video into an animation", AssetKind::Animation),
        (&game_base, "不要改成游戏，把它变成视频", AssetKind::Video),
        (&game_base, "把这个游戏改成代码视频并换成金色", AssetKind::Video),
        (&game_base, "turn this game into a video and change colors to gold", AssetKind::Video),
        (&game_base, "make it purple and turn this game into a video", AssetKind::Video),
    ];
    for (src_draft, instr, expected_kind) in compound_cases {
        let out = synth.remix_source(
            &src_draft.source_code,
            &src_draft.manifest,
            instr,
            Some(&src_draft.title),
            &src_draft.tags,
        );
        assert!(out.validation.ok, "compound switch '{instr}' invalid: {:?}", out.validation.diagnostics);
        assert_eq!(
            out.kind, expected_kind,
            "compound switch '{instr}' expected {:?}, got {:?}",
            expected_kind, out.kind
        );
    }

    // Fix 6 (C2-ITER2-BUG-2): Same-modality palette/particle remix mentioning current modality MUST NOT wipe scenes
    let seed_vid = gpui_telegram_code_scene::find_seed_by_id("seed_video_cyber_dawn").unwrap();
    let r_seed_vid = synth.remix_source(
        &seed_vid.source_code,
        &seed_vid.manifest,
        "换成落日合成波配色，让这个视频更有氛围",
        Some(&seed_vid.title),
        &seed_vid.tags,
    );
    assert!(r_seed_vid.validation.ok);
    assert_eq!(r_seed_vid.kind, AssetKind::Video);
    assert!(
        r_seed_vid.source_code.contains("__cs1_orig_render"),
        "M1 seed_video_cyber_dawn was wiped by same-modality palette+mention remix!"
    );
    assert!(!r_seed_vid.source_code.contains("fn on_update"));

    let seed_anim = gpui_telegram_code_scene::find_seed_by_id("seed_anim_aurora_plasma").unwrap();
    let r_seed_anim = synth.remix_source(
        &seed_anim.source_code,
        &seed_anim.manifest,
        "换成极光紫，给这个动画加点粒子",
        Some(&seed_anim.title),
        &seed_anim.tags,
    );
    assert!(r_seed_anim.validation.ok);
    assert_eq!(r_seed_anim.kind, AssetKind::Animation);
    assert!(
        r_seed_anim.source_code.contains("__cs1_orig_render"),
        "M1 seed_anim_aurora_plasma was wiped by same-modality palette+mention remix!"
    );

    let r_synth_vid = synth.remix_source(
        &video_base.source_code,
        &video_base.manifest,
        "change to frost minimal palette for this video",
        Some(&video_base.title),
        &video_base.tags,
    );
    assert!(r_synth_vid.validation.ok);
    assert_eq!(r_synth_vid.archetype, video_base.archetype);

    let r_synth_anim = synth.remix_source(
        &anim_base.source_code,
        &anim_base.manifest,
        "change to ocean teal colors for this animation",
        Some(&anim_base.title),
        &anim_base.tags,
    );
    assert!(r_synth_anim.validation.ok);
    assert_eq!(r_synth_anim.archetype, anim_base.archetype);

    // Fix 7 (C2-ITER2-BUG-3): Raw unfenced cs1 with top-level map/list literals & lowercase let/fn prose
    let raw_map_global = format!("let CFG = {{ a: 1 }};\n{FEW_SHOT_VIDEO_CS1}");
    let p_map = LlmResponseParser::parse(&raw_map_global)
        .expect("valid unfenced cs1 with top-level map literal must parse");
    assert_eq!(p_map.extraction_mode, "raw_cs1");
    assert!(p_map.source_code.contains("let CFG = { a: 1 };"));
    let (d_map, ok_map) = p_map.into_validated_draft("video", Some(AssetKind::Video), "live_llm");
    assert!(ok_map && d_map.validation.ok);

    let raw_list_maps = format!("let PAL = [{{ r: 1 }}, {{ r: 2 }}];\n{FEW_SHOT_VIDEO_CS1}");
    let p_list = LlmResponseParser::parse(&raw_list_maps)
        .expect("valid unfenced cs1 with top-level list-of-maps literal must parse");
    assert_eq!(p_list.extraction_mode, "raw_cs1");
    assert!(p_list.source_code.contains("let PAL = [{ r: 1 }, { r: 2 }];"));
    let (d_list, ok_list) =
        p_list.into_validated_draft("video", Some(AssetKind::Video), "live_llm");
    assert!(ok_list && d_list.validation.ok);

    let trailing_let_prose = format!(
        "let CFG = {{ a: 1 }};\n{FEW_SHOT_VIDEO_CS1}\n\nlet me know if you want to adjust {{color}}!"
    );
    let p_let_prose = LlmResponseParser::parse(&trailing_let_prose).unwrap();
    assert!(!p_let_prose.source_code.contains("let me know"));
    assert!(!p_let_prose.source_code.contains("{color}"));
    let (d_let_prose, ok_let_prose) =
        p_let_prose.into_validated_draft("video", Some(AssetKind::Video), "live_llm");
    assert!(ok_let_prose && d_let_prose.validation.ok);

    let trailing_fn_prose = format!(
        "{FEW_SHOT_VIDEO_CS1}\n\nfn stands for function in {{lang}}."
    );
    let p_fn_prose = LlmResponseParser::parse(&trailing_fn_prose).unwrap();
    assert!(!p_fn_prose.source_code.contains("stands for"));
    assert!(!p_fn_prose.source_code.contains("{lang}"));
    let (d_fn_prose, ok_fn_prose) =
        p_fn_prose.into_validated_draft("video", Some(AssetKind::Video), "live_llm");
    assert!(ok_fn_prose && d_fn_prose.validation.ok);

    // Fix 8a (C2-ITER2-BUG-4a): Hybrid JSON placeholder mentioning "fn on_render" falls through to ```cs1 fence
    let hybrid_fn_placeholder = format!(
        "```json\n{{\"title\":\"占位函数名\",\"kind\":\"animation\",\"tags\":[\"plasma\"],\"source_code\":\"see fn on_render below\"}}\n```\n\
         ```cs1\n{FEW_SHOT_ANIMATION_CS1}\n```"
    );
    let p_ph = LlmResponseParser::parse(&hybrid_fn_placeholder).unwrap();
    assert_eq!(p_ph.extraction_mode, "hybrid_json_and_fence");
    assert_eq!(p_ph.title.as_deref(), Some("占位函数名"));
    assert_eq!(p_ph.kind, Some(AssetKind::Animation));
    assert!(p_ph.source_code.contains("shader_plasma"));
    let (d_ph, ok_ph) = p_ph.into_validated_draft("plasma", Some(AssetKind::Animation), "live_llm");
    assert!(ok_ph && d_ph.validation.ok);

    // Fix 8b (C2-ITER2-BUG-4b): UTF-8 BOM-prefixed custom script remixes cleanly via Case 3 (NOT wiped)
    let bom_custom_src = "\u{feff}fn on_render(state, t, canvas) {\n    clear(0x0A0C22FF);\n    circle(180, 320, 42, 0x00F5FFFF);\n    text(180, 140, \"KEEP_BOM_SCRIPT\", 16, 0xFFFFFFFF, \"center\");\n}";
    let manifest_anim = AssetManifest::for_kind(AssetKind::Animation);
    let r_bom = synth.remix_source(
        bom_custom_src,
        &manifest_anim,
        "加快速度并换成金色",
        Some("BomScript"),
        &[],
    );
    assert!(r_bom.validation.ok, "BOM Case 3 remix invalid: {:?}", r_bom.validation.diagnostics);
    assert!(
        r_bom.source_code.contains("KEEP_BOM_SCRIPT") && r_bom.source_code.contains("__cs1_orig_render"),
        "UTF-8 BOM custom script was wiped instead of wrapped via Case 3!"
    );
}

#[test]
fn test_iter4_case1_palette_hue_unfenced_markdown_backtrack_and_hybrid_json_guard() {
    let synth = DeterministicProceduralSynthesizer::new();

    // 5 (C2-ITER3-BUG-1): All 5 stale-palette-tag archetypes + all 7 Challenger-2 repros
    let spectrum = synth.synthesize_prompt("律动频谱代码秀 紫色", Some(AssetKind::Video));
    let plasma = synth.synthesize_prompt("紫色流体动画", Some(AssetKind::Animation));
    let synthwave = synth.synthesize_prompt("合成波公路代码短片", Some(AssetKind::Video));
    let flappy = synth.synthesize_prompt("金色赛博飞鸟游戏", Some(AssetKind::Game));
    let snake = synth.synthesize_prompt("翡翠绿贪吃蛇游戏", Some(AssetKind::Game));

    let palette_switch_cases: [(&gpui_telegram_code_scene::SynthesizedDraft, &str, AssetKind, ColorPaletteTheme, &str); 7] = [
        (&spectrum, "换成金色，把这个视频改成互动动画", AssetKind::Animation, ColorPaletteTheme::SolarGold, "purple"),
        (&spectrum, "switch colors to gold and convert this video into an animation", AssetKind::Animation, ColorPaletteTheme::SolarGold, "purple"),
        (&plasma, "换成金色并把这个动画改成代码视频", AssetKind::Video, ColorPaletteTheme::SolarGold, "purple"),
        (&synthwave, "switch colors to frost minimal and convert this video into an animation", AssetKind::Animation, ColorPaletteTheme::FrostMinimal, "synthwave"),
        (&flappy, "换成深海蓝并把这个游戏改成代码视频", AssetKind::Video, ColorPaletteTheme::OceanBio, "gold"),
        (&snake, "turn this game into a video and change color to crimson red", AssetKind::Video, ColorPaletteTheme::CrimsonArcade, "emerald"),
        (&plasma, "change color to ocean teal and convert this animation into a fractal mandala", AssetKind::Animation, ColorPaletteTheme::OceanBio, "purple"),
    ];
    for (src, instr, exp_kind, exp_pal, stale_tag) in palette_switch_cases {
        let out = synth.remix_source(
            &src.source_code,
            &src.manifest,
            instr,
            Some(&src.title),
            &src.tags,
        );
        assert!(out.validation.ok, "switch '{instr}' failed validation: {:?}", out.validation.diagnostics);
        assert_eq!(out.kind, exp_kind, "wrong kind for '{instr}'");
        assert_eq!(out.palette, exp_pal, "wrong palette for '{instr}'");
        let exp_hue = format!("let PALETTE_HUE = {:.1};", exp_pal.spec().primary_hue);
        assert!(
            out.source_code.contains(&exp_hue),
            "'{instr}' expected '{exp_hue}' in source, got header: {}",
            out.source_code.lines().take(6).collect::<Vec<_>>().join(" | ")
        );
        let exp_hdr = format!("palette={}", exp_pal.as_str());
        assert!(out.source_code.contains(&exp_hdr), "missing '{exp_hdr}' for '{instr}'");
        assert!(
            out.tags.iter().any(|t| t == exp_pal.spec().tag),
            "missing target palette tag '{}' for '{instr}': {:?}",
            exp_pal.spec().tag,
            out.tags
        );
        assert!(
            !out.tags.iter().any(|t| t == stale_tag),
            "stale palette tag '{stale_tag}' leaked into tags for '{instr}': {:?}",
            out.tags
        );
    }

    // 6a (C2-ITER3-BUG-2a): Post-fn semicolon-less `let` followed by Markdown bullets/HR/blockquote/exclamation
    let md_suffixes = [
        "\n\n- Tip: tweak {EXTRA} for faster motion!",
        "\n\n* Or change {color} to gold!",
        "\n\n---\nEnjoy your scene!",
        "\n\n!Important: keep {EXTRA} positive!",
        "\n\n> Note: adjust {speed} smoothly.",
        "\n- Immediate bullet without blank line {x}",
        "\n+ Immediate plus bullet without blank line {y}",
    ];
    for suffix in md_suffixes {
        let raw = format!("{FEW_SHOT_VIDEO_CS1}\nlet EXTRA = 1.5{suffix}");
        let parsed = LlmResponseParser::parse(&raw)
            .unwrap_or_else(|e| panic!("failed to parse unfenced cs1 with suffix {suffix:?}: {e:?}"));
        assert_eq!(parsed.extraction_mode, "raw_cs1");
        assert!(parsed.source_code.contains("let EXTRA = 1.5"));
        let (draft, ok) = parsed.into_validated_draft("video", Some(AssetKind::Video), "live_llm");
        assert!(ok && draft.validation.ok, "suffix {suffix:?} poisoned extracted code:\n{}", parsed.source_code);
    }

    // Valid multi-line post-fn `let` continuation is STILL preserved
    let valid_multiline_let = format!("{FEW_SHOT_VIDEO_CS1}\nlet EXTRA = 1.5\n+ 2.5");
    let p_ml = LlmResponseParser::parse(&valid_multiline_let).unwrap();
    assert!(p_ml.source_code.contains("+ 2.5"));
    let (d_ml, ok_ml) = p_ml.into_validated_draft("video", Some(AssetKind::Video), "live_llm");
    assert!(ok_ml && d_ml.validation.ok);

    // 6b (C2-ITER3-BUG-2b): Decl-like explanatory trailing prose rejected via compile-checked backtracking
    let decl_prose_cases = [
        format!("{FEW_SHOT_VIDEO_CS1}\n\nfn on_render(state, t, canvas) is the main render callback {{60fps}}."),
        format!("{FEW_SHOT_VIDEO_CS1}\n\nlet SPEED = 2.0 controls the rotation speed {{rad/s}}!"),
    ];
    for raw in &decl_prose_cases {
        let parsed = LlmResponseParser::parse(raw).expect("decl-like trailing prose should backtrack cleanly");
        assert_eq!(parsed.extraction_mode, "raw_cs1");
        assert!(!parsed.source_code.contains("{60fps}"));
        assert!(!parsed.source_code.contains("{rad/s}"));
        let (draft, ok) = parsed.into_validated_draft("video", Some(AssetKind::Video), "live_llm");
        assert!(ok && draft.validation.ok, "decl-like prose leaked:\n{}", parsed.source_code);
    }

    // Broken `on_render` after valid `on_init` MUST still be returned intact for self-repair
    let broken_unfenced = "fn on_init(state) {\n    state.x = 1;\n}\nfn on_render(state, t, canvas) {\n    clear(0x000000FF);\n    bad_call(1, , 2);\n}";
    let p_broken = LlmResponseParser::parse(broken_unfenced).unwrap();
    assert!(p_broken.source_code.contains("bad_call(1, , 2)"));

    // 7 (C2-ITER3-BUG-3 + R2 Note): Hybrid JSON braced placeholder + Pass-2 JSON fence guard
    let hybrid_braced = format!(
        "```json\n{{\"title\":\"花括号占位\",\"kind\":\"video\",\"tags\":[\"warp\"],\"source_code\":\"see fn on_render() {{ /* full code in cs1 block below */ }}\"}}\n```\n\
         ```cs1\n{FEW_SHOT_VIDEO_CS1}\n```"
    );
    let p_hb = LlmResponseParser::parse(&hybrid_braced).unwrap();
    assert_eq!(p_hb.extraction_mode, "hybrid_json_and_fence");
    assert_eq!(p_hb.title.as_deref(), Some("花括号占位"));
    assert_eq!(p_hb.kind, Some(AssetKind::Video));
    assert_eq!(p_hb.tags, vec!["warp".to_string()]);
    let (d_hb, ok_hb) = p_hb.into_validated_draft("warp", Some(AssetKind::Video), "live_llm");
    assert!(ok_hb && d_hb.validation.ok);

    // R2 Note: JSON-only metadata fence containing "fn on_render" and "(" with NO cs1 fence must NOT be returned as cs1
    let json_only_meta_fence = "```json\n{\"title\":\"Scene (v2)\",\"kind\":\"video\",\"source_code\":\"see fn on_render below\"}\n```";
    assert!(matches!(
        LlmResponseParser::parse(json_only_meta_fence),
        Err(LlmParseError::NoScriptFound { .. })
    ));

    // Broken cs1 inside fenced JSON (no compiling fence) still falls back cleanly for self-repair
    let broken_json_fence = "```json\n{\"title\":\"Broken JSON\",\"kind\":\"game\",\"source_code\":\"fn on_render(s, t, c) { clear(0xFF); bad(1, , 2); }\"}\n```";
    let p_bj = LlmResponseParser::parse(broken_json_fence).unwrap();
    assert_eq!(p_bj.extraction_mode, "json_fenced");
    assert_eq!(p_bj.title.as_deref(), Some("Broken JSON"));
    assert!(p_bj.source_code.contains("bad(1, , 2)"));
}


