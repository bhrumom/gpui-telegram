//! Live runtime probe for UI-F06 (SidebarList), UI-F07 (GlobalSearch), and UI-F08 (FirstAgentSetup Studio).

use gpui_telegram_desktop::theme::openbot_palette::OpenBotPalette;
use gpui_telegram_desktop::views::app_shell::openbot_shell_layout::{
    compute_shell_account_footer_audit, configure_openbot_sidebar_for_shell,
    default_openbot_shell_wiring,
};
use gpui_telegram_desktop::views::apple_sidebar::AppleAppSidebar;
use gpui_telegram_desktop::views::boxes::modal_theme::ConfirmDialogState;
use gpui_telegram_desktop::views::chat_sidebar::{
    ChatSidebar, DialogDeliveryStatus, DialogItemModel,
};
use gpui_telegram_desktop::views::openbot::agent_studio::{
    FIRST_AGENT_PURPOSE_MAX_CHARS, FirstAgentSetupMode, FirstAgentSetupState,
    GLOBAL_SEARCH_DEBOUNCE_MS, GlobalSearchResultItem, GlobalSearchSectionKind, GlobalSearchState,
    GlobalSearchTab, compute_first_agent_setup_layout, compute_global_search_modal_layout,
};
use gpui_telegram_desktop::views::openbot::shell_state::{
    SIDEBAR_GROUP_ORDER, SidebarLayoutSnapshot, SidebarSection, resolve_folder_depths,
    validate_sidebar_layout_strict,
};
use gpui_telegram_desktop::views::openbot::workspace_shell::OpenBotWorkspaceShell;
use gpui_telegram_desktop::views::sidebar::dialogs_overlays::compute_openbot_sidebar_tabs_layout;
use gpui_telegram_desktop::views::sidebar::dialogs_renderer::{
    compute_openbot_pinned_grid_layout, compute_sidebar_empty_state_layout,
};
use gpui_telegram_desktop::views::sidebar::dialogs_row_view::{
    SidebarDialogMeta, compute_openbot_dialog_row_layout, openbot_context_menu_actions,
    sidebar_context_action_key,
};
use gpui_telegram_desktop::views::sidebar::ui::dialogs_topics_view::{
    DialogsTopicsViewUi, ForumTopicEntry,
};
use openbot_contracts::avatar::AVATAR_COLOR_CHOICES;
use serde_json::{Map, Value, json};

use super::{dark_palette, insert_scenario};

fn sample_dialog(id: u64, title: &str, preview: &str) -> DialogItemModel {
    DialogItemModel {
        id,
        peer_name: title.to_string(),
        avatar_initials: "A".to_string(),
        avatar_color: 0x3B93F0,
        last_message: preview.to_string(),
        timestamp: "10:42".to_string(),
        unread_count: 1,
        is_muted: false,
        is_pinned: false,
        is_online: true,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "All".to_string(),
    }
}

fn row_json(item: &DialogItemModel, meta: &SidebarDialogMeta, width_px: f32) -> Value {
    let r = compute_openbot_dialog_row_layout(item, meta, width_px, "zh-CN");
    let valid_unicode = !r.preview_displayed.contains('\u{FFFD}')
        && std::str::from_utf8(r.preview_displayed.as_bytes()).is_ok();
    json!({
        "height_px": r.height_px as i32,
        "avatar_px": r.avatar_px as i32,
        "radius_px": r.radius_px as i32,
        "min_w_0": r.min_w_0,
        "whitespace_nowrap": r.whitespace_nowrap,
        "time_visible": r.time_visible,
        "preview_displayed": r.preview_displayed,
        "preview_truncated": r.preview_truncated,
        "valid_utf8_utf16": valid_unicode,
        "thread_chevron_hover": r.thread_chevron_hover,
        "thread_subrow_height_px": r.thread_subrow_height_px as i32,
        "wait_chip_icon_only": r.wait_chip_icon_only,
        "archived_or_hidden": r.archived_or_hidden,
        "opacity": r.opacity,
        "grayscale": r.grayscale,
    })
}

