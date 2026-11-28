//! Live runtime probe for UI-F13 (@/$ Pickers & ReferenceChips), UI-F14 (Tucked .composer-stack), and UI-F15 (Unified Right SidePanel).

use std::collections::BTreeMap;

use gpui_telegram_desktop::views::app_shell::openbot_shell_layout::{
    default_openbot_shell_wiring, resolve_right_drawer_slot,
};
use gpui_telegram_desktop::views::app_shell::shell_bot_fusion::OpenBotRightSidePanelState;
use gpui_telegram_desktop::views::canonical_composer::CanonicalComposer;
use gpui_telegram_desktop::views::composer::composer_ui::extract_composer_inline_chips;
use gpui_telegram_desktop::views::composer::editor_state::{
    atomic_reference_chip_span_before_cursor, detect_autocomplete_trigger,
};
use gpui_telegram_desktop::views::composer::popups_ui::compute_openbot_picker_popup_layout;
use gpui_telegram_desktop::views::conversation_stage::ConversationStage;
use gpui_telegram_desktop::views::openbot::channel_view::{
    AwaitingRepliesTracker, AwaitingReplyEntry, AwaitingReplyState, ChannelStoppedTasksState,
    StoppedChannelTaskBannerItem,
};
use gpui_telegram_desktop::views::openbot::composer::{
    ComposerSkillEntry, ComposerState, ComposerTrigger, DollarPickerItem, DollarPickerItemKind,
    THREAD_SEARCH_ROWS,
};
use gpui_telegram_desktop::views::openbot::queue_and_voice::{
    ComposerNoticeState, ComposerStackInput, QueuePanelState, compute_composer_stack_layout,
    evaluate_steer,
};
use gpui_telegram_desktop::views::openbot::side_panel::{
    LAUNCHER_PRIMARY_PANES, MAX_SIDE_SPLITS, OPENBOT_LAUNCHER_BUTTON_HEIGHT,
    OPENBOT_LAUNCHER_MAX_WIDTH, SIDE_QUESTION_MAX_TURNS, SidePanelPaneKind,
    SidePanelSettingsTarget, SidePanelState,
};
use gpui_telegram_desktop::views::stage::entities_render::extract_rendered_message_reference_chips;
use gpui_telegram_desktop::views::stage::openbot_stage_chrome::compute_openbot_reference_chip_layout;
use openbot_contracts::tags::ChatTagKind;
use openbot_contracts::queue::QueueSender;
use serde_json::{Map, Value, json};

use super::{dark_palette, insert_scenario};

fn chip_json(c: &gpui_telegram_desktop::views::stage::openbot_stage_chrome::OpenBotReferenceChipLayout) -> Value {
    json!({
        "kind": c.kind, "id": c.id, "label": c.label, "token": c.token,
        "height_px": c.height_px as i32, "radius_px": c.radius_px as i32,
        "bg_hex": c.bg_hex, "clickable": c.clickable, "unavailable": c.unavailable,
        "navigated_on_click": c.try_navigate_on_click(),
    })
}

fn launcher_labels(target: SidePanelSettingsTarget) -> Vec<&'static str> {
    LAUNCHER_PRIMARY_PANES.iter().map(|p| p.launcher_label(target, true)).collect()
}

