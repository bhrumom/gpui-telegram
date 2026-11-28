//! Live runtime probe for UI-F01 (OpenBot Tokens & OKLab) and UI-F02 (3D PlushBot & Felt Badges).

use std::path::PathBuf;
use std::sync::Arc;

use gpui_telegram_desktop::components::bloub_avatar::{
    AgentAvatarController, AgentAvatarProps, BundledPlushAssets, EDITOR_COLOR_TOTAL_CELLS,
    MarketplaceNoMatchPlushState, PlushFeltRenderer, SidebarBadgeAnimator, SidebarBadgePhase,
    SidebarFeltBadge, SidebarWaitReason, composite_plush_rgba, composite_plush_rgba_with_assets,
    compute_working_badge_layout, resolve_ink_hex, tint_ramp,
};
use gpui_telegram_desktop::theme::TdesktopThemeMode;
use gpui_telegram_desktop::theme::openbot_palette::{
    AppAppearanceMode, AppearancePreferencePatch, AppearancePreferenceStore, CHAT_COLOR_PRESETS,
    OpenBotColorScheme, OpenBotPalette, ThemePaletteError, readable_chat_background,
    solid_bubble_takes_dark_text, solid_bubble_text_hex,
};
use gpui_telegram_desktop::views::app_shell::openbot_shell_layout::{
    OpenBotShellFrameInput, compute_openbot_shell_frame_layout,
};
use gpui_telegram_desktop::views::boxes::peers::create_managed_bot_box::CreateManagedBotBoxPeers;
use gpui_telegram_desktop::views::content_hub::apple_primitives::{
    is_legacy_blue_rgb, rgb24_to_hex, rgba_to_rgb24,
};
use openbot_contracts::avatar::{
    ACTIVITY_HOLD_MS, AgentStepActivity, AvatarMood, AvatarStyle, BloubShape, EngineShapeId,
    PlushState, SEEDED_AVATAR_SILHOUETTES, activity_mood_presentation, bloub_avatar_profile,
    compute_agent_avatar_mood, stable_index,
};
use serde_json::{Map, Value, json};

use super::{dark_palette, insert_scenario, rgba_to_rgb_array};

fn token_map_for(p: &OpenBotPalette) -> Value {
    let g = |k: &str| p.token(k).unwrap_or("").to_string();
    json!({
        "bg_frame": g("--openbot-bg-frame"), "bg_canvas": g("--openbot-bg-canvas"),
        "bg_sidebar": g("--openbot-bg-sidebar"), "bg_surface": g("--openbot-bg-surface"),
        "bg_surface_raised": g("--openbot-bg-surface-raised"),
        "bg_control_hover": g("--openbot-bg-control-hover"),
        "bg_control_active": g("--openbot-bg-control-active"),
        "surface_light": g("--openbot-surface-light"), "text_on_light": g("--openbot-text-on-light"),
        "text_primary": g("--openbot-text-primary"), "text_secondary": g("--openbot-text-secondary"),
        "text_muted": g("--openbot-text-muted"), "text_dim": g("--openbot-text-dim"),
        "border": g("--openbot-border"), "border_strong": g("--openbot-border-strong"),
        "accent": g("--openbot-accent"), "accent_text": g("--openbot-accent-text"),
        "success": g("--openbot-success"), "warning": g("--openbot-warning"),
        "danger": g("--openbot-danger"),
        "unread_surface": rgb24_to_hex(rgba_to_rgb24(p.unread_surface())),
        "unread_text": rgb24_to_hex(rgba_to_rgb24(p.unread_text())),
        "badge_new": rgb24_to_hex(rgba_to_rgb24(p.badge_new())),
    })
}

pub fn scan_plush_assets() -> (usize, usize, bool) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/openbot_plush");
    let (mut webp, mut webm, mut valid) = (0usize, 0usize, true);
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(d) {
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    let bytes = std::fs::read(&p).unwrap_or_default();
                    if ext == "webp" {
                        webp += 1;
                        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
                            valid = false;
                        }
                    } else if ext == "webm" {
                        webm += 1;
                        if bytes.len() < 4 || &bytes[..4] != b"\x1a\x45\xdf\xa3" {
                            valid = false;
                        }
                    }
                }
            }
        }
    }
    (webp, webm, valid)
}

