//! Live runtime probe for UI-F11 (Channel Stage) and UI-F12 (48px Morphing Pill Composer & + Add Menu).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use gpui_telegram_desktop::components::bloub_avatar::{
    ChannelActivityIndicatorState, ChannelAvatarLayout, ChannelClusterMode, ChannelWorkerBrief,
    resolve_channel_cluster,
};
use gpui_telegram_desktop::theme::TdesktopThemePalette;
use gpui_telegram_desktop::theme::openbot_palette::OpenBotPalette;
use gpui_telegram_desktop::views::canonical_composer::CanonicalComposer;
use gpui_telegram_desktop::views::composer::composer_ui::compute_openbot_composer_strip_layout;
use gpui_telegram_desktop::views::composer::ime_machine::{ImeState, KeyAction};
use gpui_telegram_desktop::views::composer::popups_ui::compute_composer_add_menu_layout;
use gpui_telegram_desktop::views::context_inspector::ContextInspector;
use gpui_telegram_desktop::views::conversation_stage::{
    MessageContent, MessageDirection, MessageItemModel,
};
use gpui_telegram_desktop::views::openbot::channel_editor::{
    ChannelEditorState, normalize_channel_icon_and_hue,
};
use gpui_telegram_desktop::views::openbot::channel_view::{
    ChannelConversationState, ChannelRoutingEvent, ChannelTimelineMessage,
    compute_routing_action_markers, merge_consecutive_routing_events,
};
use gpui_telegram_desktop::views::openbot::composer::{
    ComposerAttachmentDraft, ComposerSendMode, ComposerState,
};
use gpui_telegram_desktop::views::stage::cards::models::DeliveryStatus;
use gpui_telegram_desktop::views::stage::history_view_highlight_manager::{
    HistoryViewHighlightManager, SelectedQuote,
};
use gpui_telegram_desktop::views::stage::stage_bubble::{
    OPENBOT_REPLY_QUOTE_BORDER_PX, OPENBOT_REPLY_QUOTE_MAX_HEIGHT_PX, compute_group_message_layout,
};
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::channels::{
    ChannelIcon, ChannelMember, ChannelMessageAuthor, ChannelMessageAuthorKind,
};
use serde_json::{Map, Value, json};

use super::{dark_palette, insert_scenario};

fn members_vec(n: usize) -> Vec<ChannelMember> {
    (0..n).map(|i| ChannelMember { agent_id: format!("agent-{i}") }).collect()
}

fn avatar_eval_json(
    n: usize,
    layout: ChannelAvatarLayout,
    icon: Option<&ChannelIcon>,
    image_url: Option<&str>,
    image_failed: bool,
) -> Value {
    let ms = members_vec(n);
    let ev = resolve_channel_cluster(&ms, layout, icon, image_url, image_failed);
    let render_kind = match ev.mode {
        ChannelClusterMode::CustomImage => "photo",
        ChannelClusterMode::IconTile => "icon",
        ChannelClusterMode::MemberFaces => "cluster",
    };
    let face_scale_pct = ev
        .cells
        .first()
        .map(|c| ((c.face_px / ev.size_px) * 100.0).round() as u32)
        .unwrap_or(0);
    json!({
        "layout": match layout { ChannelAvatarLayout::Row => "row", ChannelAvatarLayout::Cluster => "cluster" },
        "render_kind": render_kind,
        "visible_faces": ev.shown_faces,
        "overflow_count": ev.hidden_count,
        "overflow_pill_tone": ev.overflow_pill_tone.as_str(),
        "overflow_label": ev.plus_pill.unwrap_or_default(),
        "face_scale_pct": face_scale_pct,
        "resolved_icon": ev.resolved_icon.map(|k| k.name.as_str()),
        "normalized_hue": ev.resolved_icon.map(|k| k.hue.degrees()),
        "image_fallback_active": ev.image_failed,
    })
}