fn populate_pickers(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let pl_at = compute_openbot_picker_popup_layout(true, '@', 12, 3, &dark);
    let mut c_grp = ComposerState::new(true);
    c_grp.available_agents = vec![("a1".into(), "Éclair".into()), ("a2".into(), "Researcher".into())];
    c_grp.open_picker(ComposerTrigger::At, "every");
    let at_res = c_grp.resolve_active_at_picker();
    let mut c_ev = c_grp.clone();
    c_ev.accept_at_everyone();
    let ev_chips: Vec<Value> = extract_composer_inline_chips(&c_ev.text, &dark).iter().map(chip_json).collect();

    let mut c_dol = ComposerState::new(false);
    c_dol.available_skills = vec![
        ComposerSkillEntry { slug: "web-search".into(), name: "Search".into(), status: "ok".into() },
        ComposerSkillEntry { slug: "doc-search".into(), name: "Search".into(), status: "ok".into() },
        ComposerSkillEntry { slug: "broken-tool".into(), name: "Broken".into(), status: "broken".into() },
    ];
    c_dol.open_picker(ComposerTrigger::Dollar, "");
    let dol_res = c_dol.resolve_active_dollar_picker();
    let skill_status_by_slug: BTreeMap<&str, &str> = c_dol
        .available_skills
        .iter()
        .map(|s| (s.slug.as_str(), s.status.as_str()))
        .collect();
    let dol_items: Vec<Value> = dol_res
        .dollar_items
        .iter()
        .map(|it| {
            let actual_status = skill_status_by_slug
                .get(it.id.as_str())
                .copied()
                .unwrap_or("unknown");
            json!({"slug": it.id, "name": it.name, "status": actual_status})
        })
        .collect();
    let mut c_dol_acc = c_dol.clone();
    c_dol_acc.accept_dollar_item(&DollarPickerItem {
        kind: DollarPickerItemKind::Skill, id: "web-search".into(), name: "Search".into(),
        display_label: "Search (web-search)".into(), slug: Some("web-search".into()), rank: 0,
    });
    let dol_chips: Vec<Value> = extract_composer_inline_chips(&c_dol_acc.text, &dark).iter().map(chip_json).collect();

    let sent_text = "Check @[Researcher](agent:a2) and $skill:web-search and @everyone";
    let rendered: Vec<Value> = extract_rendered_message_reference_chips(sent_text, &dark).iter().map(chip_json).collect();

    root.insert("mention_picker".into(), json!({
        "open": pl_at.open, "trigger": pl_at.trigger.to_string(), "bg_hex": pl_at.bg_hex, "border_hex": pl_at.border_hex,
        "radius_px": pl_at.radius_px as i32, "row_height_px": pl_at.row_height_px as i32,
        "max_visible_rows": pl_at.max_visible_rows, "visible_rows": pl_at.visible_rows,
        "sections": at_res.at_sections,
        "messages_debounce_ms": at_res.messages_debounce_ms, "messages_min_query": at_res.messages_min_query,
        "everyone_shown": at_res.everyone_shown,
        "inserted_chips": ev_chips, "rendered_chips": rendered,
        "results": dol_items, "skill_problems": dol_res.skill_problems,
        "leftover_partial_mention": c_ev.text.contains("@[") || c_ev.text.ends_with('@'),
        "focus_kept_in_composer": c_ev.active_trigger.is_none(),
    }));

    insert_scenario(sc, "f13_dollar_skills", "mention_picker", json!({"trigger": "$", "results": dol_items, "inserted_chips": dol_chips}));

    let mut c_tab = c_grp.clone();
    c_tab.text = "Hi".into();
    c_tab.open_picker(ComposerTrigger::At, "res");
    c_tab.insert_tag_token(ChatTagKind::Agent, "Researcher", "a2");
    let picked_text = c_tab.text.clone();
    let span = atomic_reference_chip_span_before_cursor(&picked_text, picked_text.len());
    let after_bs = span
        .map(|(s, e)| format!("{}{}", &picked_text[..s], &picked_text[e..]))
        .unwrap_or_else(|| picked_text.clone());
    insert_scenario(sc, "f13_tab_accepted", "mention_picker", json!({
        "open": c_tab.active_trigger.is_some(),
        "inserted_chips": extract_composer_inline_chips(&picked_text, &dark).iter().map(chip_json).collect::<Vec<_>>(),
    }));
    insert_scenario(sc, "f13_atomic_backspace", "mention_picker", json!({
        "open": c_tab.active_trigger.is_some(),
        "inserted_chips": extract_composer_inline_chips(&after_bs, &dark).iter().map(chip_json).collect::<Vec<_>>(),
        "leftover_partial_mention": after_bs.contains('@'),
    }));

    let mut c_nfd = c_grp.clone();
    c_nfd.open_picker(ComposerTrigger::At, "eclair");
    let nfd_res = c_nfd.resolve_active_at_picker();
    let nfd_rows: Vec<Value> = nfd_res.agents.iter().map(|a| json!({"name": a.name})).collect();
    insert_scenario(sc, "f13_nfd_eclair", "mention_picker", json!({"results": nfd_rows}));

    for sc_id in ["f13_1to1_bot", "f13_1to1_human", "f13_group_1_member"] {
        let mut c1 = ComposerState::new(sc_id == "f13_group_1_member");
        c1.available_agents = vec![("a1".into(), "Solo".into())];
        c1.open_picker(ComposerTrigger::At, "");
        let r1 = c1.resolve_active_at_picker();
        let chips1: Vec<Value> = extract_composer_inline_chips(&c1.text, &dark).iter().map(chip_json).collect();
        insert_scenario(sc, sc_id, "mention_picker", json!({"everyone_shown": r1.everyone_shown, "inserted_chips": chips1}));
    }

    let mut c_q2 = c_grp.clone();
    c_q2.open_picker(ComposerTrigger::At, "ab");
    let r_q2 = c_q2.resolve_active_at_picker();
    let mut c_q3 = c_grp.clone();
    c_q3.open_picker(ComposerTrigger::At, "abc");
    let r_q3 = c_q3.resolve_active_at_picker();
    insert_scenario(sc, "f13_query_len_2", "mention_picker", json!({"sections": r_q2.at_sections}));
    insert_scenario(sc, "f13_query_len_3", "mention_picker", json!({
        "sections": r_q3.at_sections,
        "messages_debounce_ms": r_q3.messages_debounce_ms,
        "thread_search_rows": THREAD_SEARCH_ROWS,
    }));
    insert_scenario(sc, "f13_skill_problems", "mention_picker", json!({"results": dol_items, "skill_problems": dol_res.skill_problems}));

    let total_15 = 15usize;
    let pl_15 = compute_openbot_picker_popup_layout(true, '@', total_15, 3, &dark)
        .with_scroll_offset(total_15, 0);
    insert_scenario(sc, "f13_overflow_15_rows", "mention_picker", json!({
        "visible_rows": pl_15.visible_rows, "row_height_px": pl_15.row_height_px as i32,
        "scroll_enabled": pl_15.scroll_enabled, "fade_top": pl_15.fade_top, "fade_bottom": pl_15.fade_bottom,
    }));
    let unav_chip = compute_openbot_reference_chip_layout("agent", "gone", "DeletedBot", "@[DeletedBot](agent:gone)", true, &dark);
    insert_scenario(sc, "f13_unavailable_chip", "mention_picker", json!({"rendered_chips": [chip_json(&unav_chip)]}));

    let email_open = detect_autocomplete_trigger("user@example.com", 16).is_some();
    let curr_open = detect_autocomplete_trigger("cost is $100", 12).is_some();
    insert_scenario(sc, "f13_mid_word_email_currency", "mention_picker", json!({"open": email_open || curr_open}));

    let mut c_esc = c_grp.clone();
    c_esc.open_picker(ComposerTrigger::At, "re");
    c_esc.close_picker();
    insert_scenario(sc, "f13_escape_dismissed", "mention_picker", json!({
        "open": c_esc.active_trigger.is_some(), "focus_kept_in_composer": c_esc.active_trigger.is_none(),
    }));
}

