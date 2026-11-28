//! Comprehensive Milestone M2 Integration Tests (`OB-F03`, `OB-F04`, `OB-F05`, Evaluators, M2 Probes & Non-Regression).

use std::fs;

use gpui::{point, px, size};
use gpui_telegram_desktop::components::avatar::{
    Avatar, GROK_COLOR_PALETTE, GrokPersonaState, parse_fabushi_hex_color,
    render_fabushi_shape_avatar,
};
use gpui_telegram_desktop::components::bloub_avatar::*;
use gpui_telegram_desktop::theme::openbot_palette::*;
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use image::{ImageEncoder, Rgba, RgbaImage};
use openbot_contracts::avatar::{
    AVATAR_EXPRESSIONS, AvatarStyle, BloubExpression, EngineShapeId, PlushState, PlushTransition,
    stable_index,
};
use openbot_contracts::channels::{ChannelIcon, ChannelMember};

fn encode_test_png(width: u32, height: u32, noisy: bool) -> Vec<u8> {
    let mut img = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let px_val = if noisy {
                let h = (x.wrapping_mul(2_654_435_761)) ^ (y.wrapping_mul(2_246_822_519));
                Rgba([(h & 0xFF) as u8, ((h >> 8) & 0xFF) as u8, ((h >> 16) & 0xFF) as u8, 255])
            } else {
                Rgba([59, 147, 240, 255])
            };
            img.put_pixel(x, y, px_val);
        }
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(img.as_raw(), width, height, image::ExtendedColorType::Rgba8)
        .expect("encode PNG");
    out
}

fn encode_minimal_4frame_gif() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"GIF89a");
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&2u16.to_le_bytes());
    body.extend_from_slice(&[0x80, 0, 0]);
    body.extend_from_slice(&[0x10, 0x20, 0x30, 0xE0, 0x50, 0x40]);
    body.extend_from_slice(b"!\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00");
    for idx in 0..4u16 {
        body.extend_from_slice(b"!\xf9\x04\x00");
        body.extend_from_slice(&(8u16 + idx).to_le_bytes());
        body.extend_from_slice(&[0x00, 0x00]);
        body.push(b',');
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&2u16.to_le_bytes());
        body.extend_from_slice(&2u16.to_le_bytes());
        body.push(0x00);
        if idx % 2 == 0 {
            body.extend_from_slice(b"\x02\x02\x44\x01\x00");
        } else {
            body.extend_from_slice(b"\x02\x02\x4c\x01\x00");
        }
    }
    body.push(0x3B);
    body
}

#[test]
fn ob_f03_parametric_bloub_geometry_expressions_states_and_gaze() {
    // Golden radii checks
    let tri = engine_shape_radii(EngineShapeId::Triangle);
    assert!((tri[0] - 0.842_931).abs() < 5e-5);
    assert!((tri[1] - 0.898_078).abs() < 5e-5);
    assert!((tri[2] - 0.970_952).abs() < 5e-5);
    assert!((tri[3] - 1.048_094).abs() < 5e-5);
    assert!((engine_shape_radii(EngineShapeId::Capsule)[0] - 1.04).abs() < 1e-6);

    // Seed resolution, empty seed "", goutte index-7 remap, and explicit --shape goutte rejection
    let d_std = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        t: Some(0.0),
        ..AvatarEvalRequest::default()
    })
    .expect("standard flat seed");
    assert_eq!(d_std.profile.shape, "pentagone");
    assert_eq!(d_std.profile.expression, "fier");
    assert_eq!(d_std.profile.color, "#f08a24");
    assert_eq!(d_std.nearest_hue, Some(30));
    assert_eq!(d_std.view_box, "-158 -158 316 316");
    assert_eq!(d_std.paper, "#f9f9f9");
    assert_eq!(d_std.rays_count, 64);
    assert_eq!(d_std.cubic_segments, 64);
    assert!(d_std.body_path.starts_with('M') && d_std.body_path.ends_with('Z'));
    assert_eq!(d_std.eyes.left.cutout_op, "DstOut");

    let d_empty = evaluate_avatar_spec(&AvatarEvalRequest::default()).expect("empty seed");
    assert_eq!(d_empty.profile.style, "plush");
    assert_eq!(d_empty.profile.shape, "capsule");
    assert_eq!(d_empty.profile.expression, "excite");
    assert_eq!(d_empty.profile.color, "#e8483f");

    assert_eq!(stable_index("agent-0:shape", 11), 7);
    let d_remap = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-0".to_string(),
        ..AvatarEvalRequest::default()
    })
    .expect("goutte seed remaps");
    assert_eq!(d_remap.profile.shape, "losange");
    assert_eq!(d_remap.profile.expression, "timide");
    assert_eq!(d_remap.profile.color, "#f0b429");

    assert!(
        evaluate_avatar_spec(&AvatarEvalRequest {
            seed: "agent-1".to_string(),
            shape: Some("goutte".to_string()),
            ..AvatarEvalRequest::default()
        })
        .is_err()
    );

    // All 16 expressions & somnolent / asymmetric checks
    for expr in AVATAR_EXPRESSIONS {
        let ev = evaluate_avatar_spec(&AvatarEvalRequest {
            seed: "agent-1".to_string(),
            style: Some("flat".to_string()),
            expression: Some(expr.as_str().to_string()),
            t: Some(0.0),
            ..AvatarEvalRequest::default()
        })
        .expect("expression eval");
        assert_eq!(ev.profile.expression, expr.as_str());
        if expr == BloubExpression::Somnolent {
            assert!((ev.eyes.rest_open - 0.42).abs() < 1e-6);
        }
        if matches!(
            expr,
            BloubExpression::Mefiant | BloubExpression::Confus | BloubExpression::Curieux
        ) {
            let diff =
                (ev.eyes.left.w - ev.eyes.right.w).abs() + (ev.eyes.left.h - ev.eyes.right.h).abs();
            assert!(diff > 1e-3);
        }
    }

    // All 15 parametric states
    for st in [
        "idle", "thinking", "wink", "wide", "alert", "notify", "exclaim", "sleep", "egg",
        "hexagon", "play", "orbit", "swirl", "burst", "comet",
    ] {
        let ev = evaluate_avatar_spec(&AvatarEvalRequest {
            seed: "agent-1".to_string(),
            style: Some("flat".to_string()),
            state: Some(st.to_string()),
            t: Some(1.0),
            ..AvatarEvalRequest::default()
        })
        .expect("state eval");
        assert_eq!(ev.state, st);
        match st {
            "notify" => assert_eq!(ev.ornaments.notify_color.as_deref(), Some("#2496e8")),
            "orbit" => assert_eq!(ev.ornaments.orbit_arcs, 6),
            "thinking" => assert!(ev.ornaments.busy_dots >= 3),
            _ => {}
        }
    }
}