fn strip_json(
    text: &str,
    raw_h: f32,
    att_cnt: usize,
    busy: bool,
    disabled: bool,
    dictation: bool,
    is_focused: bool,
    add_menu_open: bool,
    pal: &TdesktopThemePalette,
) -> Value {
    let l = compute_openbot_composer_strip_layout(
        text, raw_h, att_cnt, busy, disabled, dictation, is_focused, pal,
    );
    let items: Vec<&str> = compute_composer_add_menu_layout(true, false)
        .items
        .into_iter()
        .map(|i| i.id())
        .collect();
    json!({
        "compact": l.compact, "height_px": l.height_px as i32, "min_height_px": l.min_height_px as i32,
        "border_radius_px": l.border_radius_px as i32, "surface_hex": l.surface_hex,
        "add_btn_px": l.add_btn_px as i32, "add_trigger_disabled": l.add_btn_disabled,
        "action_circle_px": l.action_circle_px as i32, "action_circle_bg_hex": l.action_circle_bg_hex,
        "action_circle_kind": l.action_circle_kind, "send_disabled": l.send_disabled,
        "dictation_active": l.dictation_active, "dictation_width_px": l.dictation_width_px as i32,
        "dictation_bars": l.dictation_bars, "attachments_count": l.attachments_count,
        "add_menu_open": add_menu_open, "add_menu_items": items,
        "legacy_chip_strip_present": l.legacy_chip_strip_present, "detached_slash_present": l.detached_slash_present,
        "legacy_aa_emoji_row_present": l.legacy_aa_emoji_row_present,
        "focus_border_is_telegram_blue": l.focus_border_is_telegram_blue, "send_btn_is_telegram_blue": l.send_btn_is_telegram_blue,
    })
}

fn worker(id: &str, name: &str) -> ChannelWorkerBrief {
    ChannelWorkerBrief {
        id: id.into(), name: name.into(), avatar_seed: Some(id.into()),
        avatar_hue: None, avatar_color: None, avatar_url: None,
    }
}