fn gs_item(id: &str, sec: GlobalSearchSectionKind, title: &str, sub: &str) -> GlobalSearchResultItem {
    GlobalSearchResultItem {
        id: id.into(), section: sec, title: title.into(), subtitle: sub.into(),
        badge: None, avatar_seed: Some(id.into()), shortcut_hint: None,
    }
}

fn populate_sidebar_list_and_search(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let rail = AppleAppSidebar::new(dark, "zh-CN");
    let mut sb_wired = ChatSidebar::new();
    configure_openbot_sidebar_for_shell(&mut sb_wired, default_openbot_shell_wiring());
    let footer_audit = compute_shell_account_footer_audit(&rail, &sb_wired);

    let tabs_l = compute_openbot_sidebar_tabs_layout(false, false, false, "All", true, &dark);
    let tabs_cmp = compute_openbot_sidebar_tabs_layout(true, false, false, "All", false, &dark);
    let pg0 = compute_openbot_pinned_grid_layout(0, false, &dark);
    let pg3 = compute_openbot_pinned_grid_layout(3, false, &dark);
    let pg_cmp = compute_openbot_pinned_grid_layout(3, true, &dark);
    let long_cjk = "法布施工作区智能助手👩‍👩‍👧‍👦正在梳理季度产品路线图并整理关键里程碑与交付清单以及全部自动化校验项";
    let d_cjk = sample_dialog(101, "法布施规划助手", long_cjk);
    let mut m_topics = SidebarDialogMeta::default();
    let mut topics_ui = DialogsTopicsViewUi::new(101);
    topics_ui.topics.push(ForumTopicEntry {
        topic_id: 1, title: "Q4 Roadmap".into(), icon_emoji: "📌".into(),
        unread_count: 1, is_closed: false, is_hidden: false,
    });
    m_topics.topics = Some(topics_ui);
    let ctx_keys: Vec<&str> = openbot_context_menu_actions(101).into_iter().map(sidebar_context_action_key).collect();

    let active_rows = vec![row_json(&d_cjk, &m_topics, 280.0)];
    let es_root = compute_sidebar_empty_state_layout(active_rows.len(), &dark, "zh-CN");

    root.insert("sidebar_list".into(), json!({
        "tabs": {"visible": tabs_l.visible, "height_px": tabs_l.height_px as i32, "active_id": tabs_l.active_id, "active_underline_px": tabs_l.active_underline_px as i32, "active_underline_hex": tabs_l.active_underline_hex, "uses_blue_pill": tabs_l.uses_blue_pill, "unread_badge_height_px": tabs_l.unread_badge_height_px as i32, "sliders_menu_open": tabs_l.sliders_menu_open},
        "pinned_grid": {"visible": pg3.visible, "legacy_oversized_circles": pg3.legacy_oversized_circles, "columns": pg3.columns, "max_width_px": pg3.max_width_px as i32, "tile_w_px": pg3.tile_w_px as i32, "tile_h_px": pg3.tile_h_px as i32, "avatar_px": pg3.avatar_px as i32, "gap_px": pg3.gap_px as i32},
        "rows": active_rows,
        "context_menu_items": ctx_keys,
        "group_order": SIDEBAR_GROUP_ORDER,
        "has_duplicate_account_footer": footer_audit.has_duplicate_account_footer,
        "empty_state": {"visible": es_root.visible, "label": es_root.cta_label},
    }));

    insert_scenario(sc, "f06_zero_pins", "sidebar_list", json!({"pinned_grid": {"visible": pg0.visible, "legacy_oversized_circles": pg0.legacy_oversized_circles}}));
    insert_scenario(sc, "f06_width_320", "sidebar_list", json!({"rows": [row_json(&d_cjk, &m_topics, 320.0)]}));
    insert_scenario(sc, "f06_width_300", "sidebar_list", json!({"rows": [row_json(&d_cjk, &m_topics, 300.0)]}));
    insert_scenario(sc, "f06_width_220", "sidebar_list", json!({"rows": [row_json(&d_cjk, &m_topics, 220.0)]}));
    insert_scenario(sc, "f06_width_210", "sidebar_list", json!({"rows": [row_json(&d_cjk, &m_topics, 210.0)]}));
    insert_scenario(sc, "f06_cjk_emoji_truncate", "sidebar_list", json!({"rows": [row_json(&d_cjk, &m_topics, 260.0)]}));

    let sb_empty = ChatSidebar::new();
    let empty_filtered = sb_empty.filtered_dialogs();
    let es_empty = compute_sidebar_empty_state_layout(empty_filtered.len(), &dark, "zh-CN");
    let empty_rows: Vec<Value> = empty_filtered
        .iter()
        .map(|d| row_json(d, &SidebarDialogMeta::default(), 280.0))
        .collect();
    insert_scenario(sc, "f06_empty_workspace", "sidebar_list", json!({
        "empty_state": {"visible": es_empty.visible, "label": es_empty.cta_label},
        "pinned_grid": {"visible": pg0.visible},
        "rows": empty_rows,
    }));

    let blank_res = validate_sidebar_layout_strict(&SidebarLayoutSnapshot {
        revision: 1,
        sections: vec![SidebarSection { id: "s1".into(), name: "   ".into(), parent_id: None, collapsed: false }],
        order: vec!["s1".into(), "unassigned".into(), "people".into()],
        agent_assignments: Default::default(),
        agent_order: Default::default(),
    });
    let blank_rejected = blank_res.as_ref().is_err_and(|e| e.code == "empty_section_name");
    let deep_sections = vec![
        SidebarSection { id: "s1".into(), name: "L1".into(), parent_id: None, collapsed: false },
        SidebarSection { id: "s2".into(), name: "L2".into(), parent_id: Some("s1".into()), collapsed: false },
        SidebarSection { id: "s3".into(), name: "L3".into(), parent_id: Some("s2".into()), collapsed: false },
        SidebarSection { id: "s4".into(), name: "L4".into(), parent_id: Some("s3".into()), collapsed: false },
    ];
    let deep_res = validate_sidebar_layout_strict(&SidebarLayoutSnapshot {
        revision: 1,
        sections: deep_sections.clone(),
        order: vec!["s1".into(), "s2".into(), "s3".into(), "s4".into(), "unassigned".into(), "people".into()],
        agent_assignments: Default::default(),
        agent_order: Default::default(),
    });
    let deep_rejected = deep_res.as_ref().is_err_and(|e| e.code == "folder_depth_exceeded");
    let clamped_max_depth = resolve_folder_depths(&deep_sections).values().copied().max().unwrap_or(0);
    insert_scenario(sc, "f06_folder_blank_name", "sidebar_list", json!({"folder_editor": {"aria_invalid": blank_rejected, "rejected": blank_rejected}}));
    insert_scenario(sc, "f06_folder_depth_3", "sidebar_list", json!({"folder_editor": {"rejected": deep_rejected, "depth": clamped_max_depth}}));

    let mut m_hid = SidebarDialogMeta::default();
    m_hid.is_archived = true;
    insert_scenario(sc, "f06_hidden_row", "sidebar_list", json!({"rows": [row_json(&d_cjk, &m_hid, 280.0)]}));
    insert_scenario(sc, "f06_compact_pinned", "sidebar_list", json!({
        "pinned_grid": {"tile_w_px": pg_cmp.tile_w_px as i32, "tile_h_px": pg_cmp.tile_h_px as i32},
        "tabs": {"visible": tabs_cmp.visible},
    }));
}