#[test]
fn ob_f03_blinks_breath_pointer_gaze_decalage_and_reduced_motion() {
    let sched = blink_schedule();
    assert_eq!(sched.len(), 317);
    assert!((sched[0] - 1.4).abs() < 1e-9);
    assert!((sched[2] - 9.687_224).abs() < 5e-5);
    assert!((sched[3] - 9.927_224).abs() < 5e-5);

    for mid_t in [1.481, 9.768_224, 10.008_224] {
        let ev = evaluate_avatar_spec(&AvatarEvalRequest {
            seed: "agent-1".to_string(),
            style: Some("flat".to_string()),
            expression: Some("neutre".to_string()),
            t: Some(mid_t),
            ..AvatarEvalRequest::default()
        })
        .expect("blink midpoint");
        assert!((ev.eyes.open_squash - 0.06).abs() < 0.015);
    }

    let peak = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        t: Some(0.85),
        ..AvatarEvalRequest::default()
    })
    .expect("breath peak");
    let trough = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        t: Some(2.55),
        ..AvatarEvalRequest::default()
    })
    .expect("breath trough");
    assert!((peak.breath_sy - 1.005).abs() < 1e-4);
    assert!((trough.breath_sy - 0.995).abs() < 1e-4);
    assert!(peak.eyes.wander_active);

    // Mouse pointer gaze vs Touch ignore
    let d_mouse = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        t: Some(1.2),
        pointer: Some((0.8, -0.5)),
        pointer_type: Some("mouse".to_string()),
        ..AvatarEvalRequest::default()
    })
    .expect("mouse gaze");
    assert!((d_mouse.eyes.gaze.yaw - (-13.2)).abs() < 0.1);
    assert!((d_mouse.eyes.gaze.pitch - 16.5).abs() < 0.1);
    assert!(!d_mouse.eyes.wander_active);

    let d_ambient = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        t: Some(1.2),
        ..AvatarEvalRequest::default()
    })
    .expect("ambient gaze");
    let d_touch = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        t: Some(1.2),
        pointer: Some((0.9, -0.8)),
        pointer_type: Some("touch".to_string()),
        ..AvatarEvalRequest::default()
    })
    .expect("touch gaze");
    assert_eq!(d_touch.eyes.gaze, d_ambient.eyes.gaze);
    assert_eq!(d_touch.body_path, d_ambient.body_path);

    // Narrow silhouettes corner gaze zero clipping
    for (shape, ptr) in [("triangle", (1.0, 1.0)), ("losange", (-1.0, -1.0))] {
        let ev = evaluate_avatar_spec(&AvatarEvalRequest {
            seed: "bot-alpha".to_string(),
            shape: Some(shape.to_string()),
            style: Some("flat".to_string()),
            pointer: Some(ptr),
            pointer_type: Some("mouse".to_string()),
            t: Some(2.3),
            ..AvatarEvalRequest::default()
        })
        .expect("narrow shape");
        assert!(ev.eyes.zero_clipping);
    }

    // Reduced motion frozen pose
    let r0 = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        reduced_motion: true,
        t: Some(0.0),
        ..AvatarEvalRequest::default()
    })
    .expect("reduced t=0");
    let r1 = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("flat".to_string()),
        reduced_motion: true,
        t: Some(1.481),
        ..AvatarEvalRequest::default()
    })
    .expect("reduced t=1.481");
    assert_eq!(r0.body_path, r1.body_path);
    assert_eq!(r0.eyes.open_squash, r1.eyes.open_squash);
    assert!((r1.breath_sy - 1.0).abs() < 1e-9);
}

#[test]
fn ob_f04_plush_assets_tint_ramp_moods_transitions_and_compositor() {
    let assets = BundledPlushAssets::global();
    let webp_paths = assets.bundled_webp_paths();
    assert_eq!(webp_paths.len(), EXPECTED_BUNDLED_WEBP_COUNT);
    assert_eq!(EXPECTED_BUNDLED_WEBP_COUNT, 73);
    for rel in BUNDLED_RELATIVE_WEBP_PATHS {
        let raw = assets.read_bytes(rel).expect("read bundled webp");
        assert!(raw.len() >= 12 && &raw[0..4] == b"RIFF" && &raw[8..12] == b"WEBP");
    }
    let webm_path = assets.root_dir().join("ui/fishing.webm");
    let webm_bytes = fs::read(&webm_path).expect("fishing.webm exists");
    assert_eq!(&webm_bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);

    // Golden tint_ramp vectors (#e8483f, #000000, #ffffff)
    let eval_red = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "bot-alpha".to_string(),
        color: Some("#e8483f".to_string()),
        style: Some("plush".to_string()),
        ..AvatarEvalRequest::default()
    })
    .expect("plush red eval");
    assert_eq!(eval_red.tint_ramp.get("0"), Some(&[0, 0, 0, 255]));
    assert_eq!(eval_red.tint_ramp.get("64"), Some(&[149, 43, 37, 255]));
    assert_eq!(eval_red.tint_ramp.get("128"), Some(&[220, 68, 59, 255]));
    assert_eq!(eval_red.tint_ramp.get("140"), Some(&[232, 72, 63, 255]));
    assert_eq!(eval_red.tint_ramp.get("200"), Some(&[255, 98, 89, 255]));
    assert_eq!(eval_red.tint_ramp.get("255"), Some(&[255, 127, 118, 255]));

    let ramp_blk = tint_ramp("#000000");
    assert_eq!(ramp_blk[140], [0, 0, 0, 255]);
    assert_eq!(ramp_blk[255], [80, 80, 80, 255]);
    let ramp_wht = tint_ramp("#ffffff");
    assert_eq!(ramp_wht[140], [255, 255, 255, 255]);
    assert_eq!(ramp_wht[255], [255, 255, 255, 255]);

    // All 7 plush moods + working_faces cycle
    let w0 = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("plush".to_string()),
        mood: Some("working".to_string()),
        working_faces: true,
        t: Some(0.0),
        ..AvatarEvalRequest::default()
    })
    .expect("working t=0");
    let w1 = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("plush".to_string()),
        mood: Some("working".to_string()),
        working_faces: true,
        t: Some(1.85),
        ..AvatarEvalRequest::default()
    })
    .expect("working t=1.85");
    assert_eq!(w0.state, "thinking");
    assert_eq!(w0.eyes.left.pad_blend, "plus-lighter");
    assert_ne!(w0.profile.expression, w1.profile.expression);

    let wait_ev = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("plush".to_string()),
        mood: Some("waiting".to_string()),
        t: Some(0.2),
        ..AvatarEvalRequest::default()
    })
    .expect("waiting");
    assert_eq!(wait_ev.state, "notify");
    assert_eq!(wait_ev.ornaments.notify_color.as_deref(), Some("#2496e8"));

    let fail_ev = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("plush".to_string()),
        mood: Some("failed".to_string()),
        t: Some(0.12),
        ..AvatarEvalRequest::default()
    })
    .expect("failed");
    assert_eq!(fail_ev.state, "failed");
    assert!(fail_ev.ornaments.shake_deg.abs() > 0.1);

    let sleep_ev = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        style: Some("plush".to_string()),
        mood: Some("asleep".to_string()),
        t: Some(0.2),
        ..AvatarEvalRequest::default()
    })
    .expect("asleep");
    assert_eq!(sleep_ev.state, "asleep");
    assert_eq!(sleep_ev.ornaments.sleep_zs, 3);

    // 6 named characters + 5 transitions + RGBA compositor
    for ch in PlushCharacterId::ALL {
        let mut r = PlushFeltRenderer::for_character(ch, PlushState::Idle);
        for tr in [
            PlushTransition::Flip,
            PlushTransition::Pop,
            PlushTransition::Warp,
            PlushTransition::Dissolve,
            PlushTransition::Cut,
        ] {
            r.set_transition(tr);
            let sample = r.sample_plush_frame(0.25, false);
            let rgba = composite_plush_rgba(&sample, 64);
            assert_eq!(rgba.dimensions(), (64, 64));
            let _div = render_plush_avatar_element(&sample, 36.0);
        }
    }
}