fn populate_channel_stage(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark_ob = OpenBotPalette::dark();
    let g_msgs = vec![
        MessageItemModel {
            id: 10, sender_id: 201, sender_name: "ResearchBot".into(), avatar_initials: "R".into(), avatar_color: 0x8B5CF6,
            direction: MessageDirection::Incoming, timestamp: "10:00".into(), delivery_status: DeliveryStatus::Read,
            reply_to: None, reactions: vec![], content: MessageContent::Text { text: "Draft ready".into(), is_edited: false },
            bot_keyboard: None, is_pinned: false,
        },
        MessageItemModel {
            id: 11, sender_id: 201, sender_name: "ResearchBot".into(), avatar_initials: "R".into(), avatar_color: 0x8B5CF6,
            direction: MessageDirection::Incoming, timestamp: "10:00".into(), delivery_status: DeliveryStatus::Read,
            reply_to: None, reactions: vec![], content: MessageContent::Text { text: "Second line".into(), is_edited: false },
            bot_keyboard: None, is_pinned: false,
        },
    ];
    let gm = compute_group_message_layout(&g_msgs, 1, Some(AvatarHue::Violet), Some("#8b5cf6"));
    let mut hl = HistoryViewHighlightManager::new();
    hl.highlight_openbot(SelectedQuote { message_id: 10, ..Default::default() }, 1000);
    let merged = merge_consecutive_routing_events(&[
        ChannelRoutingEvent { action: "asked".into(), agent_id: "bot-a".into() },
        ChannelRoutingEvent { action: "asked".into(), agent_id: "bot-b".into() },
    ]);
    let mut names = BTreeMap::new();
    names.insert("bot-a".into(), "ResearchBot".into());
    names.insert("bot-b".into(), "WriterBot".into());
    let markers = compute_routing_action_markers(&merged, &names);
    let marker_json: Vec<Value> = markers.iter().map(|m| json!({"kind": "channel-routing", "action": m.action, "label": m.label, "clickable_pills": m.clickable_pills})).collect();

    let mut ch_active = ChannelConversationState::new("ch-squad", "Product Squad", vec!["bot-a".into(), "bot-b".into()], Some("bot-a".into()));
    ch_active.messages.push(ChannelTimelineMessage {
        id: "msg-10".into(),
        sequence: 1,
        author: ChannelMessageAuthor {
            kind: ChannelMessageAuthorKind::Agent,
            id: "bot-a".into(),
            name: "ResearchBot".into(),
        },
        task_id: None,
        turn_id: None,
        item_type: None,
        reply_to_message_id: None,
        reply_preview: None,
        superseded: false,
        streaming: false,
        text: "Draft ready".into(),
        created_at: "10:00".into(),
    });
    let active_reply_missing = ch_active.is_reply_target_unavailable("msg-10");
    let workers = vec![worker("bot-a", "ResearchBot"), worker("bot-b", "WriterBot")];
    ch_active.sync_activity_workers(&workers, 0, false);
    let act_snap = ch_active.activity_indicator.snapshot(6_000);

    let mut ed_ok = ChannelEditorState::new_create();
    ed_ok.name = "Product Squad".into();
    let _ = ed_ok.add_member("bot-a");
    let _ = ed_ok.add_member("bot-b");
    let dlg_ok = ed_ok.create_dialog_layout(&dark_ob);
    let created_ch = ed_ok.create_channel_from_editor().ok();

    root.insert("channel_stage".into(), json!({
        "channel_id": created_ch.as_ref().map(|c| c.channel_id.clone()),
        "avatar": avatar_eval_json(5, ChannelAvatarLayout::Cluster, None, None, false),
        "show_author": gm.show_author, "bubble_style": gm.bubble_style,
        "author_gutter_px": gm.author_gutter_px as i32, "author_avatar_px": gm.author_avatar_px as i32,
        "author_avatar_clickable": gm.author_avatar_clickable, "author_name_clickable": gm.author_name_clickable,
        "author_name_color": gm.author_name_color, "consecutive_header_collapsed": gm.consecutive_header_collapsed,
        "reply_quote": {
            "border_px": OPENBOT_REPLY_QUOTE_BORDER_PX as i32, "max_height_px": OPENBOT_REPLY_QUOTE_MAX_HEIGHT_PX as i32,
            "jumped_to_id": hl.current_message_id().map(|id| format!("msg-{id}")),
            "flash_duration_ms": hl.active_flash_duration_ms(), "unavailable_dashed": active_reply_missing,
        },
        "action_markers": marker_json,
        "activity_indicator": {
            "visible": act_snap.visible, "worker_count": act_snap.worker_count,
            "overlap_px": act_snap.overlap_px as i32, "shimmer_animated": act_snap.shimmer_animated,
            "elapsed_text": act_snap.elapsed_text,
        },
        "create_dialog": {
            "width_px": dlg_ok.width_px as i32, "radius_px": dlg_ok.radius_px as i32,
            "surface_hex": dlg_ok.surface_hex, "selected_members": dlg_ok.selected_members,
            "submit_disabled": dlg_ok.submit_disabled, "created": created_ch.is_some(),
        },
        "archived": ch_active.archived, "read_only": ch_active.read_only,
        "composer_hidden": ch_active.read_only, "sidebar_drag_disabled": ch_active.archived,
        "reactions_enabled": ch_active.reactions_enabled,
        "mark_unread_enabled": ch_active.mark_unread_enabled,
        "thread_branch_enabled": ch_active.thread_branch_enabled,
    }));

    for (cnt, sc_id) in [(0, "f11_cluster_0"), (1, "f11_cluster_1"), (2, "f11_cluster_2"), (3, "f11_cluster_3"), (4, "f11_cluster_4"), (5, "f11_cluster_5"), (12, "f11_cluster_12"), (13, "f11_cluster_13"), (100, "f11_cluster_100")] {
        insert_scenario(sc, sc_id, "channel_stage", json!({"avatar": avatar_eval_json(cnt, ChannelAvatarLayout::Cluster, None, None, false)}));
    }
    insert_scenario(sc, "f11_avatar_row", "channel_stage", json!({"avatar": avatar_eval_json(3, ChannelAvatarLayout::Row, None, None, false)}));
    let sp_icon = ChannelIcon { name: "sparkles".into(), hue: 215 };
    insert_scenario(sc, "f11_avatar_icon", "channel_stage", json!({"avatar": avatar_eval_json(3, ChannelAvatarLayout::Cluster, Some(&sp_icon), None, false)}));
    insert_scenario(sc, "f11_avatar_photo", "channel_stage", json!({"avatar": avatar_eval_json(3, ChannelAvatarLayout::Cluster, Some(&sp_icon), Some("https://cdn.local/grp.png"), false)}));

    let code_ico = ChannelIcon { name: "code".into(), hue: 215 };
    let ev_fb = resolve_channel_cluster(&members_vec(3), ChannelAvatarLayout::Cluster, Some(&code_ico), Some("/nonexistent/openbot_404.png"), false);
    insert_scenario(sc, "f11_broken_photo_icon_fallback", "channel_stage", json!({
        "avatar": {"image_fallback_active": ev_fb.image_failed, "resolved_icon": ev_fb.resolved_icon.map(|k| k.name.as_str())}
    }));
    let (inv_icon, norm_hue) = normalize_channel_icon_and_hue(Some("dragon"), Some(999));
    insert_scenario(sc, "f11_invalid_icon_hue", "channel_stage", json!({
        "avatar": {"resolved_icon": inv_icon.map(|n| n.as_str()), "normalized_hue": norm_hue.degrees()}
    }));
}