fn studio_json(st: &FirstAgentSetupState, ob: &OpenBotPalette, lang: &str, created_id: Option<&str>, nav_chat: bool, wired_create: bool) -> Value {
    let l = compute_first_agent_setup_layout(st, ob, lang);
    let mode_s = match st.mode { FirstAgentSetupMode::First => "first", FirstAgentSetupMode::Additional => "additional" };
    json!({
        "mode": mode_s,
        "form_width_px": l.form_width_px as i32,
        "canvas_bg_hex": l.canvas_bg_hex,
        "header": {"height_px": l.header_height_px as i32, "avatar_px": l.header_avatar_px as i32, "live_name": l.header_live_name, "cancel_visible": l.header_cancel_visible},
        "hero": {"size_px": l.hero_size_px as i32, "is_plush_3d": l.hero_is_plush_3d, "pointer_follow": l.hero_pointer_follow, "shape": l.hero_shape},
        "swatches": {"cols": l.swatch_cols, "rows": l.swatch_rows, "total": l.swatch_total, "swatch_px": l.swatch_px as i32, "selected_idx": l.swatch_selected_idx, "selection_ring_css": l.swatch_selection_ring_css},
        "shapes": {"count": l.shape_count, "button_px": l.face_btn_px as i32, "preview_px": l.face_preview_px as i32, "eye_scale": l.shape_eye_scale, "contains_goutte": l.shapes_contain_goutte},
        "fields": {"name": l.field_name, "purpose": l.field_purpose, "purpose_min_height_px": l.purpose_min_height_px as i32, "model_row_height_px": l.model_row_height_px as i32},
        "submit": {"label": l.submit_label, "bg_hex": l.submit_bg_hex, "fg_hex": l.submit_fg_hex, "min_width_px": l.cta_min_width_px as i32, "disabled": l.submit_disabled, "error_message": l.error_message},
        "recommendations": {"total_templates": l.rec_total_templates, "visible_cards": l.rec_visible_cards, "card_w_px": l.rec_card_w_px as i32, "card_h_px": l.rec_card_h_px as i32, "card_avatar_px": l.rec_card_avatar_px as i32, "suggestion_id": l.rec_suggestion_id, "hydrated": l.rec_hydrated},
        "created_agent_id": created_id, "navigated_to_chat": nav_chat, "wired_to_create_managed_bot": wired_create,
    })
}

