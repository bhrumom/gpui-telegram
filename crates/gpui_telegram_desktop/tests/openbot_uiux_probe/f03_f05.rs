//! Live runtime probe for UI-F03 (Inset Edge-Framed Shell), UI-F04 (52px PageRail), and UI-F05 (SidebarTopbar/Search/+).

use gpui_telegram_desktop::theme::TdesktopThemePalette;
use gpui_telegram_desktop::views::app_shell::openbot_shell_layout::{
    ChatNavHistory, OpenBotShellFrameInput, apply_cmd_b_sidebar_toggle,
    compute_openbot_shell_frame_layout, compute_shell_account_footer_audit,
    configure_openbot_sidebar_for_shell, default_openbot_shell_wiring,
};
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleAppSidebar, AppleSidebarSection, OPENBOT_PAGE_RAIL_WIDTH, PINNABLE_SECTIONS,
    RAIL_MORE_MENU_GUTTER_PX, RAIL_MORE_MENU_PLACEMENT, load_page_rail_hidden_items,
    load_page_rail_pins, save_page_rail_prefs,
};
use gpui_telegram_desktop::views::chat_sidebar::{
    ChatSidebar, DEFAULT_WIDTH as LEGACY_SIDEBAR_DEFAULT_WIDTH, DialogDeliveryStatus,
    DialogItemModel, OPENBOT_SIDEBAR_COLLAPSE_THRESHOLD, OPENBOT_SIDEBAR_DEFAULT_WIDTH,
    OPENBOT_SIDEBAR_EXPAND_THRESHOLD, OPENBOT_SIDEBAR_MAX_WIDTH, OPENBOT_SIDEBAR_MIN_WIDTH,
};
use gpui_telegram_desktop::views::content_hub::ContentHubSection;
use gpui_telegram_desktop::views::openbot::conversation::CONVERSATION_MIN_WIDTH_PX;
use gpui_telegram_desktop::views::openbot::shell_state::{
    ServerPlatform, ServerRailState, ServerViewMode, UsageRingMetric,
};
use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
use gpui_telegram_desktop::views::sidebar::dialogs_overlays::compute_openbot_sidebar_tabs_layout;
use gpui_telegram_desktop::views::sidebar::openbot_sidebar_chrome::{
    compute_openbot_sidebar_search_row_layout, compute_openbot_sidebar_topbar_layout,
};
use openbot_contracts::providers::AgentProviderId;
use serde_json::{Map, Value, json};

use super::{dark_palette, insert_scenario, light_palette};

fn sec_id(s: AppleSidebarSection) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn sb_mode_str(sb: &ChatSidebar) -> &'static str {
    if sb.is_sidebar_closed {
        "closed"
    } else if sb.is_collapsed {
        "compact"
    } else {
        "expanded"
    }
}

fn shell_json_from_input(input: &OpenBotShellFrameInput, srv: &ServerRailState, platform: ServerPlatform) -> Value {
    let f = compute_openbot_shell_frame_layout(input);
    let srv_w = srv.effective_width_px(platform, input.viewport_w.max(0.0) as u32);
    json!({
        "outer_frame_bg_hex": f.frame_bg_hex,
        "top_strip_height_px": f.top_chrome_height_px as i32,
        "canvas_card": {
            "bg_hex": f.canvas_bg_hex,
            "radius_px": f.workspace_card_radius_px as i32,
            "border_px": f.workspace_card_border_px,
            "inset_right_px": f.workspace_margin_px as i32,
            "inset_bottom_px": f.workspace_margin_px as i32,
            "full_bleed": f.narrow_viewport,
        },
        "rail_visible": f.page_rail_visible,
        "rail_width_px": f.page_rail_width_px as i32,
        "sidebar_visible": f.sidebar_rect.width > 0.0,
        "sidebar_width_px": f.sidebar_rect.width as i32,
        "conversation_min_width_px": CONVERSATION_MIN_WIDTH_PX as i32,
        "conversation_rect": {
            "x": f.stage_column_rect.x, "y": f.stage_column_rect.y,
            "w": f.stage_column_rect.width, "h": f.stage_column_rect.height,
        },
        "right_panel_count": f.right_drawer.active_right_drawer_count,
        "page_kind": f.page_kind.as_str(),
        "top_row_tabs_start_px": f.top_row_tabs_start_px as i32,
        "window_glass_active": f.window_glass_active,
        "tab_shape_enabled": f.tab_shape_enabled,
        "server_rail": {"view_mode": srv.view_mode.as_str(), "visible": srv_w > 0.0, "width_px": srv_w as i32},
    })
}