#[test]
fn ob_f04_clusters_284px_editor_normalizer_crew_pile_mascots_and_app_logo() {
    // ChannelAvatarCluster: 0, 1..4, 8, 12, 13, Row, IconTile, broken image fallback
    for (n, exp_shown, exp_hidden, exp_pill, exp_fallback) in [
        (0usize, 0usize, 0usize, None, Some("UsersRound")),
        (1, 1, 0, None, None),
        (4, 4, 0, None, None),
        (8, 3, 5, Some("+5"), None),
        (12, 3, 9, Some("+9"), None),
        (13, 2, 11, Some("+11"), None),
    ] {
        let ev = evaluate_avatar_spec(&AvatarEvalRequest {
            seed: "agent-1".to_string(),
            cluster_count: Some(n),
            ..AvatarEvalRequest::default()
        })
        .expect("cluster eval");
        assert_eq!(ev.cluster.shown_faces, exp_shown);
        assert_eq!(ev.cluster.hidden_count, exp_hidden);
        assert_eq!(ev.cluster.plus_pill.as_deref(), exp_pill);
        assert_eq!(ev.cluster.fallback_glyph.as_deref(), exp_fallback);
    }

    let members_5: Vec<ChannelMember> = (0..5)
        .map(|i| ChannelMember {
            agent_id: format!("agent-{i}"),
        })
        .collect();
    let row_eval =
        resolve_channel_cluster(&members_5, ChannelAvatarLayout::Row, None, None, false);
    assert_eq!(row_eval.shown_faces, 3);
    assert_eq!(row_eval.plus_pill, None);
    assert_eq!(row_eval.row_overlap_px, -16.0);

    let mut cluster_ctrl = ChannelAvatarCluster::new();
    cluster_ctrl.on_image_error("https://invalid.example/broken.webp");
    let broken_eval = cluster_ctrl.evaluate(
        &members_5,
        ChannelAvatarLayout::Cluster,
        Some(&ChannelIcon {
            name: "code".to_string(),
            hue: 215,
        }),
        Some("https://invalid.example/broken.webp"),
    );
    assert_eq!(broken_eval.mode, ChannelClusterMode::IconTile);

    // 284px AgentAvatarEditorState
    assert_eq!(AgentAvatarEditorState::popover_width_for_panel(400.0), 284.0);
    let mut editor = AgentAvatarEditorState::new("agent-1", None, None, None, None);
    assert_eq!(editor.style(), AvatarStyle::Plush);
    assert!(editor.automatic_color_selected());
    let flat_patch = editor
        .set_style(AvatarStyle::Flat)
        .expect("switch to flat")
        .expect("patch emitted");
    assert!(flat_patch.avatar_seed.as_deref().unwrap_or("").contains(":flat:"));
    assert_eq!(editor.style(), AvatarStyle::Flat);
    let mut rng = Mulberry32::new(42);
    let _ = editor.shuffle(&mut rng).expect("shuffle succeeds");
    assert!(editor.can_reset_to_id());
    let _popover_el = render_avatar_editor_popover(&editor, 400.0, true, 0.0);
    let _ = editor.reset_to_id().expect("reset to id");

    // AvatarImageNormalizer: rectangular PNG, animated 4-frame GIF, and negative rejections
    let tmp = tempfile::tempdir().expect("tempdir");
    let rect_png = tmp.path().join("rect_1200x800.png");
    fs::write(&rect_png, encode_test_png(1200, 800, false)).expect("write rect png");
    let norm_ev = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        normalize_image: Some(rect_png),
        ..AvatarEvalRequest::default()
    })
    .expect("normalize png");
    let ni = norm_ev.normalized_image.expect("normalized_image present");
    assert_eq!(ni.mime, "image/webp");
    assert_eq!(ni.width, ni.height);
    assert!([512, 448, 384, 320].contains(&ni.width));
    assert!(ni.byte_length >= 12 && ni.byte_length <= 524_288);
    assert!(ni.magic.starts_with("RIFF") && ni.magic.contains("WEBP"));
    assert!(!ni.animated);

    let anim_gif = tmp.path().join("anim_4frame.gif");
    fs::write(&anim_gif, encode_minimal_4frame_gif()).expect("write anim gif");
    let gif_ev = evaluate_avatar_spec(&AvatarEvalRequest {
        seed: "agent-1".to_string(),
        normalize_image: Some(anim_gif),
        ..AvatarEvalRequest::default()
    })
    .expect("normalize animated gif");
    let gi = gif_ev.normalized_image.expect("gif normalized");
    assert!(gi.animated);
    assert_eq!(gi.frame_count, 4);
    assert!(gi.byte_length <= 524_288);

    let noisy_out = AvatarImageNormalizer::normalize_bytes(&encode_test_png(600, 600, true), Some("image/png"))
        .expect("noisy png normalizes within 512 KiB");
    assert!(noisy_out.byte_length <= 524_288);

    let huge = vec![0u8; 11 * 1024 * 1024];
    assert!(AvatarImageNormalizer::normalize_bytes(&huge, Some("image/png")).is_err());
    assert!(AvatarImageNormalizer::normalize_bytes(b"<svg></svg>", Some("image/svg+xml")).is_err());
    assert!(AvatarImageNormalizer::normalize_bytes(b"", Some("image/webp")).is_err());
    assert!(
        AvatarImageNormalizer::normalize_bytes(b"RIFF\x10\x00\x00\x00XXXXbad", Some("image/webp"))
            .is_err()
    );

    // 240Hz CrewPilePhysics, GlobalSearchPlush, ServerPlushLogo, Marketplace, Recovery, 5-click AppLogo
    let mut pile = CrewPilePhysics::new(0x1234_5678, false, DEFAULT_ROOM);
    assert!((8..=64).contains(&pile.bodies.len()));
    pile.step(0.25);
    pile.pointer_repel(480.0, 560.0, 120.0, 900.0);
    pile.step(0.10);
    let hop = pile.hop_out();
    assert!(hop.length_ms > 0.0);
    let _stage = render_crew_pile_stage(&pile, 0.35, None);

    let srv_logo = ServerPlushLogo::new("srv-alpha");
    assert!(srv_logo.resolve_rgba(BundledPlushAssets::global()).is_ok());
    let _srv_el = srv_logo.render_element();

    let mut search_plush = GlobalSearchPlushState::new(false);
    search_plush.update_caret(180.0, 20.0, 100.0, 20.0, 0.0);
    assert_eq!(search_plush.tick(100.0).gaze, Some((0.5, 0.0)));
    let dart_sample = search_plush.tick(650.0);
    assert!(dart_sample.looking_for_result);

    let mut mkt = MarketplaceNoMatchPlushState::new(false);
    assert_eq!(mkt.active_asset_name(), "fishing.webm");
    mkt.on_loaded_top_left_alpha(200);
    assert_eq!(mkt.active_asset_name(), "fishing-still.webp");
    assert!(recovery_plush_svg().contains("plus-lighter"));

    let mut logo = AppLogoState::new(AppLogoVariant::Production);
    for click_idx in 0..4 {
        assert_eq!(logo.register_click((click_idx as f64) * 120.0), AppLogoReaction::Wink);
    }
    assert_eq!(logo.register_click(500.0), AppLogoReaction::Party);
    let party_frame = logo.sample(650.0);
    assert_eq!(party_frame.easter_egg, Some("party"));
    let _logo_el = render_app_logo(&logo, 650.0, 48.0);
}