fn populate_stack_and_side_panel(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let dark_ob = dark.openbot_palette();
    let wiring = default_openbot_shell_wiring();

    // UI-F14 Tucked .composer-stack
    let idle_st = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None, awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    let mut qp = QueuePanelState::default();
    let _ = qp.push_item("d1", "M1", QueueSender::User);
    let _ = qp.push_item("d2", "M2", QueueSender::User);
    let _ = qp.push_item("d3", "M3", QueueSender::User);
    let _ = qp.request_delete("d1", true);
    let _ = qp.reorder(1, 0);
    let _ = qp.begin_edit_at("d2", 100);
    let _ = qp.save_edit("M2-edited");
    let q_texts: Vec<String> = qp.items.iter().map(|i| i.text.clone()).collect();
    let q_act = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &q_texts, queue_can_steer: true, reply_sender: None, awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    let steer_ok = evaluate_steer(false, true);
    let steer_no = evaluate_steer(true, true);

    let idle_stopped = ChannelStoppedTasksState::default();
    let idle_resuming: Vec<String> = idle_stopped.resuming_ids.iter().cloned().collect();
    root.insert("composer_stack".into(), json!({
        "visible": idle_st.visible, "height_px": idle_st.height_px as i32,
        "radius_px": idle_st.radius_px as i32, "overlap_bottom_px": idle_st.overlap_bottom_px as i32,
        "z_index": idle_st.z_index, "composer_z_index": idle_st.composer_z_index,
        "queue": {"row_height_px": q_act.queue_row_height_px as i32, "items": q_act.queue_items, "visible_rows": q_act.queue_visible_rows, "can_steer": steer_ok.can_steer, "steered_on_tab": steer_ok.steered_on_tab, "steer_refused": steer_ok.steer_refused},
        "reply_preview": {"visible": idle_st.reply_preview_visible, "min_height_px": idle_st.reply_preview_min_height_px as i32, "sender": idle_st.reply_preview_sender},
        "awaiting_replies": {"visible": idle_st.awaiting_visible, "title": idle_st.awaiting_title, "tasks": idle_st.awaiting_tasks, "dismiss_available": idle_st.awaiting_dismiss_available},
        "stopped_tasks": {"visible": idle_st.stopped_visible, "tasks": idle_st.stopped_tasks, "resume_dispatch_count": idle_stopped.resume_dispatch_count, "resuming_ids": idle_resuming},
        "open_waits_count": idle_st.open_waits_count,
        "notice": {"visible": idle_st.notice_visible, "has_copy_details": idle_st.notice_has_copy_details, "truncated": idle_st.notice_truncated},
    }));

    insert_scenario(sc, "f14_stack_active", "composer_stack", json!({
        "visible": q_act.visible, "height_px": q_act.height_px as i32, "radius_px": q_act.radius_px as i32,
        "overlap_bottom_px": q_act.overlap_bottom_px as i32, "z_index": q_act.z_index, "composer_z_index": q_act.composer_z_index,
    }));
    insert_scenario(sc, "f14_steer_refused_routine", "composer_stack", json!({
        "queue": {"can_steer": steer_no.can_steer, "steer_refused": steer_no.steer_refused, "steered_on_tab": steer_no.steered_on_tab, "items": q_texts}
    }));

    let ten_q: Vec<String> = (1..=10).map(|i| format!("Item {i}")).collect();
    let q10 = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &ten_q, queue_can_steer: true, reply_sender: None, awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    insert_scenario(sc, "f14_queue_10_items_clamp", "composer_stack", json!({
        "queue": {"visible_rows": q10.queue_visible_rows, "items": q10.queue_items, "row_height_px": q10.queue_row_height_px as i32}
    }));

    let mut cmp_rp = CanonicalComposer::new();
    cmp_rp.begin_reply(10, "BotA".into(), "Draft ready".into());
    let rp_show = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true,
        reply_sender: cmp_rp.replying_to.as_ref().map(|(_, s, _)| s.as_str()),
        awaiting_tasks: &[], awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    insert_scenario(sc, "f14_reply_shown", "composer_stack", json!({
        "visible": rp_show.visible, "height_px": rp_show.height_px as i32,
        "reply_preview": {"visible": rp_show.reply_preview_visible, "min_height_px": rp_show.reply_preview_min_height_px as i32, "sender": rp_show.reply_preview_sender},
    }));
    cmp_rp.cancel_reply_or_edit();
    let rp_clr = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true,
        reply_sender: cmp_rp.replying_to.as_ref().map(|(_, s, _)| s.as_str()),
        awaiting_tasks: &[], awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    insert_scenario(sc, "f14_reply_dismissed", "composer_stack", json!({
        "visible": rp_clr.visible, "height_px": rp_clr.height_px as i32,
        "reply_preview": {"visible": rp_clr.reply_preview_visible},
    }));

    let mut aw_tracker = AwaitingRepliesTracker::default();
    for (id, name) in [("a1", "BotA"), ("a2", "BotB"), ("a3", "BotC")] {
        aw_tracker.upsert_entry(AwaitingReplyEntry {
            agent_id: id.into(), agent_name: name.into(), state: AwaitingReplyState::Working,
            preview: None, delivery_id: None, waiting_reason: None,
        });
    }
    let aw_names: Vec<String> = aw_tracker.entries.iter().map(|e| e.agent_name.clone()).collect();
    let aw_act = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None, awaiting_tasks: &aw_names,
        awaiting_all_settled: aw_tracker.all_settled(), stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    for e in &mut aw_tracker.entries {
        e.state = AwaitingReplyState::Replied;
    }
    let aw_set = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None, awaiting_tasks: &aw_names,
        awaiting_all_settled: aw_tracker.all_settled(), stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    insert_scenario(sc, "f14_awaiting_subtasks", "composer_stack", json!({
        "awaiting_replies": {"visible": aw_act.awaiting_visible, "title": aw_act.awaiting_title, "tasks": aw_act.awaiting_tasks},
        "z_index": aw_act.z_index, "composer_z_index": aw_act.composer_z_index,
    }));
    insert_scenario(sc, "f14_awaiting_active", "composer_stack", json!({"awaiting_replies": {"visible": aw_act.awaiting_visible, "dismiss_available": aw_act.awaiting_dismiss_available}}));
    insert_scenario(sc, "f14_awaiting_settled", "composer_stack", json!({"awaiting_replies": {"visible": aw_set.awaiting_visible, "dismiss_available": aw_set.awaiting_dismiss_available}}));

    let mut stopped_ctrl = ChannelStoppedTasksState::new(vec![
        StoppedChannelTaskBannerItem { task_id: "t1".into(), owner_agent_id: Some("a1".into()), error: Some("paused".into()), run_agent_ids: vec!["a1".into()], is_assignment_loop: false },
        StoppedChannelTaskBannerItem { task_id: "t2".into(), owner_agent_id: Some("a2".into()), error: Some("paused".into()), run_agent_ids: vec!["a2".into()], is_assignment_loop: false },
    ]);
    let stopped_ids: Vec<String> = stopped_ctrl.items.iter().map(|i| i.task_id.clone()).collect();
    let long_notice = ComposerNoticeState { title: Some("Error".into()), body: "x".repeat(320), reference: Some("ref-99".into()), dismissible: true };
    let st_not = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None, awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &stopped_ids, open_waits_count: 25, notice: Some(&long_notice),
    });
    insert_scenario(sc, "f14_stopped_and_notice", "composer_stack", json!({
        "stopped_tasks": {"visible": st_not.stopped_visible, "tasks": st_not.stopped_tasks},
        "open_waits_count": st_not.open_waits_count,
        "notice": {"visible": st_not.notice_visible, "has_copy_details": st_not.notice_has_copy_details, "truncated": st_not.notice_truncated},
    }));

    if let Some(first_id) = stopped_ctrl.items.first().map(|it| it.task_id.clone()) {
        let _ = stopped_ctrl.begin_resume(&first_id);
        let _ = stopped_ctrl.begin_resume(&first_id);
    }
    let resuming_vec: Vec<String> = stopped_ctrl.resuming_ids.iter().cloned().collect();
    insert_scenario(sc, "f14_stopped_double_click", "composer_stack", json!({
        "stopped_tasks": {
            "resume_dispatch_count": stopped_ctrl.resume_dispatch_count,
            "resuming_ids": resuming_vec,
        }
    }));
    insert_scenario(sc, "f14_stopped_archived", "composer_stack", json!({"stopped_tasks": {"visible": stopped_ctrl.is_visible(true)}}));

    let multi_st = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: Some("BotA"), awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: Some(&long_notice),
    });
    insert_scenario(sc, "f14_multi_cards", "composer_stack", json!({
        "visible": multi_st.visible, "reply_preview": {"visible": multi_st.reply_preview_visible}, "notice": {"visible": multi_st.notice_visible},
    }));
    insert_scenario(sc, "f14_multi_cards_cleared", "composer_stack", json!({"visible": idle_st.visible, "height_px": idle_st.height_px as i32}));
    insert_scenario(sc, "f14_waits_cap_20_and_long_notice", "composer_stack", json!({
        "open_waits_count": st_not.open_waits_count,
        "notice": {"visible": st_not.notice_visible, "has_copy_details": st_not.notice_has_copy_details, "truncated": st_not.notice_truncated},
    }));

    // UI-F15 Unified Right SidePanel
    let mut sp = OpenBotRightSidePanelState::default();
    sp.return_to_launcher();
    let sp_l = sp.compute_layout(1108.0, 640.0);
    let sp_h = sp.compute_header_layout(SidePanelSettingsTarget::Assistant, sp_l.width_px, true);
    let slot_root = resolve_right_drawer_slot(wiring, true, false, dark.is_apple_style(), sp_l.open, sp_l.maximized, sp_l.width_px, sp.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant);
    let root_stage = ConversationStage::new();
    let root_msg_before = root_stage.messages.len();
    let mut sp_split_probe = sp.clone();
    sp_split_probe.open_pane(SidePanelPaneKind::Browser);
    let can_split_root = sp_split_probe.try_split_down(SidePanelPaneKind::Question);
    root.insert("side_panel".into(), json!({
        "open": sp_l.open, "launcher_visible": sp_l.launcher_visible,
        "launcher_max_width_px": OPENBOT_LAUNCHER_MAX_WIDTH as i32,
        "launcher_button_height_px": OPENBOT_LAUNCHER_BUTTON_HEIGHT as i32,
        "launcher_button_bg_hex": dark_ob.token("--openbot-bg-surface-raised").unwrap_or(""),
        "launcher_buttons": launcher_labels(SidePanelSettingsTarget::Assistant),
        "active_right_drawer_count": slot_root.active_right_drawer_count,
        "legacy_standalone_inspector_mounted": slot_root.mount_legacy_standalone_inspector,
        "competing_companion_panel_mounted": slot_root.mount_legacy_companion_panel,
        "width_px": sp_l.width_px as i32, "persisted_width_px": sp_l.persisted_width_px as i32,
        "header_height_px": sp_h.height_px as i32,
        "open_tabs": sp_h.open_tabs, "add_pane_menu_items": SidePanelState::available_pane_ids(),
        "maximized": sp_l.maximized, "effective_width_pct": sp_l.effective_width_pct,
        "active_splits": sp.state.active_split_ids(), "max_splits": sp_l.max_splits,
        "main_group_height_px": sp_l.main_group_height_px as i32, "lower_split_height_px": sp_l.lower_split_height_px as i32,
        "split_share": sp_l.lower_share, "main_msg_delta": root_stage.messages.len().saturating_sub(root_msg_before),
        "conversation_width_px": (1108.0 - sp_l.width_px) as i32,
        "split_accepted": can_split_root, "overflow_chevron_visible": sp_h.overflow_chevron_visible,
        "hidden_tabs_count": sp_h.hidden_tabs_count, "combined_actions_menu": sp_h.combined_actions_menu,
        "unavailable_tab_flagged": sp_h.unavailable_tab_flagged, "side_question_turns": sp.state.question.turns.len(),
        "side_question_max_turns": SIDE_QUESTION_MAX_TURNS,
        "side_question_draft_restored": !sp.state.question.draft_question.is_empty(),
    }));

    insert_scenario(sc, "f15_launcher_group", "side_panel", json!({"launcher_buttons": launcher_labels(SidePanelSettingsTarget::Channel)}));
    insert_scenario(sc, "f15_launcher_human", "side_panel", json!({"launcher_buttons": launcher_labels(SidePanelSettingsTarget::Contact)}));

    let mut sp_tabs = OpenBotRightSidePanelState::default();
    sp_tabs.open_pane(SidePanelPaneKind::Browser);
    sp_tabs.open_pane(SidePanelPaneKind::Files);
    let h_tabs = sp_tabs.compute_header_layout(SidePanelSettingsTarget::Assistant, 420.0, true);
    insert_scenario(sc, "f15_tabs_open", "side_panel", json!({
        "header_height_px": h_tabs.height_px as i32, "open_tabs": h_tabs.open_tabs, "launcher_visible": h_tabs.launcher_visible,
    }));
    sp_tabs.close_tab(SidePanelPaneKind::Browser);
    sp_tabs.close_tab(SidePanelPaneKind::Files);
    let l_closed = sp_tabs.compute_layout(1108.0, 640.0);
    let h_closed = sp_tabs.compute_header_layout(SidePanelSettingsTarget::Assistant, 420.0, true);
    insert_scenario(sc, "f15_tabs_all_closed", "side_panel", json!({"open_tabs": h_closed.open_tabs, "launcher_visible": l_closed.launcher_visible}));

    let mut sp_520 = OpenBotRightSidePanelState::default();
    sp_520.open_pane(SidePanelPaneKind::Settings);
    let w520 = sp_520.clamp_width_for_workspace(520.0, 1108.0);
    let l_520 = sp_520.compute_layout(1108.0, 640.0);
    insert_scenario(sc, "f15_resized_520", "side_panel", json!({"width_px": w520 as i32, "persisted_width_px": l_520.persisted_width_px as i32}));
    sp_520.state.toggle_maximize();
    let l_max = sp_520.compute_layout(1108.0, 640.0);
    insert_scenario(sc, "f15_maximized", "side_panel", json!({"maximized": l_max.maximized, "effective_width_pct": l_max.effective_width_pct}));

    let main_stage = ConversationStage::new();
    let msg_before = main_stage.messages.len();
    let mut sp_split = OpenBotRightSidePanelState::default();
    sp_split.open_pane(SidePanelPaneKind::Browser);
    sp_split.split_down(SidePanelPaneKind::Question);
    let _ = sp_split.state.question.append_turn("What does this module do?", "It manages layout.");
    let l_split = sp_split.compute_layout(1108.0, 640.0);
    insert_scenario(sc, "f15_split_and_side_question", "side_panel", json!({
        "active_splits": sp_split.state.active_split_ids(), "max_splits": MAX_SIDE_SPLITS,
        "main_group_height_px": l_split.main_group_height_px as i32, "lower_split_height_px": l_split.lower_split_height_px as i32,
        "split_share": l_split.lower_share, "main_msg_delta": main_stage.messages.len().saturating_sub(msg_before),
    }));

    let mut sp_lo = OpenBotRightSidePanelState::default();
    sp_lo.open_pane(SidePanelPaneKind::Files);
    let w_lo = sp_lo.clamp_width_for_workspace(200.0, 948.0);
    let w_hi = sp_lo.clamp_width_for_workspace(2200.0, 948.0);
    insert_scenario(sc, "f15_drag_200", "side_panel", json!({"width_px": w_lo as i32}));
    insert_scenario(sc, "f15_drag_2200_at_1280", "side_panel", json!({"width_px": w_hi as i32, "conversation_width_px": (948.0 - w_hi) as i32}));

    let third_ok = sp_split.try_split_down(SidePanelPaneKind::Threads);
    let l_after3 = sp_split.compute_layout(1108.0, 640.0);
    insert_scenario(sc, "f15_invalid_third_split", "side_panel", json!({
        "split_accepted": third_ok, "active_splits": sp_split.state.active_split_ids(),
        "main_group_height_px": l_after3.main_group_height_px as i32, "lower_split_height_px": l_after3.lower_split_height_px as i32,
    }));

    let mut sp_ov = OpenBotRightSidePanelState::default();
    for p in SidePanelPaneKind::ALL {
        sp_ov.open_pane(p);
    }
    let h_ov = sp_ov.compute_header_layout(SidePanelSettingsTarget::Assistant, 360.0, true);
    insert_scenario(sc, "f15_overflow_8_tabs", "side_panel", json!({
        "overflow_chevron_visible": h_ov.overflow_chevron_visible,
        "hidden_tabs_count": h_ov.hidden_tabs_count,
        "combined_actions_menu": h_ov.combined_actions_menu,
        "header_height_px": h_ov.height_px as i32,
    }));

    let mut sp_unav = OpenBotRightSidePanelState::default();
    sp_unav.open_pane(SidePanelPaneKind::Files);
    let _ = sp_unav.sync_file_tab_path_availability(
        "files:deleted",
        std::path::Path::new("/nonexistent/openbot_deleted_file.md"),
    );
    let h_unav = sp_unav.compute_header_layout(SidePanelSettingsTarget::Assistant, 420.0, true);
    insert_scenario(sc, "f15_unavailable_file_tab", "side_panel", json!({"unavailable_tab_flagged": h_unav.unavailable_tab_flagged}));

    sp_520.close_tab(SidePanelPaneKind::Settings);
    let l_close_max = sp_520.compute_layout(1108.0, 640.0);
    insert_scenario(sc, "f15_close_last_maximized", "side_panel", json!({"maximized": l_close_max.maximized, "launcher_visible": l_close_max.launcher_visible}));

    let mut sp_q = OpenBotRightSidePanelState::default();
    for i in 0..SIDE_QUESTION_MAX_TURNS {
        let _ = sp_q.state.question.append_turn(format!("Q{i}"), format!("A{i}"));
    }
    sp_q.state.question.draft_question = "Draft 21".into();
    let failed_21 = sp_q.state.question.append_turn("Draft 21", "A21").is_err();
    insert_scenario(sc, "f15_side_question_cap_and_fail", "side_panel", json!({
        "side_question_turns": sp_q.state.question.turns.len(),
        "side_question_max_turns": SIDE_QUESTION_MAX_TURNS,
        "main_msg_delta": main_stage.messages.len().saturating_sub(msg_before),
        "side_question_draft_restored": failed_21 && sp_q.state.question.draft_question == "Draft 21",
    }));

    let mut sp_spam = OpenBotRightSidePanelState::default();
    for i in 0..12 {
        if i % 2 == 0 { sp_spam.open_pane(SidePanelPaneKind::Settings); } else { sp_spam.return_to_launcher(); }
    }
    let slot_spam = resolve_right_drawer_slot(wiring, true, false, dark.is_apple_style(), sp_spam.state.open, sp_spam.state.maximized, sp_spam.state.width_px, sp_spam.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant);
    insert_scenario(sc, "f15_toggle_spam_stress", "side_panel", json!({
        "active_right_drawer_count": slot_spam.active_right_drawer_count,
        "legacy_standalone_inspector_mounted": slot_spam.mount_legacy_standalone_inspector,
        "competing_companion_panel_mounted": slot_spam.mount_legacy_companion_panel,
    }));
}

pub fn populate(root: &mut Map<String, Value>, sc: &mut Map<String, Value>) {
    populate_pickers(root, sc);
    populate_stack_and_side_panel(root, sc);
}
