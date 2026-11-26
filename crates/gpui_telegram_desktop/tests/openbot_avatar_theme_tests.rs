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