fn make_input(
    vw: f32,
    vh: f32,
    pal: TdesktopThemePalette,
    panel: ActiveWorkspacePanel,
    sb_closed: bool,
    sb_w: f32,
    window_glass: bool,
) -> OpenBotShellFrameInput {
    let mut inp = OpenBotShellFrameInput::desktop_default(vw, vh, pal);
    inp.active_panel = panel;
    inp.sidebar_closed = sb_closed;
    inp.sidebar_width_px = sb_w;
    inp.window_glass = window_glass;
    inp
}

fn populate_shell_and_rail(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let light = light_palette();
    let dark_ob = dark.openbot_palette();
    let srv_default = ServerRailState::new();

    root.insert("shell".into(), shell_json_from_input(
        &make_input(1440.0, 900.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_light_shell", "shell", shell_json_from_input(
        &make_input(1440.0, 900.0, light, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_page_agents", "shell", shell_json_from_input(
        &make_input(1440.0, 900.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_page_titled", "shell", shell_json_from_input(
        &make_input(1440.0, 900.0, dark, ActiveWorkspacePanel::ContentHub(ContentHubSection::Books), false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_page_settings", "shell", shell_json_from_input(
        &make_input(1440.0, 900.0, dark, ActiveWorkspacePanel::Settings, true, 0.0, false),
        &srv_default, ServerPlatform::Linux,
    ));

    let mut sr_rail = ServerRailState::new();
    sr_rail.view_mode = ServerViewMode::Rail;
    for (sc_id, plat, vp_w) in [
        ("f03_server_rail_darwin", ServerPlatform::MacOs, 1440.0_f32),
        ("f03_server_rail_linux", ServerPlatform::Linux, 1440.0),
        ("f03_server_rail_801", ServerPlatform::Linux, 801.0),
        ("f03_server_rail_800", ServerPlatform::Linux, 800.0),
    ] {
        let w = sr_rail.effective_width_px(plat, vp_w as u32);
        insert_scenario(sc, sc_id, "shell", json!({
            "server_rail": {"view_mode": sr_rail.view_mode.as_str(), "visible": w > 0.0, "width_px": w as i32}
        }));
    }
    insert_scenario(sc, "f03_sidebar_closed", "shell", shell_json_from_input(
        &make_input(1440.0, 900.0, dark, ActiveWorkspacePanel::Chats, true, 0.0, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_viewport_720", "shell", shell_json_from_input(
        &make_input(720.0, 800.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_viewport_721", "shell", shell_json_from_input(
        &make_input(721.0, 800.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_viewport_320", "shell", shell_json_from_input(
        &make_input(320.0, 600.0, dark, ActiveWorkspacePanel::Chats, true, 0.0, false),
        &srv_default, ServerPlatform::Linux,
    ));
    for w in [1440.0_f32, 720.0, 500.0, 721.0] {
        let _ = compute_openbot_shell_frame_layout(&make_input(w, 800.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false));
    }
    insert_scenario(sc, "f03_resize_sweep_restored", "shell", shell_json_from_input(
        &make_input(1280.0, 800.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_window_glass", "shell", shell_json_from_input(
        &make_input(1440.0, 900.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, true),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_viewport_0x0", "shell", shell_json_from_input(
        &make_input(0.0, 0.0, dark, ActiveWorkspacePanel::Chats, true, 0.0, false),
        &srv_default, ServerPlatform::Linux,
    ));
    insert_scenario(sc, "f03_viewport_4k", "shell", shell_json_from_input(
        &make_input(3840.0, 2160.0, dark, ActiveWorkspacePanel::Chats, false, OPENBOT_SIDEBAR_DEFAULT_WIDTH, false),
        &srv_default, ServerPlatform::Linux,
    ));

    // UI-F04 PageRail
    let rail = AppleAppSidebar::new(dark, "zh-CN");
    let mut sb_wired = ChatSidebar::new();
    configure_openbot_sidebar_for_shell(&mut sb_wired, default_openbot_shell_wiring());
    let footer_audit = compute_shell_account_footer_audit(&rail, &sb_wired);
    let rl = rail.compute_layout(900.0);
    let mut top_items: Vec<String> = vec!["chats".into()];
    top_items.extend(rail.pinned_sections.iter().copied().map(sec_id));
    top_items.push("more".into());
    let all_more_entries: Vec<String> = PINNABLE_SECTIONS.iter().copied().map(sec_id).collect();
    let pinned_ids: Vec<String> = rail.pinned_sections.iter().copied().map(sec_id).collect();
    let hid_init: Vec<&str> = rail.hidden_bottom_items.iter().map(|i| i.as_str()).collect();
    let mut rail_chk_chats = AppleAppSidebar::new(dark, "zh-CN");
    let chats_unpin_res = rail_chk_chats.toggle_pin_local(AppleSidebarSection::Chats);
    root.insert("page_rail".into(), json!({
        "visible": rl.visible, "width_px": OPENBOT_PAGE_RAIL_WIDTH as i32,
        "padding_top_px": rl.padding_top_px as i32, "padding_bottom_px": rl.padding_bottom_px as i32,
        "gap_px": rl.gap_px as i32, "button_size_px": rl.button_size_px as i32,
        "button_radius_px": rl.button_radius_px as i32, "icon_size_px": rl.icon_size_px as i32,
        "icon_stroke_px": rl.icon_stroke_px, "top_items": top_items,
        "pinned_items": pinned_ids, "persisted_pins": pinned_ids,
        "active_btn_bg_hex": dark_ob.token("--openbot-bg-control-active").unwrap_or(""),
        "more_menu": {"open": rail.more_menu_open, "placement": RAIL_MORE_MENU_PLACEMENT, "gutter_px": RAIL_MORE_MENU_GUTTER_PX as i32, "entries": all_more_entries},
        "bottom_items": {
            "usage_ring_slots": rl.bottom_items.usage_ring_slots,
            "collapsed_more_rings_count": rl.bottom_items.collapsed_more_rings_count,
            "min_free_px": rl.bottom_items.min_free_px as i32,
            "help_visible": rl.bottom_items.help_visible, "settings_visible": rl.bottom_items.settings_visible,
            "update_dot_px": rl.bottom_items.update_dot_px as i32, "update_dot_hex": rl.bottom_items.update_dot_hex,
            "divider_px": [rl.bottom_items.divider_w_px as i32, rl.bottom_items.divider_h_px as i32],
            "account_tile_px": rl.bottom_items.account_tile_px as i32,
            "account_tile_is_plush": rl.bottom_items.account_tile_is_plush,
        },
        "total_account_footers_in_shell": footer_audit.total_account_footers_in_shell,
        "chats_unpinnable": chats_unpin_res,
        "hidden_bottom_items": hid_init, "persisted_hidden": hid_init,
        "undo_toast_visible": rail.undo_toast_item.is_some(),
    }));
}

fn populate_rail_scenarios_and_sidebar_chrome(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let all_more_entries: Vec<String> = PINNABLE_SECTIONS.iter().copied().map(sec_id).collect();
    let tmp = tempfile::tempdir().expect("tempdir for rail & sidebar prefs");

    let mut rail_pin = AppleAppSidebar::new(dark, "zh-CN");
    rail_pin.toggle_more_menu();
    rail_pin.toggle_pin_local(AppleSidebarSection::MyCreations);
    rail_pin.toggle_pin_local(AppleSidebarSection::Workflows);
    save_page_rail_prefs(tmp.path(), &rail_pin.pinned_sections, &rail_pin.hidden_bottom_items)
        .expect("save rail pins");
    let reloaded_pins: Vec<String> = load_page_rail_pins(tmp.path()).into_iter().map(sec_id).collect();
    insert_scenario(sc, "f04_more_menu_pin", "page_rail", json!({
        "more_menu": {"open": rail_pin.more_menu_open, "placement": RAIL_MORE_MENU_PLACEMENT, "gutter_px": RAIL_MORE_MENU_GUTTER_PX as i32, "entries": all_more_entries},
        "persisted_pins": reloaded_pins,
    }));

    let mut rail_hide = AppleAppSidebar::new(dark, "zh-CN");
    rail_hide.hide_bottom_item_by_id("help");
    save_page_rail_prefs(tmp.path(), &rail_hide.pinned_sections, &rail_hide.hidden_bottom_items)
        .expect("save hidden bottom");
    let rl_h = rail_hide.compute_layout(900.0);
    let hid_ids: Vec<&str> = rail_hide.hidden_bottom_items.iter().map(|i| i.as_str()).collect();
    let persisted_hid: Vec<&str> = load_page_rail_hidden_items(tmp.path()).into_iter().map(|i| i.as_str()).collect();
    insert_scenario(sc, "f04_hide_help", "page_rail", json!({
        "hidden_bottom_items": hid_ids, "persisted_hidden": persisted_hid,
        "bottom_items": {"help_visible": rl_h.bottom_items.help_visible},
        "undo_toast_visible": rail_hide.undo_toast_item.is_some(),
    }));
    rail_hide.undo_hide_bottom_item();
    save_page_rail_prefs(tmp.path(), &rail_hide.pinned_sections, &rail_hide.hidden_bottom_items)
        .expect("save undone hidden bottom");
    let rl_u = rail_hide.compute_layout(900.0);
    let hid_after: Vec<&str> = rail_hide.hidden_bottom_items.iter().map(|i| i.as_str()).collect();
    let persisted_after: Vec<&str> = load_page_rail_hidden_items(tmp.path()).into_iter().map(|i| i.as_str()).collect();
    insert_scenario(sc, "f04_undo_hide_help", "page_rail", json!({
        "hidden_bottom_items": hid_after, "persisted_hidden": persisted_after,
        "bottom_items": {"help_visible": rl_u.bottom_items.help_visible},
        "undo_toast_visible": rail_hide.undo_toast_item.is_some(),
    }));

    let mut rail_unpin_chats = AppleAppSidebar::new(dark, "zh-CN");
    let unpin_chats_ok = rail_unpin_chats.toggle_pin_local(AppleSidebarSection::Chats);
    let mut unpin_chats_top: Vec<String> = vec!["chats".into()];
    unpin_chats_top.extend(rail_unpin_chats.pinned_sections.iter().copied().map(sec_id));
    unpin_chats_top.push("more".into());
    insert_scenario(sc, "f04_unpin_chats_attempt", "page_rail", json!({
        "chats_unpinnable": unpin_chats_ok,
        "top_items": unpin_chats_top,
    }));

    let mut rail_short = AppleAppSidebar::new(dark, "zh-CN");
    rail_short.set_usage_ring_metrics((0..5).map(|i| UsageRingMetric { provider: AgentProviderId::Codex, label: format!("P{i}"), primary_ratio: Some(0.4), secondary_ratio: None, unavailable: false, reset_label: None, detail_summary: None }).collect());
    let rl_s = rail_short.compute_layout(360.0);
    insert_scenario(sc, "f04_short_360px_rings", "page_rail", json!({
        "bottom_items": {"min_free_px": rl_s.bottom_items.min_free_px as i32, "usage_ring_slots": rl_s.bottom_items.usage_ring_slots, "collapsed_more_rings_count": rl_s.bottom_items.collapsed_more_rings_count}
    }));

    let corrupt_dir = tempfile::tempdir().expect("tempdir corrupt pins");
    std::fs::write(
        corrupt_dir.path().join("openbot-page-rail-pins-v1.json"),
        r#"{"version":1,"pinned_sections":["community","nonexistent_tab","","chats","books","community"]}"#,
    )
    .expect("write corrupt pins");
    let loaded_corrupt: Vec<String> = load_page_rail_pins(corrupt_dir.path()).into_iter().map(sec_id).collect();
    insert_scenario(sc, "f04_corrupt_pins", "page_rail", json!({"persisted_pins": loaded_corrupt}));

    let mut rail_none = AppleAppSidebar::new(dark, "zh-CN");
    for s in rail_none.pinned_sections.clone() {
        rail_none.toggle_pin_local(s);
    }
    rail_none.toggle_more_menu();
    let mut none_top: Vec<String> = vec!["chats".into()];
    none_top.extend(rail_none.pinned_sections.iter().copied().map(sec_id));
    none_top.push("more".into());
    let none_pinned: Vec<String> = rail_none.pinned_sections.iter().copied().map(sec_id).collect();
    insert_scenario(sc, "f04_unpin_all", "page_rail", json!({
        "pinned_items": none_pinned, "top_items": none_top,
        "more_menu": {"open": rail_none.more_menu_open, "entries": all_more_entries},
    }));

    let mut rail_720 = AppleAppSidebar::new(dark, "zh-CN");
    rail_720.toggle_more_menu();
    rail_720.set_narrow_hidden(true);
    let mut rail_721 = AppleAppSidebar::new(dark, "zh-CN");
    rail_721.set_narrow_hidden(false);
    let rl_720 = rail_720.compute_layout(800.0);
    let rl_721 = rail_721.compute_layout(800.0);
    insert_scenario(sc, "f04_rail_720", "page_rail", json!({"visible": rl_720.visible, "more_menu": {"open": rail_720.more_menu_open}}));
    insert_scenario(sc, "f04_rail_721", "page_rail", json!({"visible": rl_721.visible, "width_px": rl_721.width_px as i32}));

    let mut rail_mand = AppleAppSidebar::new(dark, "zh-CN");
    let hid_settings_ok = rail_mand.hide_bottom_item_by_id("settings");
    let hid_account_ok = rail_mand.hide_bottom_item_by_id("account");
    assert!(!hid_settings_ok && !hid_account_ok);
    let rl_mand = rail_mand.compute_layout(900.0);
    let mand_hid: Vec<&str> = rail_mand.hidden_bottom_items.iter().map(|i| i.as_str()).collect();
    insert_scenario(sc, "f04_hide_mandatory", "page_rail", json!({
        "hidden_bottom_items": mand_hid,
        "bottom_items": {
            "help_visible": rl_mand.bottom_items.help_visible,
            "settings_visible": rl_mand.bottom_items.settings_visible,
            "account_tile_px": rl_mand.bottom_items.account_tile_px as i32,
        },
    }));

    // UI-F05 SidebarChrome
    let mut hist = ChatNavHistory::new();
    let tb_empty = compute_openbot_sidebar_topbar_layout(hist.can_back(), hist.can_forward(), true, true);
    hist.push(101);
    hist.push(202);
    let tb_at_b = compute_openbot_sidebar_topbar_layout(hist.can_back(), hist.can_forward(), true, true);
    let _ = hist.go_back();
    let tb_back = compute_openbot_sidebar_topbar_layout(hist.can_back(), hist.can_forward(), true, true);

    let wired_sb = |pal: TdesktopThemePalette| -> ChatSidebar {
        let mut s = ChatSidebar::new();
        configure_openbot_sidebar_for_shell(&mut s, default_openbot_shell_wiring());
        s.set_palette(pal);
        s
    };

    let sb_root = wired_sb(dark);
    let sr_default = compute_openbot_sidebar_search_row_layout(sb_root.is_collapsed, true, 2, sb_root.is_new_menu_open, &dark, "zh-CN");
    root.insert("sidebar_chrome".into(), json!({
        "default_width_px": OPENBOT_SIDEBAR_DEFAULT_WIDTH as i32,
        "width_px": sb_root.rendered_sidebar_width() as i32,
        "persisted_width_px": sb_root.openbot_width as i32,
        "min_width_px": OPENBOT_SIDEBAR_MIN_WIDTH as i32,
        "max_width_px": OPENBOT_SIDEBAR_MAX_WIDTH as i32,
        "compat_const_default_width_px": LEGACY_SIDEBAR_DEFAULT_WIDTH as i32,
        "mode": sb_mode_str(&sb_root),
        "collapse_threshold_px": OPENBOT_SIDEBAR_COLLAPSE_THRESHOLD as i32,
        "expand_threshold_px": OPENBOT_SIDEBAR_EXPAND_THRESHOLD as i32,
        "topbar": {
            "height_px": tb_at_b.height_px as i32, "toggle_visible": tb_at_b.toggle_visible,
            "back_enabled": tb_at_b.back_enabled, "forward_enabled": tb_at_b.forward_enabled,
            "back_opacity": tb_at_b.back_opacity, "forward_opacity": tb_at_b.forward_opacity,
            "mac_clearance_px": tb_at_b.mac_clearance_px as i32,
        },
        "search": {
            "height_px": sr_default.height_px as i32, "radius_px": sr_default.radius_px as i32,
            "bg_hex": sr_default.bg_hex, "funnel_size_px": sr_default.funnel_size_px as i32,
            "filter_active": sr_default.filter_active, "compact_search_visible": sr_default.compact_search_visible,
            "compact_search_size_px": sr_default.compact_search_size_px as i32,
            "filter_empty_visible": sr_default.filter_empty_visible, "clear_filter_visible": sr_default.clear_filter_visible,
        },
        "add_button": {
            "size_px": sr_default.add_button_size_px as i32, "bg_hex": sr_default.add_button_bg_hex,
            "fg_hex": sr_default.add_button_fg_hex, "menu_open": sr_default.add_menu_open, "menu_items": sr_default.add_menu_items,
        }
    }));

    let mut sb_plus = wired_sb(dark);
    sb_plus.toggle_new_menu();
    let sr_plus = compute_openbot_sidebar_search_row_layout(sb_plus.is_collapsed, false, 2, sb_plus.is_new_menu_open, &dark, "zh-CN");
    insert_scenario(sc, "f05_plus_menu", "sidebar_chrome", json!({
        "add_button": {
            "size_px": sr_plus.add_button_size_px as i32, "bg_hex": sr_plus.add_button_bg_hex,
            "fg_hex": sr_plus.add_button_fg_hex, "menu_open": sr_plus.add_menu_open, "menu_items": sr_plus.add_menu_items,
        }
    }));

    let mut sb = wired_sb(dark);
    sb.apply_openbot_drag_width(340.0);
    sb.save_openbot_width_prefs(tmp.path()).expect("save sidebar width");
    let mut sb_reloaded = wired_sb(dark);
    let persisted_w = sb_reloaded.load_openbot_width_prefs(tmp.path());
    insert_scenario(sc, "f05_resized_340", "sidebar_chrome", json!({"width_px": sb.rendered_sidebar_width() as i32, "persisted_width_px": persisted_w as i32}));
    sb.apply_openbot_drag_width(105.0);
    insert_scenario(sc, "f05_drag_105_compact", "sidebar_chrome", json!({"mode": sb_mode_str(&sb), "width_px": sb.rendered_sidebar_width() as i32}));
    sb.apply_openbot_drag_width(124.0);
    insert_scenario(sc, "f05_drag_124_expand", "sidebar_chrome", json!({"mode": sb_mode_str(&sb), "width_px": sb.rendered_sidebar_width() as i32}));

    let mut sb_c = wired_sb(dark);
    let cmd_b_toggled_chats = apply_cmd_b_sidebar_toggle(ActiveWorkspacePanel::Chats, &mut sb_c);
    assert!(cmd_b_toggled_chats && sb_c.is_sidebar_closed);
    let sh_c = compute_openbot_shell_frame_layout(&make_input(
        1440.0,
        900.0,
        dark,
        ActiveWorkspacePanel::Chats,
        sb_c.is_sidebar_closed,
        sb_c.rendered_sidebar_width(),
        false,
    ));
    let tb_closed = compute_openbot_sidebar_topbar_layout(
        false,
        false,
        sh_c.page_kind.as_str() == "agents",
        sh_c.page_rail_visible,
    );
    insert_scenario(sc, "f05_cmd_b_closed", "sidebar_chrome", json!({"mode": sb_mode_str(&sb_c), "width_px": sb_c.rendered_sidebar_width() as i32, "topbar": {"toggle_visible": tb_closed.toggle_visible}}));
    sb_c.toggle_closed_from_user();
    insert_scenario(sc, "f05_panel_left_restored", "sidebar_chrome", json!({"mode": sb_mode_str(&sb_c), "width_px": sb_c.rendered_sidebar_width() as i32}));
    let r2 = |v: f32| ((v as f64) * 100.0).round() / 100.0;
    insert_scenario(sc, "f05_history_at_b", "sidebar_chrome", json!({"topbar": {"height_px": tb_at_b.height_px as i32, "back_enabled": tb_at_b.back_enabled, "forward_enabled": tb_at_b.forward_enabled, "forward_opacity": r2(tb_at_b.forward_opacity), "mac_clearance_px": tb_at_b.mac_clearance_px as i32}}));
    insert_scenario(sc, "f05_history_after_back", "sidebar_chrome", json!({"topbar": {"forward_enabled": tb_back.forward_enabled}}));
    insert_scenario(sc, "f05_empty_history", "sidebar_chrome", json!({"topbar": {"back_enabled": tb_empty.back_enabled, "forward_enabled": tb_empty.forward_enabled, "back_opacity": r2(tb_empty.back_opacity), "forward_opacity": r2(tb_empty.forward_opacity)}}));

    for (name, w) in [("f05_drag_650", 650.0_f32), ("f05_drag_127", 127.0), ("f05_drag_neg50", -50.0), ("f05_drag_nan", f32::NAN)] {
        let mut t = wired_sb(dark);
        t.apply_openbot_drag_width(w);
        insert_scenario(sc, name, "sidebar_chrome", json!({"mode": sb_mode_str(&t), "width_px": t.rendered_sidebar_width() as i32}));
    }
    let mut sb_dc = wired_sb(dark); sb_dc.apply_openbot_drag_width(105.0); sb_dc.apply_openbot_drag_width(114.0);
    let mut sb_de = wired_sb(dark); sb_de.apply_openbot_drag_width(114.0);
    insert_scenario(sc, "f05_deadband_from_compact_114", "sidebar_chrome", json!({"mode": sb_mode_str(&sb_dc), "width_px": sb_dc.rendered_sidebar_width() as i32}));
    insert_scenario(sc, "f05_deadband_from_expanded_114", "sidebar_chrome", json!({"mode": sb_mode_str(&sb_de), "width_px": sb_de.rendered_sidebar_width() as i32}));

    let mut sb_cmp = wired_sb(dark);
    sb_cmp.apply_openbot_drag_width(90.0);
    let sr_cmp = compute_openbot_sidebar_search_row_layout(sb_cmp.is_collapsed, false, 1, false, &dark, "zh-CN");
    let tabs_cmp = compute_openbot_sidebar_tabs_layout(sb_cmp.is_collapsed, false, false, "All", false, &dark);
    insert_scenario(sc, "f05_compact_88", "sidebar_chrome", json!({"mode": sb_mode_str(&sb_cmp), "search": {"compact_search_visible": sr_cmp.compact_search_visible, "compact_search_size_px": sr_cmp.compact_search_size_px as i32}}));
    insert_scenario(sc, "f05_compact_88", "sidebar_list", json!({"tabs": {"visible": tabs_cmp.visible}}));

    let mut sb_books = wired_sb(dark);
    let books_panel = ActiveWorkspacePanel::ContentHub(ContentHubSection::Books);
    let cmd_b_toggled_books = apply_cmd_b_sidebar_toggle(books_panel, &mut sb_books);
    assert!(
        !cmd_b_toggled_books && !sb_books.is_sidebar_closed,
        "Cmd+B must be ignored on non-Agents pages (Books)"
    );
    let sh_books = compute_openbot_shell_frame_layout(&make_input(
        1440.0,
        900.0,
        dark,
        books_panel,
        sb_books.is_sidebar_closed,
        sb_books.rendered_sidebar_width(),
        false,
    ));
    let tb_books = compute_openbot_sidebar_topbar_layout(
        false,
        false,
        sh_books.page_kind.as_str() == "agents",
        sh_books.page_rail_visible,
    );
    insert_scenario(sc, "f05_books_page_cmd_b", "sidebar_chrome", json!({
        "topbar": {"toggle_visible": tb_books.toggle_visible},
        "cmd_b_ignored": !cmd_b_toggled_books,
    }));
    insert_scenario(sc, "f05_books_page_cmd_b", "shell", json!({"sidebar_visible": sh_books.sidebar_rect.width > 0.0}));

    let mut sb_filt = wired_sb(dark);
    sb_filt.dialogs.push(DialogItemModel {
        id: 501,
        peer_name: "Research Assistant".into(),
        avatar_initials: "R".into(),
        avatar_color: 0x3B93F0,
        last_message: "Architecture summary ready".into(),
        timestamp: "10:42".into(),
        unread_count: 1,
        is_muted: false,
        is_pinned: false,
        is_online: true,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "All".into(),
    });
    sb_filt.dialogs.push(DialogItemModel {
        id: 502,
        peer_name: "Product Squad".into(),
        avatar_initials: "P".into(),
        avatar_color: 0x8B5CF6,
        last_message: "Sprint review at 3pm".into(),
        timestamp: "10:40".into(),
        unread_count: 0,
        is_muted: false,
        is_pinned: false,
        is_online: true,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "All".into(),
    });
    sb_filt.set_search_query("zzz_no_matching_dialog");
    let filtered_zero = sb_filt.filtered_dialogs();
    let sr_zero = compute_openbot_sidebar_search_row_layout(
        sb_filt.is_collapsed,
        !sb_filt.search_query.is_empty(),
        filtered_zero.len(),
        false,
        &dark,
        "zh-CN",
    );
    let zero_rows: Vec<Value> = filtered_zero
        .iter()
        .map(|d| json!({"id": d.id, "title": d.peer_name}))
        .collect();
    insert_scenario(sc, "f05_filter_zero_matches", "sidebar_chrome", json!({"search": {"filter_empty_visible": sr_zero.filter_empty_visible, "clear_filter_visible": sr_zero.clear_filter_visible}}));
    insert_scenario(sc, "f05_filter_zero_matches", "sidebar_list", json!({"rows": zero_rows}));
    sb_filt.set_search_query("");
    let filtered_clr = sb_filt.filtered_dialogs();
    let sr_clr = compute_openbot_sidebar_search_row_layout(
        sb_filt.is_collapsed,
        !sb_filt.search_query.is_empty(),
        filtered_clr.len(),
        false,
        &dark,
        "zh-CN",
    );
    insert_scenario(sc, "f05_filter_cleared", "sidebar_chrome", json!({"search": {"filter_active": sr_clr.filter_active}}));
}

pub fn populate(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    populate_shell_and_rail(root, sc);
    populate_rail_scenarios_and_sidebar_chrome(root, sc);
}