#[test]
fn ob_f05_palette_oklab_watcher_transitions_and_six_backdrops() {
    let dark = OpenBotPalette::dark();
    assert_eq!(dark.token_count(), 58);
    assert_eq!(dark.token("--openbot-bg-frame"), Some("#0d0d0d"));
    assert_eq!(dark.token("--openbot-bg-canvas"), Some("#1a1a1a"));
    assert_eq!(dark.token("--openbot-bg-sidebar"), Some("#1a1a1a"));
    assert_eq!(dark.token("--openbot-text-primary"), Some("#ffffff"));
    assert_eq!(dark.token("--openbot-accent"), Some("#007cf7"));
    assert_eq!(dark.window_glass_tokens().len(), 15);

    let light = OpenBotPalette::light();
    assert_eq!(light.token_count(), 58);
    assert_eq!(light.token("--openbot-bg-frame"), Some("#e8e8eb"));
    assert_eq!(light.token("--openbot-bg-canvas"), Some("#f5f5f6"));
    assert_eq!(light.token("--openbot-bg-surface"), Some("#ffffff"));
    assert_eq!(light.token("--openbot-text-primary"), Some("#141414"));
    assert_eq!(light.token("--openbot-accent-hover"), Some("#006ad4"));

    // OKLab readable_chat_background golden vectors & achromatic bounds
    assert_eq!(
        readable_chat_background("#f75d59", OpenBotColorScheme::Light).unwrap(),
        "#fceae8"
    );
    assert_eq!(
        readable_chat_background("#f75d59", OpenBotColorScheme::Dark).unwrap(),
        "#442321"
    );
    assert_eq!(
        readable_chat_background("#ffffff", OpenBotColorScheme::Dark).unwrap(),
        "#2e2e2e"
    );
    assert_eq!(
        readable_chat_background("#000000", OpenBotColorScheme::Light).unwrap(),
        "#eeeeee"
    );

    // CSS injection rejection, >64 KiB rejection, BAD_ID!.json skip, 32-file cap, & persist
    let tmp = tempfile::tempdir().expect("tempdir");
    let evil_path = tmp.path().join("evil.json");
    fs::write(
        &evil_path,
        r##"{"name":"Evil","appearance":"dark","colors":{"canvas":"#1a1a1a","frame":"#0d0d0d","sidebar":"#1a1a1a","surface":"#212121","text":"#ffffff","textMuted":"#979797","border":"#2a2a2a","accent":"red; background: url(https://evil.test)","accentForeground":"#ffffff","success":"#31cf76","warning":"#ff9412","danger":"#fd2f3b"}}"##,
    )
    .unwrap();
    assert!(
        evaluate_theme_spec(&ThemeEvalRequest {
            theme_json: Some(evil_path),
            ..ThemeEvalRequest::default()
        })
        .is_err()
    );

    let huge_path = tmp.path().join("huge.json");
    fs::write(&huge_path, format!("{{{}}}", " ".repeat(70_000))).unwrap();
    assert!(
        evaluate_theme_spec(&ThemeEvalRequest {
            theme_json: Some(huge_path),
            ..ThemeEvalRequest::default()
        })
        .is_err()
    );

    let many_dir = tmp.path().join("many_themes");
    fs::create_dir_all(&many_dir).unwrap();
    let valid_json = r##"{"name":"Valid","appearance":"dark","colors":{"canvas":"#1a1a1a","frame":"#0d0d0d","sidebar":"#1a1a1a","surface":"#212121","text":"#ffffff","textMuted":"#979797","border":"#2a2a2a","accent":"#007cf7","accentForeground":"#ffffff","success":"#31cf76","warning":"#ff9412","danger":"#fd2f3b"}}"##;
    for i in 0..36 {
        fs::write(many_dir.join(format!("theme-{i:02}.json")), valid_json).unwrap();
    }
    fs::write(many_dir.join("BAD_ID!.json"), valid_json).unwrap();
    let cap_eval = evaluate_theme_spec(&ThemeEvalRequest {
        mode: Some("dark".to_string()),
        themes_dir: Some(many_dir),
        persist: true,
        data_dir: Some(tmp.path().to_path_buf()),
        ..ThemeEvalRequest::default()
    })
    .expect("32-cap eval");
    assert_eq!(cap_eval.loaded_palettes.len(), 32);
    assert!(cap_eval.persisted_path.expect("persisted").is_file());

    // All 6 ChatBackdropScenes + aliases + unknown rejection + CustomBackdropModel
    for (cli_name, canonical, exp_dark) in [
        ("pond", "pond", "#0f2426"),
        ("forest", "jellyfish", "#0e1a2b"),
        ("city", "sunset", "#1d1528"),
        ("snow", "snowfall", "#111a26"),
        ("desert", "fireflies", "#121a14"),
        ("coast", "coast", "#1a1030"),
    ] {
        let ev = evaluate_theme_spec(&ThemeEvalRequest {
            mode: Some("dark".to_string()),
            backdrop: Some(cli_name.to_string()),
            elapsed_sec: Some(14.5),
            ..ThemeEvalRequest::default()
        })
        .expect("backdrop eval");
        assert_eq!(ev.backdrop.scene, cli_name);
        assert_eq!(ev.backdrop.canonical_scene, canonical);
        assert_eq!(ev.backdrop.base_hex, exp_dark);
        assert_eq!(ev.backdrop.cell_px, 6);
        assert!((ev.backdrop.calm_center_alpha - 0.55).abs() < 1e-9);
        assert!(ev.backdrop.draw_cells > 0);
    }
    assert!(
        evaluate_theme_spec(&ThemeEvalRequest {
            backdrop: Some("mars-colony".to_string()),
            ..ThemeEvalRequest::default()
        })
        .is_err()
    );
    assert!(custom_backdrop_kind("image/svg+xml", 128).is_none());

    // sweepTheme & beam overlays
    let sweep_state = ThemeSweepState::new(OpenBotColorScheme::Dark, OpenBotColorScheme::Light, false);
    let _sweep_el = render_theme_sweep_overlay(&sweep_state);
    let beam_state = BeamWaveState::new(point(px(32.0), px(96.0)), size(px(960.0), px(640.0)), true, false);
    let _beam_el = render_beam_overlay(&beam_state);
}

