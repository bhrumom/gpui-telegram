//! Live runtime probe for UI-F09 (38px ConversationHeader) and UI-F10 (1:1 Chat Stream, Activity, Steps, Search, DM).

use gpui_telegram_desktop::theme::TdesktopThemePalette;
use gpui_telegram_desktop::theme::openbot_palette::{OpenBotPalette, solid_bubble_text_hex};
use gpui_telegram_desktop::views::conversation_stage::{
    ConversationStage, DirectDmState, MessageContent, MessageDirection, MessageItemModel,
    compute_agent_activity_layout, compute_scroll_rail_layout,
};
use gpui_telegram_desktop::views::openbot::ask_ui::{
    UI_BLOCK_CARD_WIDTH_EXPR, validate_and_evaluate_ui_block,
};
use gpui_telegram_desktop::views::openbot::conversation::{
    ChatTimelineRow, ConversationViewState, compute_message_preview,
};
use gpui_telegram_desktop::views::openbot::steps_and_trace::{AgentStepsState, TraceWaterfallState};
use gpui_telegram_desktop::views::stage::cards::models::DeliveryStatus;
use gpui_telegram_desktop::views::stage::rich_meta::resolve_image_source;
use gpui_telegram_desktop::views::stage::stage_bars::{
    OpenBotHeaderProps, compute_chat_search_bar_layout, compute_openbot_header_layout,
};
use gpui_telegram_desktop::views::stage::stage_bubble::{
    BubbleGroupLayout, OPENBOT_ENTRY_GAP_PX, OPENBOT_GROUPED_GAP_PX, compute_openbot_bubble_layout,
};
use gpui_telegram_desktop::views::stage::wallpaper::compute_agent_empty_state_layout;
use openbot_contracts::conversation::ConversationMessage;
use serde_json::{Map, Value, json};

use super::{dark_palette, insert_scenario};

fn header_json(props: &OpenBotHeaderProps<'_>, pal: &TdesktopThemePalette) -> Value {
    let l = compute_openbot_header_layout(props, pal);
    json!({
        "height_px": l.height_px as i32,
        "position_mode": l.position_mode,
        "bg_strip_hex": l.bg_strip_hex,
        "header_rect": {"x": l.header_rect.x as i32, "y": l.header_rect.y as i32, "w": l.header_rect.w as i32, "h": l.header_rect.h as i32},
        "timeline_rect": {"x": l.timeline_rect.x as i32, "y": l.timeline_rect.y as i32, "w": l.timeline_rect.w as i32, "h": l.timeline_rect.h as i32},
        "overlaps_timeline": l.overlaps_timeline,
        "shifted_start_px": l.header_rect.x as i32,
        "topbar_controls_right_edge": props.topbar_controls_right_edge_px as i32,
        "wrapped_to_multiline": l.wrapped_to_multiline,
        "actions_clipped": l.actions_clipped,
        "tab_shape": {"visible": l.tab_shape_visible, "fillet_radius_px": l.tab_fillet_radius_px as i32, "fill_hex": l.tab_fill_hex, "transition_ms": l.tab_transition_ms, "disabled_by_motion_or_glass": l.tab_disabled_by_motion_or_glass},
        "title_btn": {"height_px": l.title_btn_height_px as i32, "avatar_px": l.title_avatar_px as i32, "disabled": l.title_disabled, "subtitle_visible": l.title_subtitle_visible, "click_target_pane": l.title_click_target_pane},
        "new_thread_btn_visible": l.new_thread_btn_visible,
        "thread_tabs_count": l.thread_tabs_count,
        "model_picker": {"visible": l.model_picker_visible, "height_px": l.model_picker_height_px as i32, "disabled": l.model_picker_disabled, "opacity": l.model_picker_opacity, "tooltip": l.model_picker_tooltip, "fit_mode": l.model_picker_fit_mode.as_str(), "pending_badge_visible": l.pending_badge_visible},
        "pinned_popover": {"open": l.pinned_popover_open, "max_width_px": l.pinned_popover_max_width_px as i32, "count": l.pinned_count, "empty_state_visible": l.pinned_empty_state_visible},
        "side_panel_toggle": {"size_px": l.side_panel_toggle_size_px as i32, "hairline_visible": l.side_panel_hairline_visible},
        "isolation_lock_visible": l.isolation_lock_visible,
    })
}