fn populate_channel_guards_and_composer(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let dark_ob = OpenBotPalette::dark();

    let mut ed_zero = ChannelEditorState::new_create();
    ed_zero.name = "Empty Group".into();
    let dlg_zero = ed_zero.create_dialog_layout(&dark_ob);
    insert_scenario(sc, "f11_create_zero_members", "channel_stage", json!({
        "create_dialog": {"submit_disabled": dlg_zero.submit_disabled, "created": ed_zero.create_channel_from_editor().is_ok()}
    }));

    let mut ed_81 = ChannelEditorState::new_create();
    ed_81.name = "a".repeat(81);
    let _ = ed_81.add_member("bot-a");
    let dlg_81 = ed_81.create_dialog_layout(&dark_ob);
    let err_81 = ed_81.validate_draft().err().map(|e| e.to_string()).unwrap_or_default();
    insert_scenario(sc, "f11_create_name_81", "channel_stage", json!({
        "create_dialog": {"submit_disabled": dlg_81.submit_disabled, "error_banner": err_81}
    }));

    let mut ch_arch = ChannelConversationState::new("ch-arch", "Archived Squad", vec!["bot-a".into()], Some("bot-a".into()));
    ch_arch.set_archived(true);
    let arch_menu = ch_arch.context_menu_items();
    insert_scenario(sc, "f11_archived_channel", "channel_stage", json!({
        "archived": ch_arch.archived, "read_only": ch_arch.read_only,
        "composer_hidden": ch_arch.read_only, "sidebar_drag_disabled": ch_arch.archived,
        "context_menu_items": arch_menu,
    }));

    let ch_missing = ChannelConversationState::new("ch-del", "Squad", vec!["bot-a".into()], Some("bot-a".into()));
    let deleted_label = ch_missing.resolve_author_label("deleted-bot-99");
    let reply_target_missing = ch_missing.is_reply_target_unavailable("msg-deleted-99");
    insert_scenario(sc, "f11_deleted_author_missing_reply", "channel_stage", json!({
        "deleted_author_label": deleted_label,
        "reply_quote": {"unavailable_dashed": reply_target_missing},
        "reactions_enabled": ch_missing.reactions_enabled,
        "mark_unread_enabled": ch_missing.mark_unread_enabled,
        "thread_branch_enabled": ch_missing.thread_branch_enabled,
    }));

    let mut act_empty = ChannelActivityIndicatorState::default();
    act_empty.sync_workers(&[], 0, false);
    let snap_empty = act_empty.snapshot(0);
    let mut act_rm = ChannelActivityIndicatorState::default();
    act_rm.sync_workers(&[worker("bot-a", "ResearchBot")], 0, true);
    let snap_rm = act_rm.snapshot(4_000);
    insert_scenario(sc, "f11_activity_empty", "channel_stage", json!({"activity_indicator": {"visible": snap_empty.visible}}));
    insert_scenario(sc, "f11_activity_reduced_motion", "channel_stage", json!({"activity_indicator": {"visible": snap_rm.visible, "shimmer_animated": snap_rm.shimmer_animated}}));

    // UI-F12 Composer
    root.insert("composer".into(), strip_json("", 24.0, 0, false, false, false, true, false, &dark));
    insert_scenario(sc, "f12_multiline_card", "composer", strip_json("Line 1\nLine 2", 64.0, 0, false, false, false, true, false, &dark));
    insert_scenario(sc, "f12_attachment_card", "composer", strip_json("See attached", 24.0, 1, false, false, false, true, false, &dark));

    let mut c_menu = ComposerState::new(false);
    c_menu.toggle_add_menu();
    insert_scenario(sc, "f12_add_menu", "composer", strip_json("", 24.0, 0, false, false, false, true, c_menu.add_menu_open, &dark));
    insert_scenario(sc, "f12_draft_send", "composer", strip_json("Run build check", 24.0, 0, false, false, false, true, false, &dark));
    insert_scenario(sc, "f12_active_stop", "composer", strip_json("", 24.0, 0, true, false, false, true, false, &dark));

    let mut cmp_ime = CanonicalComposer::new();
    cmp_ime.ime_machine.on_ime_start();
    cmp_ime.ime_machine.on_ime_update("你好", 6);
    cmp_ime.ime_machine.on_ime_commit("你好");
    let act_commit = cmp_ime.ime_machine.handle_enter_key(false);
    let commit_sent = usize::from(act_commit == KeyAction::SendMessage);
    let commit_draft = cmp_ime.ime_machine.text_buffer.clone();
    cmp_ime.ime_machine.last_commit_at = Instant::now().checked_sub(Duration::from_millis(200));
    let act_send = cmp_ime.ime_machine.handle_enter_key(false);
    let send_count = usize::from(act_send == KeyAction::SendMessage);
    if send_count > 0 {
        cmp_ime.ime_machine.text_buffer.clear();
    }
    insert_scenario(sc, "f12_ime_commit", "composer", json!({"sent_count": commit_sent, "draft_text": commit_draft}));
    insert_scenario(sc, "f12_ime_send", "composer", json!({"sent_count": send_count, "draft_text": cmp_ime.ime_machine.text_buffer}));
    insert_scenario(sc, "f12_dictation_mic", "composer", strip_json("", 24.0, 0, false, false, true, true, false, &dark));

    let mut c4000 = ComposerState::new(false);
    let r4000 = c4000.paste_text(&"a".repeat(4000));
    let mut c4001 = ComposerState::new(false);
    let r4001 = c4001.paste_text(&"a".repeat(4001));
    insert_scenario(sc, "f12_paste_4000", "composer", json!({"pasted_as_file": r4000.pasted_as_file, "inline_text_length": r4000.inline_text_length}));
    let l4001 = compute_openbot_composer_strip_layout(&c4001.text, 24.0, c4001.attachments.len(), false, false, false, true, &dark);
    insert_scenario(sc, "f12_paste_4001", "composer", json!({
        "pasted_as_file": r4001.pasted_as_file,
        "pasted_file_name": r4001.pasted_file_name.unwrap_or_default(),
        "border_radius_px": l4001.border_radius_px as i32,
    }));

    let mut c_cap = ComposerState::new(false);
    for i in 0..10 {
        let _ = c_cap.add_attachment(ComposerAttachmentDraft::from_raw(format!("f{i}.txt"), 1024, "file"), false);
    }
    insert_scenario(sc, "f12_attach_exact_cap", "composer", json!({
        "attachment_accepted": c_cap.validate_current_attachments(false).is_ok(),
        "attachments_count": c_cap.attachments.len(),
    }));
    let over_res = c_cap.add_attachment(ComposerAttachmentDraft::from_raw("f11.txt", 1024, "file"), false);
    insert_scenario(sc, "f12_attach_over_cap", "composer", json!({
        "attachment_accepted": over_res.is_ok(),
        "attachments_count": c_cap.attachments.len(),
    }));

    let mut c_img = ComposerState::new(false);
    c_img.set_custom_pi_text_only(true);
    let _ = c_img.add_attachment(ComposerAttachmentDraft::from_raw("shot.png", 2048, "image"), false);
    insert_scenario(sc, "f12_text_only_image_warn", "composer", json!({"image_model_warning": c_img.image_warning_banner_visible(), "image_inputs_enabled": c_img.image_inputs_enabled}));
    c_img.click_allow_images();
    insert_scenario(sc, "f12_text_only_image_allowed", "composer", json!({"image_model_warning": c_img.image_warning_banner_visible(), "image_inputs_enabled": c_img.image_inputs_enabled}));

    let mut c_ws = ComposerState::new(false);
    c_ws.text = "   \t  ".into();
    let l_ws = compute_openbot_composer_strip_layout(&c_ws.text, 24.0, 0, false, false, false, true, &dark);
    insert_scenario(sc, "f12_whitespace_submit", "composer", json!({
        "sent_count": usize::from(c_ws.can_send()), "action_circle_kind": l_ws.action_circle_kind,
    }));

    let mut c_busy = ComposerState::new(false);
    c_busy.active_turn_running = true;
    let busy_mode = c_busy.send_mode();
    let mut busy_insp = ContextInspector::new();
    busy_insp.bot_is_busy = busy_mode == ComposerSendMode::Stop;
    let busy_new_chat = busy_insp.try_start_new_chat_from_settings();
    insert_scenario(sc, "f12_busy_new_chat", "composer", json!({
        "new_chat_blocked_while_busy": busy_new_chat.new_chat_busy_toast && !busy_new_chat.chat_reset,
        "active_turn_preserved": busy_mode == ComposerSendMode::Stop && c_busy.active_turn_running,
    }));

    let mut cmp_esc = CanonicalComposer::new();
    cmp_esc.ime_machine.on_ime_start();
    cmp_esc.ime_machine.on_ime_update("ni", 2);
    cmp_esc.ime_machine.on_ime_cancel();
    let esc_sent = usize::from(cmp_esc.ime_machine.handle_enter_key(false) == KeyAction::SendMessage);
    insert_scenario(sc, "f12_ime_escape_cancel", "composer", json!({
        "ime_composing": matches!(cmp_esc.ime_machine.state, ImeState::Composing { .. }),
        "sent_count": esc_sent,
    }));
    let mut cmp_blur = CanonicalComposer::new();
    cmp_blur.is_input_focused = true;
    cmp_blur.ime_machine.on_ime_start();
    cmp_blur.ime_machine.on_ime_update("hao", 3);
    let blur_changed = cmp_blur.on_focus_lost();
    assert!(blur_changed && !cmp_blur.is_input_focused);
    let blur_sent = usize::from(cmp_blur.ime_machine.handle_enter_key(false) == KeyAction::SendMessage);
    insert_scenario(sc, "f12_ime_focus_loss", "composer", json!({
        "ime_composing": matches!(cmp_blur.ime_machine.state, ImeState::Composing { .. }),
        "sent_count": blur_sent,
    }));

    let l_dis = compute_openbot_composer_strip_layout("", 24.0, 0, false, true, false, false, &dark);
    let mut c_dis = ComposerState::new(false);
    c_dis.channel_archived_read_only = true;
    let opened_dis = c_dis.try_toggle_add_menu(true);
    insert_scenario(sc, "f12_composer_disabled", "composer", json!({
        "add_trigger_disabled": l_dis.add_btn_disabled,
        "add_menu_open": opened_dis || c_dis.add_menu_open,
    }));
    let dm_menu = compute_composer_add_menu_layout(false, false);
    let dm_items: Vec<&str> = dm_menu.items.iter().map(|i| i.id()).collect();
    insert_scenario(sc, "f12_human_dm_add_menu", "composer", json!({
        "add_menu_items": dm_items,
        "leading_or_trailing_separator": dm_menu.leading_or_trailing_separator,
    }));
}

pub fn populate(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    populate_channel_stage(root, sc);
    populate_channel_guards_and_composer(root, sc);
}