#[test]
fn all_six_m2_ui_probes_and_baseline_avatar_tdesktop_non_regression() {
    let cp = probe_crew_pile_physics(None, None);
    assert_eq!(cp.step_hz, 240);
    assert_eq!(cp.solver_iterations, 4);
    assert!((8..=64).contains(&cp.body_count));
    for slug in ["yoshi", "nexo", "malu", "fred", "vita", "cloe"] {
        assert!(cp.characters.iter().any(|c| c == slug));
    }
    assert!((cp.breathe_period_sec - 3.2).abs() < 1e-6);

    let cp_red = probe_crew_pile_reduced_motion_and_broken_url(None, None);
    assert_eq!(cp_red.max_squash, 0.0);
    assert_eq!(cp_red.max_tilt, 0.0);
    assert!(cp_red.all_faces_idle);
    assert!(cp_red.failed_url_fallback);

    let hz = probe_avatar_horizon_cull();
    assert!(hz.culled_at_0_02);
    assert!((hz.alpha_at_0_06 - 0.5).abs() < 0.01);

    let mm = probe_avatar_mid_morph();
    assert!(!mm.snapped_to_rest);
    assert!(mm.max_pose_jump < 0.05);

    let tmp = tempfile::tempdir().expect("tempdir");
    let hot_default = probe_theme_hot_reload(Some(tmp.path())).expect("default hot reload");
    assert!(hot_default.watcher_triggered);
    assert_eq!(hot_default.active_canvas, "#1a1b26");
    assert_eq!(hot_default.rewritten_canvas, "#eff1f5");
    assert!(hot_default.css_injection_refused);
    assert_eq!(hot_default.refused_fallback_canvas, "#1a1a1a");
    assert_eq!(hot_default.restored_canvas, "#1a1b26");

    // Caller-staged Catppuccin Latte in data/themes/catppuccin-latte.json
    let caller_themes = tmp.path().join("themes");
    fs::create_dir_all(&caller_themes).unwrap();
    fs::write(
        caller_themes.join("catppuccin-latte.json"),
        r##"{"name":"Catppuccin Latte","appearance":"light","colors":{"canvas":"#eff1f5","frame":"#e6e9ef","sidebar":"#e6e9ef","surface":"#ccd0da","text":"#4c4f69","textMuted":"#6c6f85","border":"#bcc0cc","accent":"#1e66f5","accentForeground":"#eff1f5","success":"#40a02b","warning":"#df8e1d","danger":"#d20f39"}}"##,
    )
    .unwrap();
    let hot_staged = probe_theme_hot_reload(Some(tmp.path())).expect("staged hot reload");
    assert!(hot_staged.watcher_triggered);
    assert_eq!(hot_staged.active_canvas, "#eff1f5");

    let sweep_red = probe_theme_sweep_reduced_motion(true);
    assert_eq!(sweep_red.sweep_duration_ms, 0);
    assert_eq!(sweep_red.beam_duration_ms, 0);
    assert!(!sweep_red.backdrop_animated);
    assert_eq!(sweep_red.backdrop_time_sec, 42.0);

    // Baseline non-regression: Avatar (classic, 8 Grok shapes, bloub:*, plush:*) + TdesktopThemePalette
    assert_eq!(parse_fabushi_hex_color("#10B981"), Some(0x10B981));
    for spec in GROK_COLOR_PALETTE {
        assert_eq!(parse_fabushi_hex_color(spec.id), Some(spec.swatch_rgb));
    }
    for shape in [
        "classic", "blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop",
        "bloub:pentagone", "bloub:agent-1", "plush:nexo", "plush:agent-1",
    ] {
        let _el = render_fabushi_shape_avatar(shape, 0x2AABEE, "Alice Bot", 36.0);
    }
    let av = Avatar::new("OpenBot Agent", 40.0)
        .with_color(0xE8483F)
        .with_image_url(Some("file:///nonexistent/broken.webp".to_string()))
        .with_shape("plush:agent-1")
        .with_persona_state(GrokPersonaState::Working)
        .with_online(true)
        .with_palette(TdesktopThemePalette::build(
            TdesktopThemeMode::Night,
            0x2AABEE,
            13,
            12,
        ));
    assert_eq!(av.size, 40.0);
    assert!(av.is_online);

    for mode in [
        TdesktopThemeMode::Night,
        TdesktopThemeMode::Tinted,
        TdesktopThemeMode::Day,
        TdesktopThemeMode::Classic,
    ] {
        let p = TdesktopThemePalette::build(mode, 0x2AABEE, 13, 12);
        assert_eq!(p.mode, mode);
    }
}