pub fn default_header_props<'a>(tabs: &'a [(String, String)]) -> OpenBotHeaderProps<'a> {
    OpenBotHeaderProps {
        chat_id: Some(1001), title: "OpenBot Assistant", subtitle: "Ready", initials: "OB",
        avatar_url: None, active_fabushi_bot: None, is_bot_chat: true, is_group_chat: false,
        group_member_tiles: None, group_member_briefs: &[], bot_activity_state: "idle",
        active_thread_id: None, thread_tabs: tabs, active_model_label: "Claude 4.5",
        active_thinking_effort: "High", model_picker_open: false, pinned_count: 2,
        pinned_popover_open: true, search_open: false, more_menu_open: false,
        side_panel_open: false, right_panel_slot_open: false, external_hosted: true,
        available_width_px: 880.0, is_isolated_lock: false, is_archived: false,
        provider_connected: true, pending_model_badge: Some("Next: GPT-5"),
        reduced_motion: false, window_glass: false, sidebar_closed: false,
        topbar_controls_right_edge_px: 0.0, message_ttl_seconds: 0,
    }
}

fn msg_item(id: u64, out: bool, text: &str) -> MessageItemModel {
    MessageItemModel {
        id,
        sender_id: if out { 1 } else { 2 },
        sender_name: if out { "You".into() } else { "Bot".into() },
        avatar_initials: if out { "Y".into() } else { "B".into() },
        avatar_color: 0x3B93F0,
        direction: if out { MessageDirection::Outgoing } else { MessageDirection::Incoming },
        timestamp: "10:42".into(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: vec![],
        content: MessageContent::Text { text: text.into(), is_edited: false },
        bot_keyboard: None,
        is_pinned: false,
    }
}

fn default_group() -> BubbleGroupLayout {
    BubbleGroupLayout {
        attached_to_prev: false, attached_to_next: false, show_sender_name: false,
        show_bottom_avatar: true, show_tail: false, reserve_avatar_column: true,
    }
}

fn bubble_json(m: &MessageItemModel, err_exp: bool, pal: &TdesktopThemePalette) -> Value {
    let b = compute_openbot_bubble_layout(m, default_group(), false, err_exp, pal, "en");
    let resolved_txt = solid_bubble_text_hex(&b.bg_hex);
    json!({
        "author": b.author, "bg_hex": b.bg_hex, "uses_legacy_blue": b.uses_legacy_blue,
        "radius_outer_px": b.radius_outer_px as i32, "radius_grouped_px": b.radius_grouped_px as i32,
        "actions_opacity_rest": b.actions_opacity_rest, "time_opacity_rest": b.time_opacity_rest,
        "actions_opacity_hover": b.actions_opacity_hover,
        "contains_raw_json_wall": b.contains_raw_json_wall,
        "wide_table_or_code": b.wide_table_or_code,
        "resolved_text_hex": resolved_txt,
        "pending_error_retry": b.pending_error_retry,
    })
}

pub fn populate(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let dark_ob = OpenBotPalette::dark();
    let empty_tabs: Vec<(String, String)> = vec![];
    let mut stage = ConversationStage::new();
    stage.append_and_focus_thread_tab("Follow-up");

    let p_bot = default_header_props(&stage.thread_tabs);
    root.insert("conversation_header".into(), header_json(&p_bot, &dark));
    let mut p_grp = default_header_props(&empty_tabs);
    p_grp.is_bot_chat = false; p_grp.is_group_chat = true;
    insert_scenario(sc, "f09_title_group", "conversation_header", header_json(&p_grp, &dark));
    insert_scenario(sc, "f09_group_active", "conversation_header", header_json(&p_grp, &dark));
    let mut p_arch = p_grp; p_arch.is_archived = true;
    insert_scenario(sc, "f09_group_archived", "conversation_header", header_json(&p_arch, &dark));
    let mut p_hum = default_header_props(&empty_tabs); p_hum.is_bot_chat = false;
    insert_scenario(sc, "f09_title_human", "conversation_header", header_json(&p_hum, &dark));
    let mut p_mid = default_header_props(&empty_tabs); p_mid.available_width_px = 620.0;
    insert_scenario(sc, "f09_narrow_620", "conversation_header", header_json(&p_mid, &dark));
    let mut p_tight = default_header_props(&empty_tabs); p_tight.available_width_px = 424.0;
    insert_scenario(sc, "f09_narrow_424", "conversation_header", header_json(&p_tight, &dark));
    let mut p_disc = default_header_props(&empty_tabs); p_disc.provider_connected = false;
    insert_scenario(sc, "f09_provider_disconnected", "conversation_header", header_json(&p_disc, &dark));
    let mut p_pin0 = default_header_props(&empty_tabs); p_pin0.pinned_count = 0; p_pin0.pinned_popover_open = true;
    insert_scenario(sc, "f09_pinned_empty", "conversation_header", header_json(&p_pin0, &dark));
    let mut p_shift = default_header_props(&empty_tabs); p_shift.sidebar_closed = true; p_shift.topbar_controls_right_edge_px = 44.0;
    insert_scenario(sc, "f09_sidebar_closed_shift", "conversation_header", header_json(&p_shift, &dark));
    let mut p_rm = default_header_props(&empty_tabs); p_rm.reduced_motion = true;
    insert_scenario(sc, "f09_reduced_motion", "conversation_header", header_json(&p_rm, &dark));
    let mut p_wg = default_header_props(&empty_tabs); p_wg.window_glass = true;
    insert_scenario(sc, "f09_window_glass", "conversation_header", header_json(&p_wg, &dark));

    let m_user = msg_item(1, true, "Summarize this architecture");
    let m_asst = msg_item(2, false, "| Service | Latency |\n|---|---|\n| Core | 12ms |\n```rust\nfn main() {}\n```");
    let m_err = msg_item(3, false, "{\"error\":\"rate_limit\",\"status\":429,\"detail\":\"Retry after 10s\"}");
    let b_err_closed = compute_openbot_bubble_layout(&m_err, default_group(), false, false, &dark, "en");
    let b_err_open = compute_openbot_bubble_layout(&m_err, default_group(), false, true, &dark, "en");
    let steps_default = AgentStepsState::default();
    let wf = TraceWaterfallState::from_probe_values(&[
        json!({"id": "sp-1", "label": "Model", "kind": "model", "start_ms": 0, "end_ms": 120}),
        json!({"id": "sp-2", "label": "Tool", "kind": "tool", "start_ms": 120, "end_ms": 300}),
    ]);

    let ui_spec = json!({
        "version": 1, "type": "confirm", "title": "Deploy",
        "actions": [{"id": "confirm", "label": "Confirm"}, {"id": "cancel", "label": "Cancel"}]
    });
    let ui_eval = validate_and_evaluate_ui_block(&ui_spec, Some(&json!({"actionId": "confirm"})), None).ok();
    let ui_status = if ui_eval.as_ref().is_some_and(|e| e.sanitized_response.is_some()) { "answered" } else { "pending" };

    let act6 = compute_agent_activity_layout(true, 6, false, false);
    let act2 = compute_agent_activity_layout(true, 2, false, false);
    let act61 = compute_agent_activity_layout(true, 61, true, false);
    let act_rm = compute_agent_activity_layout(true, 8, false, true);
    let empty_l = compute_agent_empty_state_layout(false);
    let sr_l = compute_scroll_rail_layout(6, 3);
    let cs_bar = compute_chat_search_bar_layout(true, "Core", 0, 2, 0.0);
    let mut dm = DirectDmState { peer_online: true, ..Default::default() };
    dm.on_text_input("Hello!", 1000);
    let dm_snap = dm.snapshot("Hello!");

    let valid_plush_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/openbot_plush/bloub/boule.webp");
    let root_broken_img = resolve_image_source(&valid_plush_path.to_string_lossy(), None).is_none();
    let mut conv = ConversationViewState::new("a1", "Bot", "seed");
    for (id, author, text) in [("m1", "user", "hi"), ("m2", "assistant", "ok")] {
        if let Ok(cm) = serde_json::from_value::<ConversationMessage>(json!({"id": id, "author": author, "text": text, "createdAt": "10:00", "status": "completed"})) {
            conv.rows.push(ChatTimelineRow { message: cm, timestamp_ms: 1000, corrupt_override: None, steps: None, ui_block_card: None, runtime_cards: vec![], voice_player: None });
        }
    }
    let root_boundary_eval = conv.evaluate_row_boundaries(false);

    root.insert("chat_stream".into(), json!({
        "entry_gap_px": OPENBOT_ENTRY_GAP_PX as i32,
        "grouped_gap_px": OPENBOT_GROUPED_GAP_PX as i32,
        "bubbles": [bubble_json(&m_user, false, &dark), bubble_json(&m_asst, false, &dark)],
        "activity": {"visible": act6.visible, "bubble_less": act6.bubble_less, "avatar_px": act6.avatar_px as i32, "shimmer_active": act6.shimmer_active, "shimmer_frozen_reduced_motion": act6.shimmer_frozen_reduced_motion, "steps_max_height_px": act6.steps_max_height_px as i32, "elapsed_visible": act6.elapsed_visible},
        "steps": {
            "collapsed_by_default": !steps_default.expanded,
            "error_drawer_open": b_err_closed.error_drawer_open,
            "error_drawer_btn_label": b_err_closed.error_drawer_btn_label,
            "trace_waterfall_available": !wf.spans.is_empty(),
            "error_pre_max_height_px": b_err_closed.error_pre_max_height_px as i32,
        },
        "ui_block": {"width_expr": UI_BLOCK_CARD_WIDTH_EXPR, "status": ui_status, "badge_hex": dark_ob.token("--openbot-success").unwrap_or("")},
        "empty_state": {"visible": empty_l.visible, "avatar_px": empty_l.avatar_px as i32, "heading_px": empty_l.heading_px as i32},
        "scroll_rail": {"visible": sr_l.visible, "tick_px": sr_l.tick_px as i32, "wave_widths_px": sr_l.wave_widths_px.map(|v| v as i32)},
        "chat_search": {"open": cs_bar.open, "width_px": cs_bar.width_px as i32, "height_px": cs_bar.height_px as i32, "right_offset_px": cs_bar.right_offset_px as i32, "match_counter": cs_bar.match_counter},
        "direct_dm": {"stream_width_px": dm_snap.stream_width_px as i32, "online_dot_px": dm_snap.online_dot_px as i32, "typing_active": dm_snap.typing_active, "typing_idle_timeout_ms": dm_snap.typing_idle_timeout_ms, "send_disabled": dm_snap.send_disabled},
        "broken_image_fallback_visible": root_broken_img,
        "row_boundary_caught_error": root_boundary_eval.row_boundary_isolated,
    }));

    insert_scenario(sc, "f10_thinking_2s", "chat_stream", json!({"activity": {"elapsed_visible": act2.elapsed_visible}}));
    insert_scenario(sc, "f10_thinking_plain_61s", "chat_stream", json!({"activity": {"elapsed_visible": act61.elapsed_visible}}));
    insert_scenario(sc, "f10_steps_error_default", "chat_stream", json!({"bubbles": [bubble_json(&m_err, false, &dark)]}));
    insert_scenario(sc, "f10_steps_error_opened", "chat_stream", json!({"steps": {"error_drawer_open": b_err_open.error_drawer_open, "error_pre_max_height_px": b_err_open.error_pre_max_height_px as i32}}));
    insert_scenario(sc, "f10_raw_json_tool_error", "chat_stream", json!({"bubbles": [bubble_json(&m_err, false, &dark)]}));
    insert_scenario(sc, "f10_reduced_motion", "chat_stream", json!({"activity": {"visible": act_rm.visible, "shimmer_frozen_reduced_motion": act_rm.shimmer_frozen_reduced_motion, "shimmer_active": act_rm.shimmer_active}}));

    let broken_img_unresolved = resolve_image_source("/nonexistent/openbot_missing_404.png", None).is_none();
    let mut m_failed = msg_item(4, true, "Retry me");
    m_failed.delivery_status = DeliveryStatus::Failed;
    insert_scenario(sc, "f10_broken_img_and_retry", "chat_stream", json!({
        "broken_image_fallback_visible": broken_img_unresolved,
        "bubbles": [bubble_json(&m_failed, false, &dark)],
    }));

    let cs_360 = compute_chat_search_bar_layout(true, "zzz-no-hit", 0, 0, 360.0);
    insert_scenario(sc, "f10_search_with_side_panel_360", "chat_stream", json!({"chat_search": {"open": cs_360.open, "match_counter": cs_360.match_counter, "right_offset_px": cs_360.right_offset_px as i32}}));

    let p20k = compute_message_preview(&"a".repeat(20_000), false, false);
    let p20k1 = compute_message_preview(&format!("{}🚀{}", "a".repeat(2999), "b".repeat(17_001)), false, false);
    insert_scenario(sc, "f10_msg_20000", "chat_stream", json!({"bubbles": [{"truncated": p20k.collapsed, "show_full_visible": p20k.show_more_available}]}));
    insert_scenario(sc, "f10_msg_20001", "chat_stream", json!({"bubbles": [{"truncated": p20k1.collapsed, "preview_len": p20k1.preview_utf16_len, "show_full_visible": p20k1.show_more_available, "valid_utf8_utf16": p20k1.surrogate_valid, "preview_text": p20k1.preview_text}]}));

    let eval = conv.evaluate_row_boundaries(true);
    assert!(
        eval.row_boundary_isolated && eval.sibling_rows_rendered >= 2 && eval.isolated_corrupt_rows == 1,
        "expected >=2 healthy sibling rows and 1 isolated corrupt row, got {eval:?}"
    );
    dm.tick(5000);
    let dm_idle = dm.snapshot("   ");
    insert_scenario(sc, "f10_row_isolation_and_dm_idle", "chat_stream", json!({
        "row_boundary_caught_error": eval.row_boundary_isolated,
        "sibling_rows_rendered": eval.sibling_rows_rendered,
        "bubbles": [bubble_json(&m_user, false, &dark), bubble_json(&m_asst, false, &dark)],
        "direct_dm": {"send_disabled": dm_idle.send_disabled, "typing_active": dm_idle.typing_active},
    }));
}