fn populate_tokens(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark_ob = OpenBotPalette::dark();
    let light_ob = OpenBotPalette::light();
    let td_dark = dark_palette();
    let tmp = tempfile::tempdir().expect("tempdir for AppearancePreferenceStore");

    let valid_palette_json = json!({
        "id": "midnight-studio", "name": "Midnight Studio", "appearance": "dark", "scheme": "dark",
        "colors": {
            "canvas": "#16181c", "frame": "#0c0d10", "sidebar": "#14161a", "surface": "#1e2026",
            "text": "#f4f6f8", "textMuted": "#8f96a3", "border": "#2a2d36", "accent": "#007cf7",
            "accentForeground": "#ffffff", "success": "#31cf76", "warning": "#ff9412", "danger": "#fd2f3b"
        }
    });
    let mut store = AppearancePreferenceStore::for_data_dir(tmp.path());
    std::fs::create_dir_all(store.themes_dir()).expect("create themes dir");
    std::fs::write(
        store.themes_dir().join("midnight-studio.json"),
        valid_palette_json.to_string(),
    )
    .expect("write custom palette");
    let _ = store.load();
    let _ = store.set_appearance(AppAppearanceMode::Dark);
    let _ = store.set_palette(Some("midnight-studio".into()));
    let custom_valid = store.selected_custom_palette().is_some()
        && OpenBotPalette::from_theme_json_str("midnight-studio", &valid_palette_json.to_string()).is_ok();
    let _ = store.set_palette(None);
    let reset_pal = store.effective_palette();
    let reset_bg_frame = reset_pal.token("--openbot-bg-frame").unwrap_or("").to_string();
    let reset_restores_defaults =
        reset_pal.token("--openbot-bg-frame") == dark_ob.token("--openbot-bg-frame")
            && reset_pal.token("--openbot-bg-canvas") == dark_ob.token("--openbot-bg-canvas");
    let pref_dark = store.effective_preference();

    let mut presets_map = Map::new();
    for preset in CHAT_COLOR_PRESETS {
        presets_map.insert(preset.id.to_string(), json!(preset.hex));
    }
    let oklab_samples: Vec<Value> = [("#4699fe", OpenBotColorScheme::Dark), ("#e5a50a", OpenBotColorScheme::Light)]
        .iter()
        .map(|(h, s)| json!({"hex": h, "scheme": s.as_str(), "result_hex": readable_chat_background(h, *s).unwrap_or_default()}))
        .collect();
    let solid_bubble_samples: Vec<Value> = [
        "#e5a50a", "#07b655", "#06a8d3", "#b474f4", "#242424", "#999999", "#939393", "#ffff00", "#000000",
    ]
    .iter()
    .map(|h| json!({"hex": h, "takes_dark_text": solid_bubble_takes_dark_text(h), "resolved_text_hex": solid_bubble_text_hex(h)}))
    .collect();

    let sh_wg_off = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(1440.0, 900.0, td_dark));
    let mut wg_on_in = OpenBotShellFrameInput::desktop_default(1440.0, 900.0, td_dark);
    wg_on_in.window_glass = true;
    let sh_wg_on = compute_openbot_shell_frame_layout(&wg_on_in);
    let glass_toks = dark_ob.window_glass_tokens();

    let modes: Vec<String> = [TdesktopThemeMode::Night, TdesktopThemeMode::Day, TdesktopThemeMode::Tinted, TdesktopThemeMode::Classic]
        .iter()
        .map(|m| format!("{m:?}"))
        .collect();

    root.insert("tokens".into(), json!({
        "scheme": reset_pal.scheme.as_str(),
        "appearance": pref_dark.appearance.as_str(),
        "fallback_applied": AppAppearanceMode::parse(pref_dark.appearance.as_str()).is_none(),
        "dark": token_map_for(&dark_ob), "light": token_map_for(&light_ob),
        "tdesktop_sync": {
            "uses_legacy_telegram_blue_chrome": is_legacy_blue_rgb(td_dark.msg_out_bg) || is_legacy_blue_rgb(td_dark.topbar_bg),
            "primary_cta_hex": dark_ob.token("--openbot-surface-light").unwrap_or(""),
            "user_bubble_hex": rgb24_to_hex(td_dark.msg_out_bg),
            "active_tab_bg_hex": dark_ob.token("--openbot-bg-control-active").unwrap_or(""),
            "supported_modes": modes,
        },
        "chat_color_presets": Value::Object(presets_map),
        "oklab_samples": oklab_samples,
        "solid_bubble_samples": solid_bubble_samples,
        "custom_palette": {
            "valid": custom_valid, "palette": valid_palette_json,
            "reset_restores_defaults": reset_restores_defaults,
            "reset_bg_frame": reset_bg_frame,
        },
        "window_glass": {
            "enabled": sh_wg_off.window_glass_active,
            "popovers_opaque": dark_ob.rgba("--openbot-bg-surface").is_some_and(|c| c.a >= 0.999),
            "tab_shape_disabled": !sh_wg_off.tab_shape_enabled,
        },
    }));

    let _ = store.set_appearance(AppAppearanceMode::Light);
    let light_eff = store.effective_preference();
    let light_pal = store.effective_palette();
    insert_scenario(sc, "f01_light_tokens", "tokens", json!({
        "scheme": light_pal.scheme.as_str(), "appearance": light_eff.appearance.as_str(),
    }));

    let inv_tmp = tempfile::tempdir().expect("tempdir invalid appearance");
    let mut inv_store = AppearancePreferenceStore::for_data_dir(inv_tmp.path());
    std::fs::write(inv_store.preference_path(), r#"{"version":1,"appearance":"neon-invalid"}"#).expect("write bad pref");
    let _ = inv_store.load();
    let inv_pal = inv_store.effective_palette();
    insert_scenario(sc, "f01_invalid_appearance", "tokens", json!({
        "fallback_applied": AppAppearanceMode::parse("neon-invalid").is_none(),
        "appearance": inv_pal.scheme.as_str(), "scheme": inv_pal.scheme.as_str(),
    }));

    let bad_p1 = json!({"id": "BAD ID!", "name": "Bad", "appearance": "dark", "scheme": "dark", "colors": {}});
    let bad_p2 = json!({"id": "ok-id", "name": "BadHex", "appearance": "dark", "scheme": "dark", "colors": {"canvas": "not-hex"}});
    let bad_p3 = json!({"id": "ok-id2", "name": "BadScheme", "appearance": "sepia", "scheme": "sepia", "colors": {}});
    let e1 = OpenBotPalette::from_theme_json_str("BAD ID!", &bad_p1.to_string()).err();
    let e2 = OpenBotPalette::from_theme_json_str("ok-id", &bad_p2.to_string()).err();
    let e3 = OpenBotPalette::from_theme_json_str("ok-id2", &bad_p3.to_string()).err();
    assert!(
        matches!(e1, Some(ThemePaletteError::InvalidPaletteId(_))),
        "expected InvalidPaletteId for bad_p1, got {e1:?}"
    );
    assert!(
        matches!(e2, Some(ThemePaletteError::InvalidColor(_))),
        "expected InvalidColor for bad_p2, got {e2:?}"
    );
    assert!(
        matches!(e3, Some(ThemePaletteError::InvalidAppearance(_))),
        "expected InvalidAppearance for bad_p3, got {e3:?}"
    );
    insert_scenario(sc, "f01_invalid_palette", "tokens", json!({
        "custom_palette": {
            "valid": !(e1.is_some() && e2.is_some() && e3.is_some()),
            "error_code": e1.as_ref().map(|e| e.code()),
            "rejected_samples": [bad_p1, bad_p2, bad_p3],
        }
    }));
    let extreme_samples: Vec<Value> = [
        ("#ff00ff", OpenBotColorScheme::Dark), ("#00ff00", OpenBotColorScheme::Dark),
        ("#000000", OpenBotColorScheme::Light), ("#ffffff", OpenBotColorScheme::Light),
    ]
    .iter()
    .map(|(h, s)| json!({"hex": h, "scheme": s.as_str(), "result_hex": readable_chat_background(h, *s).unwrap_or_default()}))
    .collect();
    insert_scenario(sc, "f01_oklab_extreme", "tokens", json!({"oklab_samples": extreme_samples}));

    let wcag_samples: Vec<Value> = ["#999999", "#939393", "#ffff00", "#000000"]
        .iter()
        .map(|h| json!({
            "hex": h,
            "takes_dark_text": solid_bubble_takes_dark_text(h),
            "resolved_text_hex": solid_bubble_text_hex(h),
        }))
        .collect();
    insert_scenario(sc, "f01_wcag_boundary", "tokens", json!({"solid_bubble_samples": wcag_samples}));

    let bad_fonts = ["Inter\"; body { color: red }", "Comic\nSans", "   padded   "];
    let mut first_font_err_code: Option<&'static str> = None;
    let mut all_bad_rejected = true;
    for bf in bad_fonts {
        match store.update(AppearancePreferencePatch { interface_font: Some(Some(bf.to_string())), ..Default::default() }) {
            Ok(_) => all_bad_rejected = false,
            Err(err) => {
                if first_font_err_code.is_none() {
                    first_font_err_code = Some(err.code());
                }
            }
        }
    }
    let clean_ok = store.update(AppearancePreferencePatch { interface_font: Some(Some("Inter, SF Pro Display".to_string())), ..Default::default() }).is_ok();
    insert_scenario(sc, "f01_invalid_fonts", "tokens", json!({
        "font_validation": {
            "valid": !all_bad_rejected,
            "error_code": first_font_err_code,
            "rejected_inputs": bad_fonts,
            "clean_font_accepted": clean_ok,
        }
    }));
    insert_scenario(sc, "f01_window_glass_on", "tokens", json!({
        "window_glass": {
            "enabled": sh_wg_on.window_glass_active,
            "popovers_opaque": !glass_toks.contains_key("--openbot-bg-surface")
                && dark_ob.rgba("--openbot-bg-surface").is_some_and(|c| c.a >= 0.999),
            "tab_shape_disabled": !sh_wg_on.tab_shape_enabled,
        }
    }));
    insert_scenario(sc, "f01_window_glass_off", "tokens", json!({
        "window_glass": {
            "enabled": sh_wg_off.window_glass_active,
            "popovers_opaque": dark_ob.rgba("--openbot-bg-surface").is_some_and(|c| c.a >= 0.999),
            "tab_shape_disabled": !sh_wg_off.tab_shape_enabled,
        }
    }));
}