#[test]
fn m1_ui_f01_openbot_tokens_typed_accessors_oklab_and_chrome_sync() {
    use gpui::rgb;
    use gpui_telegram_desktop::theme::{
        CHAT_COLOR_PRESETS, OPENBOT_BADGE_NEW_DARK, OPENBOT_BADGE_NEW_LIGHT, OPENBOT_FELT_AMBER,
        OPENBOT_FELT_CREAM, OPENBOT_FELT_PLUM, OPENBOT_FELT_RASPBERRY,
        readable_chat_background_for_dark, solid_bubble_takes_dark_text,
    };

    let dark = OpenBotPalette::dark();
    assert_eq!(dark.token_count(), 58);
    assert_eq!(dark.window_glass_tokens().len(), 15);
    assert_eq!(dark.bg_frame(), rgb(0x0D0D0D));
    assert_eq!(dark.bg_canvas(), rgb(0x1A1A1A));
    assert_eq!(dark.bg_sidebar(), rgb(0x1A1A1A));
    assert_eq!(dark.bg_surface(), rgb(0x212121));
    assert_eq!(dark.bg_surface_raised(), rgb(0x242424));
    assert_eq!(dark.bg_control(), rgb(0x212121));
    assert_eq!(dark.bg_control_hover(), rgb(0x242424));
    assert_eq!(dark.bg_control_active(), rgb(0x2A2A2A));
    assert_eq!(dark.border_hairline(), rgb(0x262626));
    assert_eq!(dark.border_strong_hairline(), rgb(0x323232));
    assert_eq!(dark.text_primary(), rgb(0xFFFFFF));
    assert_eq!(dark.text_secondary(), rgb(0xDCDCDC));
    assert_eq!(dark.text_muted(), rgb(0x979797));
    assert_eq!(dark.text_dim(), rgb(0x6A6A6A));
    assert_eq!(dark.surface_light(), rgb(0xF0F0F0));
    assert_eq!(dark.text_on_light(), rgb(0x141414));
    assert_eq!(dark.accent(), rgb(0x007CF7));
    assert_eq!(dark.accent_hover(), rgb(0x1F8FFF));
    assert_eq!(dark.accent_text(), rgb(0x42A0FF));
    assert_eq!(dark.success(), rgb(0x31CF76));
    assert_eq!(dark.warning(), rgb(0xFF9412));
    assert_eq!(dark.danger(), rgb(0xFD2F3B));
    assert_eq!(dark.unread_surface(), rgb(0x74B9FF));
    assert_eq!(dark.unread_text(), rgb(0x07131F));
    assert_eq!(dark.badge_new(), rgb(0xFF1E5E));
    assert_eq!(dark.felt_plum(), (rgb(0x705083), rgb(0x4A285C)));
    assert_eq!(dark.felt_amber(), (rgb(0xFF9511), rgb(0xCB680C)));
    assert_eq!(dark.felt_raspberry(), (rgb(0xF52070), rgb(0xB40641)));
    assert_eq!(dark.felt_cream(), rgb(0xF6EFE2));
    assert!((dark.border().a - 0.035).abs() < 1e-3);
    assert!((dark.border_strong().a - 0.045).abs() < 1e-3);
    assert!((dark.hover().a - 0.06).abs() < 1e-3);
    assert!((dark.text_readable().a - 0.52).abs() < 1e-3);
    assert!((dark.accent_soft().a - 0.16).abs() < 1e-3);

    let light = OpenBotPalette::light();
    assert_eq!(light.token_count(), 58);
    assert_eq!(light.bg_frame(), rgb(0xE8E8EB));
    assert_eq!(light.bg_canvas(), rgb(0xF5F5F6));
    assert_eq!(light.bg_sidebar(), rgb(0xF5F5F6));
    assert_eq!(light.bg_surface(), rgb(0xFFFFFF));
    assert_eq!(light.bg_surface_raised(), rgb(0xFCFCFC));
    assert_eq!(light.bg_control_hover(), rgb(0xF0F0F2));
    assert_eq!(light.bg_control_active(), rgb(0xE8E8EB));
    assert_eq!(light.border_hairline(), rgb(0xE8E8EB));
    assert_eq!(light.border_strong_hairline(), rgb(0xDCDCE0));
    assert_eq!(light.text_primary(), rgb(0x141414));
    assert_eq!(light.text_muted(), rgb(0x6B6B70));
    assert_eq!(light.surface_light(), rgb(0x1C1C1E));
    assert_eq!(light.text_on_light(), rgb(0xFFFFFF));
    assert_eq!(light.badge_new(), rgb(0xE0004A));

    assert_eq!(OPENBOT_BADGE_NEW_DARK, "#ff1e5e");
    assert_eq!(OPENBOT_BADGE_NEW_LIGHT, "#e0004a");
    assert_eq!(OPENBOT_FELT_PLUM, ("#705083", "#4a285c"));
    assert_eq!(OPENBOT_FELT_AMBER, ("#ff9511", "#cb680c"));
    assert_eq!(OPENBOT_FELT_RASPBERRY, ("#f52070", "#b40641"));
    assert_eq!(OPENBOT_FELT_CREAM, "#f6efe2");
    assert_eq!(CHAT_COLOR_PRESETS.len(), 11);

    assert_eq!(
        readable_chat_background_for_dark("#f75d59", true).unwrap(),
        "#442321"
    );
    assert_eq!(
        readable_chat_background_for_dark("#f75d59", false).unwrap(),
        "#fceae8"
    );
    assert_eq!(dark.readable_chat_background("#f75d59").unwrap(), "#442321");
    assert_eq!(
        light.readable_chat_background("#f75d59").unwrap(),
        "#fceae8"
    );
    assert!(solid_bubble_takes_dark_text("#e5a50a"));
    assert!(!solid_bubble_takes_dark_text("#007cf7"));

    // TdesktopThemePalette OpenBot sync: zero legacy harsh Telegram/Apple blue active fills
    let ob_dark = TdesktopThemePalette::build_openbot(TdesktopThemeMode::Night, 0, 15, 16)
        .with_ui_scale(125)
        .with_blur_wallpaper(true);
    assert_eq!(ob_dark.openbot_scheme(), OpenBotColorScheme::Dark);
    assert_eq!(ob_dark.openbot_palette().bg_frame(), rgb(0x0D0D0D));
    assert_eq!(ob_dark.window_bg, 0x0D0D0D);
    assert_eq!(ob_dark.dialogs_bg, 0x1A1A1A);
    assert_eq!(ob_dark.stage_bg, 0x1A1A1A);
    assert_eq!(ob_dark.dialogs_active_bg, 0x212121);
    assert_eq!(ob_dark.dialogs_hover_bg, 0x242424);
    assert_eq!(ob_dark.msg_in_bg, 0x212121);
    assert_eq!(ob_dark.msg_out_bg, 0x242424);
    assert_eq!(ob_dark.border_fg, 0x262626);
    assert_eq!(ob_dark.unread_badge_bg, 0x705083);
    assert_eq!(ob_dark.unread_badge_fg, 0xF6EFE2);
    assert_eq!(ob_dark.font_size_px, 15);
    assert_eq!(ob_dark.bubble_radius_px, 16);
    assert_eq!(ob_dark.ui_scale_pct, 125);
    assert!(ob_dark.blur_wallpaper);
    assert_ne!(ob_dark.dialogs_active_bg, 0x007AFF);
    assert_ne!(ob_dark.dialogs_active_bg, 0x0A84FF);
    assert_eq!(ob_dark.with_openbot_chrome(), ob_dark);

    let ob_light = TdesktopThemePalette::build_openbot(TdesktopThemeMode::Day, 0, 14, 18);
    assert_eq!(ob_light.openbot_scheme(), OpenBotColorScheme::Light);
    assert_eq!(ob_light.openbot_palette().bg_frame(), rgb(0xE8E8EB));
    assert_eq!(ob_light.window_bg, 0xE8E8EB);
    assert_eq!(ob_light.dialogs_bg, 0xF5F5F6);
    assert_eq!(ob_light.dialogs_active_bg, 0xE8E8EB);
    assert_eq!(ob_light.msg_in_bg, 0xFFFFFF);
    assert_eq!(ob_light.msg_out_bg, 0xFCFCFC);
    assert_eq!(ob_light.msg_out_fg, 0x141414);
    assert_ne!(ob_light.dialogs_active_bg, 0x1680F3);
    assert_eq!(ob_light.with_openbot_chrome(), ob_light);

    let ob_classic = TdesktopThemePalette::build_openbot(TdesktopThemeMode::Classic, 0, 14, 12);
    assert_eq!(ob_classic.openbot_scheme(), OpenBotColorScheme::Light);
    assert_eq!(ob_classic.msg_out_bg, 0xEFFDDE);
    assert_eq!(ob_classic.dialogs_active_bg, 0xE8E8EB);
}