fn populate_global_search_and_studio(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark_ob = OpenBotPalette::dark();
    let gs_layout = compute_global_search_modal_layout(1440.0, 900.0, true);
    let mut gs = GlobalSearchState::new(false);
    let root_opened = gs.try_open_palette(false);
    gs.set_query("planner", 80.0, 100);
    let seq_root = gs.begin_remote_search(260).unwrap_or(0);
    gs.apply_remote_results(seq_root, vec![
        gs_item("agent-planner", GlobalSearchSectionKind::Agents, "Trip Planner", "Travel planner"),
        gs_item("msg-42", GlobalSearchSectionKind::Messages, "Q4 Invoice", "Invoice PDF attached"),
    ]);
    let vms = gs.build_visible_row_view_models();
    let mut cycle = Vec::new();
    let mut g_tab = GlobalSearchState::new(false);
    for _ in 0..GlobalSearchTab::ALL.len() {
        g_tab.cycle_tab(true);
        cycle.push(g_tab.active_tab.as_str());
    }

    let mut gs_idle = GlobalSearchState::new(false);
    gs_idle.open_palette();
    gs_idle.set_caret_x(40.0, 100);
    let (idle_x, idle_y) = gs_idle.plush_gaze_offset_pct();
    let idle_pm = gs_idle.plush_metrics();

    root.insert("global_search".into(), json!({
        "open": gs.open, "width_px": gs_layout.width_px as i32, "top_px": gs_layout.top_px as i32,
        "bg_hex": gs_layout.bg_hex, "ring_border": gs_layout.ring_border,
        "input_height_px": gs_layout.input_height_px as i32,
        "footer": {"height_px": gs_layout.footer_height_px as i32, "hints": gs_layout.footer_hints},
        "tabs": GlobalSearchTab::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
        "active_tab": gs.active_tab.as_str(), "tab_cycle_trace": cycle,
        "row_height_px": gs_layout.row_height_px as i32, "section_header_height_px": gs_layout.section_header_height_px as i32,
        "highlighted_index": gs.selected_index, "results": vms,
        "plush": {"visible": gs_idle.is_plush_visible(), "size_px": gs_layout.plush_size_px as i32, "caret_offset_x_pct": idle_x, "caret_offset_y_pct": idle_y, "idle_scan_active": idle_pm.animated, "head_tilt_deg": idle_pm.tilt_deg, "reduced_motion_frozen": !idle_pm.animated},
        "remote": {"debounce_ms": GLOBAL_SEARCH_DEBOUNCE_MS, "latest_seq": gs.remote_request_seq, "stale_discarded_count": gs.remote_stale_discarded, "error_retry_visible": gs.remote_error.is_some()},
        "navigated_target_id": gs.last_activated_target_id, "blocked_by_active_dialog": !root_opened,
    }));

    insert_scenario(sc, "f07_plush_idle", "global_search", json!({
        "plush": {"visible": gs_idle.is_plush_visible(), "size_px": gs_layout.plush_size_px as i32, "caret_offset_x_pct": idle_x, "caret_offset_y_pct": idle_y, "idle_scan_active": idle_pm.animated, "head_tilt_deg": idle_pm.tilt_deg, "reduced_motion_frozen": !idle_pm.animated}
    }));
    let mut gs_exec = gs.clone();
    let activated = gs_exec.execute_selected();
    insert_scenario(sc, "f07_enter_executed", "global_search", json!({"open": gs_exec.open, "navigated_target_id": activated.map(|a| a.id)}));

    let mut gs_rem = GlobalSearchState::new(false);
    gs_rem.set_query("invoice", 40.0, 0);
    let seq1 = gs_rem.begin_remote_search(200).expect("debounce elapsed");
    gs_rem.apply_remote_results(seq1, vec![
        gs_item("msg-inv", GlobalSearchSectionKind::Messages, "Invoice #104", "Paid"),
        gs_item("file-inv", GlobalSearchSectionKind::Files, "invoice.pdf", "120 KB"),
    ]);
    insert_scenario(sc, "f07_remote_invoice", "global_search", json!({
        "remote": {"debounce_ms": GLOBAL_SEARCH_DEBOUNCE_MS, "latest_seq": gs_rem.remote_request_seq},
        "results": gs_rem.build_visible_row_view_models(),
    }));

    let mut gs_stale = GlobalSearchState::new(false);
    gs_stale.set_query("a", 10.0, 0);
    assert!(
        gs_stale.begin_remote_search(GLOBAL_SEARCH_DEBOUNCE_MS - 1).is_none(),
        "begin_remote_search must enforce 150ms debounce"
    );
    let s1 = gs_stale.begin_remote_search(200).expect("seq 1");
    gs_stale.set_query("alpha", 50.0, 220);
    assert!(
        gs_stale.begin_remote_search(220 + GLOBAL_SEARCH_DEBOUNCE_MS - 1).is_none(),
        "subsequent query must wait full 150ms debounce"
    );
    let s2 = gs_stale.begin_remote_search(400).expect("seq 2");
    gs_stale.apply_remote_results(s2, vec![gs_item("a1", GlobalSearchSectionKind::Agents, "Alpha Agent", "Match")]);
    gs_stale.apply_remote_results(s1, vec![gs_item("old", GlobalSearchSectionKind::Agents, "Stale", "Old")]);
    insert_scenario(sc, "f07_stale_response_guard", "global_search", json!({
        "remote": {"latest_seq": gs_stale.remote_request_seq, "stale_discarded_count": gs_stale.remote_stale_discarded},
        "results": gs_stale.build_visible_row_view_models(),
    }));

    let mut gs_err = GlobalSearchState::new(false);
    gs_err.set_query("alpha", 20.0, 0);
    assert!(gs_err.begin_remote_search(GLOBAL_SEARCH_DEBOUNCE_MS - 1).is_none());
    let se1 = gs_err.begin_remote_search(200).expect("seq err 1");
    gs_err.fail_remote_search(se1, "timeout");
    insert_scenario(sc, "f07_remote_error", "global_search", json!({"remote": {"latest_seq": gs_err.remote_request_seq, "error_retry_visible": gs_err.remote_error.is_some()}}));
    let se2 = gs_err.retry_remote_search(500);
    gs_err.apply_remote_results(se2, vec![]);
    insert_scenario(sc, "f07_remote_recovered", "global_search", json!({"remote": {"latest_seq": gs_err.remote_request_seq, "error_retry_visible": gs_err.remote_error.is_some()}}));

    let gs_zero = GlobalSearchState::new(false);
    insert_scenario(sc, "f07_zero_match", "global_search", json!({"results": gs_zero.build_visible_row_view_models(), "plush": {"visible": gs_zero.is_plush_visible(), "size_px": gs_layout.plush_size_px as i32}}));
    let gs_narrow = compute_global_search_modal_layout(480.0, 800.0, true);
    insert_scenario(sc, "f07_narrow_480", "global_search", json!({"width_px": gs_narrow.width_px as i32, "top_px": gs_narrow.top_px as i32}));

    let mut gs_rm = GlobalSearchState::new(true);
    gs_rm.set_query("q", 120.0, 0);
    let rm_m = gs_rm.plush_metrics();
    let (rm_x, rm_y) = gs_rm.plush_gaze_offset_pct();
    insert_scenario(sc, "f07_reduced_motion", "global_search", json!({
        "plush": {"visible": gs_rm.is_plush_visible(), "reduced_motion_frozen": !rm_m.animated, "idle_scan_active": rm_m.animated, "head_tilt_deg": rm_m.tilt_deg, "caret_offset_x_pct": rm_x, "caret_offset_y_pct": rm_y}
    }));

    let mut gs_nav = gs.clone();
    gs_nav.move_selection(-1);
    gs_nav.move_selection(100);
    insert_scenario(sc, "f07_arrow_bounds", "global_search", json!({"highlighted_index": gs_nav.selected_index}));

    let active_dlg = ConfirmDialogState::open_dialog(true);
    let mut gs_blk = GlobalSearchState::new(false);
    let opened = gs_blk.try_open_palette(active_dlg.open);
    insert_scenario(sc, "f07_blocked_by_dialog", "global_search", json!({"blocked_by_active_dialog": !opened, "open": gs_blk.open}));

    // UI-F08 FirstAgentSetup Studio
    let mut st_first = FirstAgentSetupState::first_run();
    st_first.name = "Research Bot".into();
    let _ = st_first.set_purpose("Analyze literature and summarize papers");
    root.insert("agent_studio".into(), studio_json(&st_first, &dark_ob, "zh-CN", None, false, false));

    let mut st_sw = st_first.clone();
    st_sw.select_color_choice(AVATAR_COLOR_CHOICES[3]);
    insert_scenario(sc, "f08_swatch_pick", "agent_studio", studio_json(&st_sw, &dark_ob, "zh-CN", None, false, false));
    let mut st_sh = st_first.clone();
    st_sh.select_face(6);
    insert_scenario(sc, "f08_shape_pick", "agent_studio", studio_json(&st_sh, &dark_ob, "zh-CN", None, false, false));
    insert_scenario(sc, "f08_filled_form", "agent_studio", studio_json(&st_first, &dark_ob, "zh-CN", None, false, false));

    let mut st_trip = FirstAgentSetupState::first_run();
    st_trip.select_suggestion("trip-planner");
    insert_scenario(sc, "f08_template_trip_planner", "agent_studio", studio_json(&st_trip, &dark_ob, "zh-CN", None, false, false));

    let mut ws_sub = OpenBotWorkspaceShell::new();
    ws_sub.first_agent_setup = st_sh.clone();
    let created = ws_sub.submit_first_agent_setup().ok();
    insert_scenario(sc, "f08_submit_created", "agent_studio", studio_json(&ws_sub.first_agent_setup, &dark_ob, "zh-CN", created.as_deref(), created.is_some(), created.is_some()));

    let mut st_blank = FirstAgentSetupState::first_run();
    st_blank.name = "   ".into();
    insert_scenario(sc, "f08_blank_name", "agent_studio", studio_json(&st_blank, &dark_ob, "zh-CN", None, false, false));
    let mut ws_over = OpenBotWorkspaceShell::new();
    ws_over.first_agent_setup = FirstAgentSetupState::first_run();
    ws_over.first_agent_setup.name = "OverLimitBot".into();
    let over_set_res = ws_over
        .first_agent_setup
        .set_purpose("x".repeat(FIRST_AGENT_PURPOSE_MAX_CHARS + 1));
    assert!(over_set_res.is_err());
    assert!(
        ws_over.first_agent_setup.validate().is_err(),
        "over-limit purpose must fail FirstAgentSetupState::validate()"
    );
    let over_created = ws_over.submit_first_agent_setup().ok();
    assert!(over_created.is_none());
    insert_scenario(
        sc,
        "f08_over_limit_purpose",
        "agent_studio",
        studio_json(
            &ws_over.first_agent_setup,
            &dark_ob,
            "zh-CN",
            over_created.as_deref(),
            over_created.is_some(),
            over_created.is_some(),
        ),
    );

    let mut ws_esc_add = OpenBotWorkspaceShell::new();
    ws_esc_add.first_agent_setup = FirstAgentSetupState::new(FirstAgentSetupMode::Additional);
    let esc_add = ws_esc_add.handle_first_agent_setup_escape();
    insert_scenario(sc, "f08_escape_additional", "agent_studio", studio_json(&ws_esc_add.first_agent_setup, &dark_ob, "zh-CN", None, esc_add, false));
    let mut ws_esc_first = OpenBotWorkspaceShell::new();
    ws_esc_first.first_agent_setup = FirstAgentSetupState::first_run();
    let esc_first = ws_esc_first.handle_first_agent_setup_escape();
    insert_scenario(sc, "f08_escape_first", "agent_studio", studio_json(&ws_esc_first.first_agent_setup, &dark_ob, "zh-CN", None, esc_first, false));

    let mut st_berr = st_first.clone();
    st_berr.record_backend_error("RPC timeout creating bot");
    insert_scenario(sc, "f08_backend_error", "agent_studio", studio_json(&st_berr, &dark_ob, "zh-CN", None, false, false));
    let mut st_inbox = FirstAgentSetupState::first_run();
    st_inbox.select_suggestion("inbox-helper");
    insert_scenario(sc, "f08_template_inbox", "agent_studio", studio_json(&st_inbox, &dark_ob, "zh-CN", None, false, false));
    let _ = st_inbox.set_purpose("Custom manual instructions");
    insert_scenario(sc, "f08_template_edited", "agent_studio", studio_json(&st_inbox, &dark_ob, "zh-CN", None, false, false));
    let mut st_cjk = FirstAgentSetupState::first_run();
    st_cjk.name = "法布施助手✨".into();
    st_cjk.select_color_choice(AVATAR_COLOR_CHOICES[15]);
    st_cjk.select_face(11);
    insert_scenario(sc, "f08_cjk_swatch_shape_sweep", "agent_studio", studio_json(&st_cjk, &dark_ob, "zh-CN", None, false, false));
}

pub fn populate(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    populate_sidebar_list_and_search(root, sc);
    populate_global_search_and_studio(root, sc);
}