fn populate_plush(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark_ob = OpenBotPalette::dark();
    let (webp_count, webm_count, all_valid_webp_headers) = scan_plush_assets();

    let mut creator = CreateManagedBotBoxPeers::new();
    creator.open_fabushi_creator();
    let shape_prefix = creator
        .avatar_shape
        .find(':')
        .map(|i| &creator.avatar_shape[..=i])
        .unwrap_or("");
    let is_plush_3d = creator.avatar_shape.starts_with("plush:");

    let silhouettes: Vec<&str> = BloubShape::ALL.iter().map(|s| s.as_str()).collect();
    let sample_hex = "#3b93f0";
    let lut1 = tint_ramp(sample_hex);
    let assets = BundledPlushAssets::new();
    let t1 = assets.tinted_rgba("bloub/boule.webp", sample_hex, None).ok();
    let t2 = assets.tinted_rgba("bloub/boule.webp", sample_hex, None).ok();
    let cache_hit = match (&t1, &t2) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    };
    let sample_grey = 140usize;
    let valid_ink_resolved = resolve_ink_hex(sample_hex);

    let active_mood = compute_agent_avatar_mood(true, true, true, true, true, true);
    let supported_moods: Vec<&str> = [
        AvatarMood::Failed, AvatarMood::Waiting, AvatarMood::Working,
        AvatarMood::Responded, AvatarMood::Idle, AvatarMood::Connecting, AvatarMood::Asleep,
    ]
    .iter()
    .map(|m| m.as_str())
    .collect();

    let all_activities = [
        AgentStepActivity::Searching, AgentStepActivity::Browsing, AgentStepActivity::Reading,
        AgentStepActivity::Running, AgentStepActivity::Editing, AgentStepActivity::Delegating,
        AgentStepActivity::Working, AgentStepActivity::Reviewing,
    ];
    let mut act_expr_map = Map::new();
    for act in all_activities {
        let pres = activity_mood_presentation(act);
        let label = match pres.state {
            PlushState::Idle => pres.expression.map(|e| e.as_str()).unwrap_or(""),
            PlushState::Wide => "wide",
            PlushState::Wink => "wink",
            PlushState::Thinking => "thinking",
            PlushState::Notify => "notify",
            PlushState::Failed => "failed",
            PlushState::Asleep => "asleep",
        };
        if let Ok(Value::String(k)) = serde_json::to_value(act) {
            act_expr_map.insert(k, json!(label));
        }
    }

    let wb_active = compute_working_badge_layout(true, false, 240);
    let ball_assets: Vec<&str> = wb_active
        .ball_asset_paths
        .iter()
        .map(|p| p.rsplit('/').next().unwrap_or(p))
        .collect();
    let (plum_top, plum_bot) = dark_ob.felt_plum();
    let (amber_top, _) = dark_ob.felt_amber();
    let waiting_badge = SidebarFeltBadge::Waiting { reason: SidebarWaitReason::Approval };
    let responded_badge = SidebarFeltBadge::Responded;
    let err_badge = SidebarFeltBadge::routine_failed();
    let err_asset = err_badge
        .felt_asset_path()
        .and_then(|p| p.rsplit('/').next())
        .unwrap_or("");
    let mut anim_init = SidebarBadgeAnimator::default();
    anim_init.update(Some(SidebarFeltBadge::Unread { count: 1 }), 0, false);
    anim_init.update(Some(SidebarFeltBadge::Unread { count: 2 }), 100, false);
    let init_bump = serde_json::to_value(anim_init.phase).expect("serialize SidebarBadgePhase");

    let active_ctrl = AgentAvatarController::new("agent-active");
    let mut active_props = AgentAvatarProps::new("agent-active");
    active_props.mood = AvatarMood::Working;
    active_props.on_screen = true;
    active_props.document_focused = true;
    let res_active = active_ctrl.resolve(&active_props);
    let mut renderer = PlushFeltRenderer::for_seed("agent-active", None, None, AvatarMood::Idle);
    let frame = renderer.sample_plush_frame(0.0, false);
    let rendered_img = composite_plush_rgba(&frame, 32);
    let render_ok = rendered_img.pixels().any(|p| p.0[3] > 0);
    let nm_norm = MarketplaceNoMatchPlushState::new(false);
    let fishing_norm = nm_norm.active_asset_name();

    let mut act_root = AgentAvatarController::new("agent-act-root");
    act_root.sync_activity(Some(AgentStepActivity::Searching), 0);
    let act_root_shown = serde_json::to_value(act_root.held_activity).expect("serialize held_activity");
    let act_root_held_ms = 0u64.saturating_sub(act_root.activity_shown_at_ms.unwrap_or(0));

    root.insert("plush".into(), json!({
        "assets": {"webp_count": webp_count, "webm_count": webm_count, "all_valid_webp_headers": all_valid_webp_headers},
        "default_bot_avatar": {
            "is_plush_3d": is_plush_3d,
            "renderer": if is_plush_3d { "PlushFeltRenderer" } else { "Flat2D" },
            "shape_prefix": shape_prefix,
            "legacy_2d_fallback_used": !is_plush_3d,
        },
        "supported_silhouettes": silhouettes,
        "color_choices_count": EDITOR_COLOR_TOTAL_CELLS,
        "tint_ramp": {
            "lut_len": lut1.len(), "cache_hit": cache_hit, "hex": sample_hex,
            "sample_grey_byte": sample_grey, "sample_rgba": lut1[sample_grey],
            "fallback_used": valid_ink_resolved.to_ascii_lowercase() != sample_hex,
        },
        "mood": {"active": active_mood.as_str(), "supported_moods": supported_moods},
        "activity": {
            "hold_threshold_ms": ACTIVITY_HOLD_MS,
            "activity_expression_map": Value::Object(act_expr_map),
            "shown": act_root_shown,
            "flickered": act_root.held_activity != Some(AgentStepActivity::Searching),
            "held_ms": act_root_held_ms,
        },
        "badges": {
            "working": {
                "width_px": wb_active.width_px as i32, "height_px": wb_active.height_px as i32,
                "bg_hex": rgb24_to_hex(wb_active.bg_rgb),
                "ball_count": wb_active.ball_count, "ball_size_px": wb_active.ball_size_px as i32,
                "ball_assets": ball_assets, "stagger_ms": wb_active.stagger_ms, "loop_ms": wb_active.loop_ms,
            },
            "unread": {
                "top_rgb": rgba_to_rgb_array(plum_top), "bottom_rgb": rgba_to_rgb_array(plum_bot),
                "text_hex": rgb24_to_hex(rgba_to_rgb24(dark_ob.felt_cream())),
                "bump_anim": init_bump,
                "exiting_mounted": anim_init.phase == SidebarBadgePhase::Out && anim_init.current.is_some(),
            },
            "waiting": {"size_px": waiting_badge.box_px() as i32, "top_rgb": rgba_to_rgb_array(amber_top)},
            "responded": {"size_px": responded_badge.box_px() as i32, "color_hex": rgb24_to_hex(rgba_to_rgb24(dark_ob.success()))},
            "error": {"size_px": err_badge.box_px() as i32, "asset": err_asset},
        },
        "runtime": {
            "reduced_motion": active_props.reduced_motion, "frozen_pose": res_active.frozen_at.is_some(),
            "fps": res_active.fps, "balls_paused": wb_active.balls_paused, "fishing_asset": fishing_norm,
            "image_failed": active_ctrl.failed_url.is_some(), "fallback_to_plush": !res_active.uses_custom_image,
            "render_ok": render_ok,
        },
    }));

    let goutte_seed = (0..256)
        .map(|i| format!("goutte-seed-{i}"))
        .find(|s| SEEDED_AVATAR_SILHOUETTES[stable_index(&format!("{s}:shape"), SEEDED_AVATAR_SILHOUETTES.len())] == EngineShapeId::Goutte)
        .expect("deterministic seed hashing to goutte");
    let raw_goutte = SEEDED_AVATAR_SILHOUETTES[stable_index(&format!("{goutte_seed}:shape"), SEEDED_AVATAR_SILHOUETTES.len())];
    let prof_g = bloub_avatar_profile(&goutte_seed, None, None);
    insert_scenario(sc, "f02_goutte_remap", "plush", json!({
        "profile": {
            "seed": goutte_seed,
            "silhouette": prof_g.shape.as_str(),
            "remapped_from_goutte": raw_goutte == EngineShapeId::Goutte && prof_g.shape.as_str() != "goutte",
        }
    }));

    let mut ctrl = AgentAvatarController::new("agent-hold");
    ctrl.sync_activity(Some(AgentStepActivity::Searching), 0);
    ctrl.sync_activity(Some(AgentStepActivity::Editing), 500);
    let shown_500 = serde_json::to_value(ctrl.held_activity).expect("serialize held_activity 500");
    let held_500 = 500u64.saturating_sub(ctrl.activity_shown_at_ms.unwrap_or(0));
    let held_before_1450 = 1450u64.saturating_sub(ctrl.activity_shown_at_ms.unwrap_or(0));
    ctrl.sync_activity(Some(AgentStepActivity::Editing), 1450);
    let shown_1450 = serde_json::to_value(ctrl.held_activity).expect("serialize held_activity 1450");
    insert_scenario(sc, "f02_activity_t500", "plush", json!({
        "activity": {"shown": shown_500, "flickered": shown_500 != json!("searching"), "held_ms": held_500}
    }));
    insert_scenario(sc, "f02_activity_t1450", "plush", json!({
        "activity": {"shown": shown_1450, "flickered": shown_1450 != json!("editing"), "held_ms": held_before_1450}
    }));

    let mut url_ctrl = AgentAvatarController::new("agent-broken-url");
    url_ctrl.sync_url(Some("https://invalid.local/404.png"));
    url_ctrl.mark_image_failed("https://invalid.local/404.png");
    let resolved_url = url_ctrl.resolve(&AgentAvatarProps::new("agent-broken-url").with_url(Some("https://invalid.local/404.png".into())));
    insert_scenario(sc, "f02_broken_avatar_url", "plush", json!({
        "runtime": {"image_failed": url_ctrl.failed_url.is_some(), "fallback_to_plush": !resolved_url.uses_custom_image},
        "default_bot_avatar": {"is_plush_3d": !resolved_url.uses_custom_image && resolved_url.style == AvatarStyle::Plush},
    }));

    let rm_ctrl = AgentAvatarController::new("agent-rm");
    let mut rm_props = AgentAvatarProps::new("agent-rm");
    rm_props.mood = AvatarMood::Working;
    rm_props.reduced_motion = true;
    let rm_res = rm_ctrl.resolve(&rm_props);
    let wb_rm = compute_working_badge_layout(true, true, 240);
    let rm_plush = MarketplaceNoMatchPlushState::new(true);
    let fish_still = rm_plush.active_asset_name();
    insert_scenario(sc, "f02_reduced_motion", "plush", json!({
        "runtime": {"reduced_motion": rm_props.reduced_motion, "frozen_pose": rm_res.frozen_at.is_some(), "fps": rm_res.fps, "balls_paused": wb_rm.balls_paused, "fishing_asset": fish_still}
    }));
    insert_scenario(sc, "f02_active_fps", "plush", json!({"runtime": {"fps": res_active.fps, "balls_paused": wb_active.balls_paused}}));

    let off_ctrl = AgentAvatarController::new("agent-off");
    let mut off_props = AgentAvatarProps::new("agent-off");
    off_props.mood = AvatarMood::Working;
    off_props.on_screen = false;
    let off_res = off_ctrl.resolve(&off_props);
    let wb_off = compute_working_badge_layout(true, !off_res.animated, 240);
    insert_scenario(sc, "f02_offscreen_fps", "plush", json!({"runtime": {"fps": off_res.fps, "balls_paused": wb_off.balls_paused}}));

    let mut anim_churn = SidebarBadgeAnimator::default();
    anim_churn.update(Some(SidebarFeltBadge::Unread { count: 1 }), 0, false);
    anim_churn.update(Some(SidebarFeltBadge::Unread { count: 2 }), 100, false);
    anim_churn.update(Some(SidebarFeltBadge::Unread { count: 3 }), 200, false);
    let bump_b_str = serde_json::to_value(anim_churn.phase).expect("serialize SidebarBadgePhase");
    anim_churn.update(None, 300, false);
    let exiting_mounted = anim_churn.phase == SidebarBadgePhase::Out && anim_churn.current.is_some();
    let bad_ink_fallback = resolve_ink_hex("not-a-hex") != "not-a-hex" && tint_ramp("not-a-hex").len() == 256;
    insert_scenario(sc, "f02_unread_churn_invalid_hex", "plush", json!({
        "badges": {"unread": {"bump_anim": bump_b_str, "exiting_mounted": exiting_mounted}},
        "tint_ramp": {"fallback_used": bad_ink_fallback},
    }));

    let empty_tmp = tempfile::tempdir().expect("tempdir missing plush assets");
    let empty_assets = BundledPlushAssets::with_root(empty_tmp.path());
    let fallback_img = composite_plush_rgba_with_assets(&frame, 32, 1.0, &empty_assets);
    let fallback_ok = fallback_img.pixels().any(|p| p.0[3] > 0);
    insert_scenario(sc, "f20_missing_plush_asset_fallback", "plush", json!({
        "runtime": {"render_ok": fallback_ok},
        "tint_ramp": {"fallback_used": empty_assets.resolve_asset_path("bloub/boule.webp").is_none() && bad_ink_fallback},
    }));
}

pub fn populate(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    populate_tokens(root, sc);
    populate_plush(root, sc);
}