#[test]
fn m1_ui_f02_plush_bot_default_routing_felt_badges_and_channel_avatar() {
    use gpui_telegram_desktop::components::avatar::{
        FeltStatusBadgeState, GrokBotAvatarProps, legacy_shape_to_bloub_shape,
        render_channel_avatar, render_felt_status_badge, render_grok_bot_avatar,
        render_plush_bot_avatar, render_plush_bot_avatar_with_controller,
    };
    use openbot_contracts::avatar::{
        ACTIVITY_HOLD_MS, AgentStepActivity, AvatarHue, AvatarMood, BloubShape,
    };

    // Legacy shape mappings & deterministic fallback for generic blob/teardrop/empty
    assert_eq!(legacy_shape_to_bloub_shape("pebble"), Some(BloubShape::Galet));
    assert_eq!(
        legacy_shape_to_bloub_shape("squircle"),
        Some(BloubShape::Squircle)
    );
    assert_eq!(
        legacy_shape_to_bloub_shape("tablet"),
        Some(BloubShape::Capsule)
    );
    assert_eq!(
        legacy_shape_to_bloub_shape("hex"),
        Some(BloubShape::Hexagone)
    );
    assert_eq!(
        legacy_shape_to_bloub_shape("hexagon"),
        Some(BloubShape::Hexagone)
    );
    assert_eq!(legacy_shape_to_bloub_shape("cloud"), Some(BloubShape::Nuage));
    assert_eq!(
        legacy_shape_to_bloub_shape("wedge"),
        Some(BloubShape::Triangle)
    );
    assert_eq!(
        legacy_shape_to_bloub_shape("diamond"),
        Some(BloubShape::Losange)
    );
    assert_eq!(
        legacy_shape_to_bloub_shape("circle"),
        Some(BloubShape::Cercle)
    );
    assert_eq!(
        legacy_shape_to_bloub_shape("shield"),
        Some(BloubShape::Pentagone)
    );
    assert_eq!(legacy_shape_to_bloub_shape("blob"), None);
    assert_eq!(legacy_shape_to_bloub_shape("teardrop"), None);
    assert_eq!(legacy_shape_to_bloub_shape(""), None);
    assert_eq!(legacy_shape_to_bloub_shape("plush"), None);

    // All 13 silhouettes, 7 moods, and 8 tool activities render via PlushFeltRenderer
    for shape in BloubShape::ALL {
        let mut r = PlushFeltRenderer::for_seed_with_activity(
            "bot-alpha",
            Some(shape),
            Some(AvatarHue::Violet),
            None,
            AvatarMood::Idle,
            None,
        );
        let sample = r.sample_plush_frame(0.0, true);
        assert_eq!(sample.shape, bloub_shape_to_engine(shape));
        let _div = render_plush_bot_avatar(
            "bot-alpha",
            Some(shape),
            Some(AvatarHue::Violet),
            None,
            40.0,
            AvatarMood::Idle,
            None,
        );
    }
    for mood in [
        AvatarMood::Idle,
        AvatarMood::Working,
        AvatarMood::Waiting,
        AvatarMood::Failed,
        AvatarMood::Responded,
        AvatarMood::Connecting,
        AvatarMood::Asleep,
    ] {
        let _div = render_plush_bot_avatar("bot-mood", None, None, Some("#3b93f0"), 36.0, mood, None);
    }
    for act in [
        AgentStepActivity::Searching,
        AgentStepActivity::Browsing,
        AgentStepActivity::Reading,
        AgentStepActivity::Running,
        AgentStepActivity::Editing,
        AgentStepActivity::Delegating,
        AgentStepActivity::Working,
        AgentStepActivity::Reviewing,
    ] {
        let _div = render_plush_bot_avatar(
            "bot-activity",
            None,
            None,
            None,
            36.0,
            AvatarMood::Working,
            Some(act),
        );
    }

    // 1400ms ACTIVITY_HOLD_MS enforced through render_plush_bot_avatar_with_controller
    let mut ctrl = AgentAvatarController::new("bot-hold");
    let mut props = AgentAvatarProps::new("bot-hold")
        .with_size(46.0)
        .with_mood(AvatarMood::Working);
    props.activity = Some(AgentStepActivity::Searching);
    let _el0 = render_plush_bot_avatar_with_controller(&props, &mut ctrl, 1_000);
    assert_eq!(ctrl.held_activity, Some(AgentStepActivity::Searching));

    props.activity = Some(AgentStepActivity::Editing);
    let _el1 = render_plush_bot_avatar_with_controller(&props, &mut ctrl, 1_500);
    assert_eq!(ctrl.held_activity, Some(AgentStepActivity::Searching));

    let _el2 = render_plush_bot_avatar_with_controller(
        &props,
        &mut ctrl,
        1_000 + u64::from(ACTIVITY_HOLD_MS),
    );
    assert_eq!(ctrl.held_activity, Some(AgentStepActivity::Editing));

    // Legacy & escape-hatch dispatch in render_grok_bot_avatar & render_fabushi_shape_avatar
    for shape in [
        "blob",
        "pebble",
        "squircle",
        "tablet",
        "wedge",
        "hex",
        "cloud",
        "teardrop",
        "",
        "plush:nexo",
        "plush:poulpe",
        "bloub:squircle",
        "grok2d:blob",
    ] {
        let _g = render_grok_bot_avatar(
            GrokBotAvatarProps::new("bot-42", shape, "purple", 36.0)
                .with_state(GrokPersonaState::Searching),
        );
        let _f = render_fabushi_shape_avatar(shape, 0x8B5CF6, "Bot 42", 36.0);
    }

    // Felt status badges (all 5 states + 3-ball hopping curve)
    assert_eq!(working_ball_hop_sample(0, 0), (0.0, 7.0, 7.0));
    assert_eq!(working_ball_hop_sample(0, 1), (0.0, 7.0, 7.0));
    assert_eq!(working_ball_hop_sample(0, 2), (0.0, 7.0, 7.0));
    let (dy_peak, _, _) = working_ball_hop_sample(245, 0);
    assert!(dy_peak < -1.2 && dy_peak >= -1.76, "dy_peak={dy_peak}");
    let (dy_stagger, _, _) = working_ball_hop_sample(245, 2);
    assert!(dy_stagger > dy_peak, "ball 2 must lag behind ball 0");

    for state in [
        FeltStatusBadgeState::Working3Balls,
        FeltStatusBadgeState::UnreadPlum(5),
        FeltStatusBadgeState::WaitingAmber,
        FeltStatusBadgeState::RespondedGreen,
        FeltStatusBadgeState::ErrorRaspberry,
    ] {
        let _dark_b = render_felt_status_badge(state, 0x1A1A1A, true, 240);
        let _light_b = render_felt_status_badge(state, 0xF5F5F6, false, 0);
    }

    // ChannelAvatar across 0/1/2/3/4/5/13 members in Cluster & Row layouts + custom icon
    for n in [0usize, 1, 2, 3, 4, 5, 13] {
        let members: Vec<ChannelMember> = (0..n)
            .map(|idx| ChannelMember {
                agent_id: format!("ch-agent-{idx}"),
            })
            .collect();
        let briefs: Vec<ChannelAgentBrief> = (0..n)
            .map(|idx| ChannelAgentBrief {
                id: format!("ch-agent-{idx}"),
                avatar_seed: Some(format!("ch-agent-{idx}")),
                avatar_hue: None,
                avatar_color: None,
                avatar_url: None,
            })
            .collect();
        let _cluster = render_channel_avatar(
            &members,
            &briefs,
            ChannelAvatarLayout::Cluster,
            None,
            None,
            false,
            46.0,
            Some(SidebarPlushRingKind::Default),
            true,
            0.0,
        );
        let _row = render_channel_avatar(
            &members,
            &briefs,
            ChannelAvatarLayout::Row,
            None,
            None,
            false,
            18.0,
            None,
            true,
            0.0,
        );
    }
    let custom_icon = ChannelIcon {
        name: "Sparkles".to_string(),
        hue: AvatarHue::Blue.degrees(),
    };
    let _icon_tile = render_channel_avatar(
        &[],
        &[],
        ChannelAvatarLayout::Cluster,
        Some(&custom_icon),
        None,
        false,
        46.0,
        None,
        true,
        0.0,
    );
}

#[test]
fn m1_ui_f04_page_rail_width_pins_persistence_and_more_menu_highlight() {
    use gpui_telegram_desktop::views::apple_sidebar::{
        AppleAppSidebar, AppleSidebarSection, COLLAPSED_WIDTH, DEFAULT_PINNED_SECTIONS,
        EXPANDED_WIDTH, OPENBOT_PAGE_RAIL_WIDTH, PAGE_RAIL_PINS_FILE, PINNABLE_SECTIONS,
        load_page_rail_pins, sanitize_pinned_sections, save_page_rail_pins,
    };

    assert!((OPENBOT_PAGE_RAIL_WIDTH - 52.0).abs() < f32::EPSILON);
    assert!((EXPANDED_WIDTH - 52.0).abs() < f32::EPSILON);
    assert!((COLLAPSED_WIDTH - 52.0).abs() < f32::EPSILON);
    assert_eq!(DEFAULT_PINNED_SECTIONS.len(), 3);
    assert_eq!(DEFAULT_PINNED_SECTIONS[0], AppleSidebarSection::Workflows);
    assert_eq!(PINNABLE_SECTIONS.len(), 11);
    assert!(!AppleSidebarSection::Chats.is_pinnable());
    assert!(!AppleSidebarSection::Marketplace.is_pinnable());
    assert!(!AppleSidebarSection::McpTools.is_pinnable());
    assert!(!AppleSidebarSection::Schedule.is_pinnable());
    assert!(!AppleSidebarSection::Books.is_pinnable());
    assert!(!AppleSidebarSection::Games.is_pinnable());
    assert!(!AppleSidebarSection::AppStore.is_pinnable());

    for sec in PINNABLE_SECTIONS {
        assert!(sec.is_pinnable());
        assert!(!sec.localized_label("zh-CN").is_empty());
        assert!(!sec.localized_label("en").is_empty());
        assert!(sec.element_id().starts_with("apple-sidebar-nav-"));
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        load_page_rail_pins(tmp.path()),
        DEFAULT_PINNED_SECTIONS.to_vec()
    );

    let sanitized = sanitize_pinned_sections(vec![
        AppleSidebarSection::Chats,
        AppleSidebarSection::McpTools,
        AppleSidebarSection::Schedule,
        AppleSidebarSection::Workflows,
        AppleSidebarSection::Usage,
        AppleSidebarSection::Workflows,
    ]);
    assert_eq!(
        sanitized,
        vec![AppleSidebarSection::Workflows, AppleSidebarSection::Usage]
    );

    save_page_rail_pins(tmp.path(), &sanitized).expect("save pins");
    assert_eq!(load_page_rail_pins(tmp.path()), sanitized);

    // Explicit empty [] preserved on reload
    save_page_rail_pins(tmp.path(), &[]).expect("save empty pins");
    assert!(load_page_rail_pins(tmp.path()).is_empty());

    // Corrupt file recovers to defaults
    fs::write(tmp.path().join(PAGE_RAIL_PINS_FILE), b"corrupt{").expect("corrupt");
    assert_eq!(
        load_page_rail_pins(tmp.path()),
        DEFAULT_PINNED_SECTIONS.to_vec()
    );

    let mut rail = AppleAppSidebar::new_with_pins(
        TdesktopThemePalette::build_openbot(TdesktopThemeMode::Night, 0, 14, 18),
        "zh-CN",
        DEFAULT_PINNED_SECTIONS.to_vec(),
    );
    assert!((rail.effective_width() - 52.0).abs() < f32::EPSILON);
    assert!(!rail.current_in_more_menu());

    rail.set_highlighted_destination(Some(AppleSidebarSection::Workflows));
    assert!(!rail.current_in_more_menu());

    rail.set_highlighted_destination(Some(AppleSidebarSection::Library));
    assert!(rail.current_in_more_menu());

    rail.set_highlighted_destination(Some(AppleSidebarSection::Marketplace));
    assert!(!rail.current_in_more_menu());

    rail.set_narrow_hidden(true);
    assert!((rail.effective_width() - 0.0).abs() < f32::EPSILON);
    rail.set_narrow_hidden(false);
    assert!((rail.effective_width() - 52.0).abs() < f32::EPSILON);
}
