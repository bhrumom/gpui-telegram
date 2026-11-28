//! Tier 4 Full End-to-End User Journeys (`j01`..`j12`).

use gpui_telegram_desktop::components::bloub_avatar::{
    ChannelAvatarLayout, resolve_channel_cluster,
};
use gpui_telegram_desktop::theme::openbot_palette::{
    OpenBotColorScheme, OpenBotPalette, readable_chat_background,
};
use gpui_telegram_desktop::views::app_shell::openbot_shell_layout::{
    OpenBotShellFrameInput, compute_openbot_shell_frame_layout, default_openbot_shell_wiring,
    evaluate_headless_shell_layout_suite, resolve_right_drawer_slot,
};
use gpui_telegram_desktop::views::app_shell::shell_bot_fusion::OpenBotRightSidePanelState;
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleAppSidebar, AppleSidebarSection, load_page_rail_pins, save_page_rail_prefs,
};
use gpui_telegram_desktop::views::boxes::modal_theme::{
    ConfirmDialogState, compute_confirm_dialog_layout,
};
use gpui_telegram_desktop::views::composer::composer_ui::{
    compute_openbot_composer_strip_layout, extract_composer_inline_chips,
};
use gpui_telegram_desktop::views::context_inspector::{BotIdentityAutosaveState, ContextInspector};
use gpui_telegram_desktop::views::conversation_stage::{
    ConversationStage, compute_agent_activity_layout,
};
use gpui_telegram_desktop::views::drawer::bot_computer_inspector::compute_openbot_bot_settings_layout;
use gpui_telegram_desktop::views::drawer::inspector_panels::compute_openbot_channel_settings_layout;
use gpui_telegram_desktop::views::openbot::agent_studio::{
    FirstAgentSetupState, compute_first_agent_setup_layout,
};
use gpui_telegram_desktop::views::openbot::channel_view::{
    ChannelConversationState, ChannelStoppedTasksState, StoppedChannelTaskBannerItem,
};
use gpui_telegram_desktop::views::openbot::composer::{
    ComposerState, DollarPickerItem, DollarPickerItemKind,
};
use gpui_telegram_desktop::views::openbot::queue_and_voice::{
    ComposerStackInput, compute_composer_stack_layout, evaluate_steer,
};
use gpui_telegram_desktop::views::openbot::settings_and_shortcuts::{
    ShortcutChord, ShortcutRegistry,
};
use gpui_telegram_desktop::views::openbot::side_panel::{
    LAUNCHER_PRIMARY_PANES, SidePanelPaneKind, SidePanelSettingsTarget,
};
use gpui_telegram_desktop::views::openbot::workspace_shell::OpenBotWorkspaceShell;
use gpui_telegram_desktop::views::stage::stage_bars::{
    compute_chat_search_bar_layout, compute_openbot_header_layout,
};
use openbot_contracts::channels::ChannelMember;
use serde_json::{Map, Value, json};

use super::f09_f10::default_header_props;
use super::{dark_palette, insert_scenario, light_palette};

fn sec_id(s: AppleSidebarSection) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

pub fn populate(_root: &Map<String, Value>, sc: &mut Map<String, Value>) {
    let dark = dark_palette();
    let light = light_palette();
    let dark_ob = OpenBotPalette::dark();
    let wiring = default_openbot_shell_wiring();
    let empty_tabs: Vec<(String, String)> = vec![];

    let mut ws01 = OpenBotWorkspaceShell::new();
    ws01.first_agent_setup = FirstAgentSetupState::first_run();
    ws01.first_agent_setup.name = "Launch Assistant".into();
    let _ = ws01.first_agent_setup.set_purpose("Manage launches");
    let l01 = compute_first_agent_setup_layout(&ws01.first_agent_setup, &dark_ob, "zh-CN");
    let cid01 = ws01.submit_first_agent_setup().ok();
    insert_scenario(sc, "j01_onboarding", "agent_studio", json!({
        "form_width_px": l01.form_width_px as i32, "hero": {"size_px": l01.hero_size_px as i32},
        "created_agent_id": cid01, "navigated_to_chat": cid01.is_some(),
    }));

    let l02 = compute_openbot_composer_strip_layout("", 24.0, 0, true, false, false, true, &dark);
    insert_scenario(sc, "j02_active_bot_turn", "composer", json!({
        "height_px": l02.height_px as i32, "border_radius_px": l02.border_radius_px as i32,
        "action_circle_kind": l02.action_circle_kind,
        "legacy_chip_strip_present": l02.legacy_chip_strip_present,
        "focus_border_is_telegram_blue": l02.focus_border_is_telegram_blue,
    }));

    let mut sp03 = OpenBotRightSidePanelState::default();
    sp03.return_to_launcher();
    let slot03 = resolve_right_drawer_slot(
        wiring, true, false, dark.is_apple_style(), sp03.state.open, sp03.state.maximized,
        sp03.state.width_px, sp03.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant,
    );
    let asst_btns03: Vec<&str> = LAUNCHER_PRIMARY_PANES
        .iter()
        .map(|p| p.launcher_label(SidePanelSettingsTarget::Assistant, true))
        .collect();
    let mut insp03 = ContextInspector::new();
    insp03.sync_inspector_for_unified_side_panel(true);
    insp03.profile.is_bot = true;
    insp03.profile.name = "Research Assistant".into();
    insp03.profile.username = "research_bot".into();
    let _ = insp03.trigger_copy_bot_handle();
    let prof03 = compute_openbot_bot_settings_layout(&insp03, &dark);
    let cd03 = ConfirmDialogState::open_dialog(true);
    let cd03_l = compute_confirm_dialog_layout(&cd03, &dark);
    let defects03 = evaluate_headless_shell_layout_suite().defects;
    insert_scenario(sc, "j03_bot_profile_flow", "side_panel", json!({
        "active_right_drawer_count": slot03.active_right_drawer_count,
        "launcher_buttons": asst_btns03,
    }));
    insert_scenario(sc, "j03_bot_profile_flow", "agent_settings_profile", json!({
        "mounted": prof03.mounted,
        "host": prof03.host,
        "avatar_px": prof03.hero_avatar_px as i32,
        "avatar_popover_width_px": prof03.avatar_editor_width_px as i32,
        "handle_copied": prof03.handle_copied,
        "autosave_debounce_ms": prof03.autosave_debounce_ms,
        "context_inspector_live_reachable": prof03.context_inspector_live_reachable,
    }));
    insert_scenario(sc, "j03_bot_profile_flow", "settings_and_overlays", json!({
        "confirm_dialog": {"initial_focus": cd03_l.initial_focus}
    }));
    insert_scenario(sc, "j03_bot_profile_flow", "defects_audit", json!({
        "d11_double_right_drawers": defects03.d11_double_right_drawers
    }));

    let m4: Vec<ChannelMember> = (0..4).map(|i| ChannelMember { agent_id: format!("gm-{i}") }).collect();
    let ev4 = resolve_channel_cluster(&m4, ChannelAvatarLayout::Cluster, None, None, false);
    let st04 = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None,
        awaiting_tasks: &["gm-0".into()], awaiting_all_settled: false,
        stopped_tasks: &["t1".into()], open_waits_count: 0, notice: None,
    });
    let mut stopped04 = ChannelStoppedTasksState::new(vec![StoppedChannelTaskBannerItem {
        task_id: "t1".into(),
        owner_agent_id: Some("gm-0".into()),
        error: Some("paused".into()),
        run_agent_ids: vec!["gm-0".into()],
        is_assignment_loop: false,
    }]);
    let _ = stopped04.begin_resume("t1");
    let _ = stopped04.begin_resume("t1");
    let resuming04: Vec<String> = stopped04.resuming_ids.iter().cloned().collect();
    insert_scenario(sc, "j04_group_lifecycle", "channel_stage", json!({
        "avatar": {"visible_faces": ev4.shown_faces, "overflow_count": ev4.hidden_count, "overflow_pill_tone": ev4.overflow_pill_tone.as_str()}
    }));
    insert_scenario(sc, "j04_group_lifecycle", "composer_stack", json!({
        "awaiting_replies": {"visible": st04.awaiting_visible},
        "stopped_tasks": {"resume_dispatch_count": stopped04.resume_dispatch_count, "resuming_ids": resuming04},
    }));

    let mut ch_insp05 = ContextInspector::new();
    ch_insp05.sync_inspector_for_unified_side_panel(true);
    ch_insp05.members.is_group_or_channel = true;
    ch_insp05.channel_context_isolated = true;
    let ch_l05 = compute_openbot_channel_settings_layout(&ch_insp05);
    let mut ch_arch05 = ChannelConversationState::new("ch-j05", "Archived", vec!["a1".into()], Some("a1".into()));
    ch_arch05.set_archived(true);
    insert_scenario(sc, "j05_group_settings_lifecycle", "conversation_header", json!({"isolation_lock_visible": ch_l05.context_isolated}));
    insert_scenario(sc, "j05_group_archived", "channel_stage", json!({"read_only": ch_arch05.read_only, "composer_hidden": ch_arch05.read_only}));

    let mut c06 = ComposerState::new(false);
    let r06 = c06.paste_text(&"p".repeat(4001));
    c06.accept_dollar_item(&DollarPickerItem { kind: DollarPickerItemKind::Skill, id: "web-search".into(), name: "Search".into(), display_label: "Search".into(), slug: Some("web-search".into()), rank: 0 });
    let chips06: Vec<Value> = extract_composer_inline_chips(&c06.text, &dark).into_iter().map(|c| json!({"kind": c.kind})).collect();
    let steer06 = evaluate_steer(false, true);
    let idle06 = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None, awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    let stage06 = ConversationStage::new();
    let mut sp06 = OpenBotRightSidePanelState::default();
    sp06.open_pane(SidePanelPaneKind::Question);
    let slot06 = resolve_right_drawer_slot(wiring, true, false, dark.is_apple_style(), sp06.state.open, sp06.state.maximized, sp06.state.width_px, sp06.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant);
    insert_scenario(sc, "j06_queue_paste_sideq", "composer", json!({"pasted_as_file": r06.pasted_as_file, "pasted_file_name": r06.pasted_file_name.unwrap_or_default()}));
    insert_scenario(sc, "j06_queue_paste_sideq", "mention_picker", json!({"inserted_chips": chips06}));
    insert_scenario(sc, "j06_queue_paste_sideq", "composer_stack", json!({"queue": {"steered_on_tab": steer06.steered_on_tab}, "height_px": idle06.height_px as i32}));
    insert_scenario(sc, "j06_queue_paste_sideq", "side_panel", json!({"main_msg_delta": stage06.messages.len(), "active_right_drawer_count": slot06.active_right_drawer_count}));

    let mut sp07 = OpenBotRightSidePanelState::default();
    for p in SidePanelPaneKind::ALL {
        sp07.open_pane(p);
    }
    sp07.split_down(SidePanelPaneKind::Files);
    let w720 = sp07.clamp_width_for_workspace(720.0, 1108.0);
    let l07 = sp07.compute_layout(1108.0, 640.0);
    let h07 = sp07.compute_header_layout(SidePanelSettingsTarget::Assistant, 360.0, true);
    let slot07 = resolve_right_drawer_slot(wiring, true, false, dark.is_apple_style(), sp07.state.open, sp07.state.maximized, w720, sp07.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant);
    let cs720 = compute_chat_search_bar_layout(true, "search", 0, 1, w720);
    insert_scenario(sc, "j07_side_panel_splits", "side_panel", json!({
        "width_px": w720 as i32, "active_splits": sp07.state.active_split_ids(),
        "max_splits": l07.max_splits, "overflow_chevron_visible": h07.overflow_chevron_visible,
        "active_right_drawer_count": slot07.active_right_drawer_count,
    }));
    insert_scenario(sc, "j07_side_panel_splits", "chat_stream", json!({"chat_search": {"right_offset_px": cs720.right_offset_px as i32}}));

    let tmp08 = tempfile::tempdir().expect("tempdir j08");
    let mut r08 = AppleAppSidebar::new(dark, "zh-CN");
    r08.toggle_pin_local(AppleSidebarSection::MyCreations);
    save_page_rail_prefs(tmp08.path(), &r08.pinned_sections, &r08.hidden_bottom_items).expect("save j08 pins");
    let pins08: Vec<String> = load_page_rail_pins(tmp08.path()).into_iter().map(sec_id).collect();
    insert_scenario(sc, "j08_all_hubs_flow", "page_rail", json!({"persisted_pins": pins08}));

    let mut p09 = default_header_props(&empty_tabs);
    p09.sidebar_closed = true; p09.topbar_controls_right_edge_px = 44.0;
    let h09 = compute_openbot_header_layout(&p09, &dark);
    let res_err = ShortcutRegistry::validate_chord("darwin", &ShortcutChord::new(false, false, false, true, "KeyQ")).err();
    let refusal09 = if res_err.as_deref().is_some_and(|m| m.to_ascii_lowercase().contains("reserved")) { "reserved" } else { "" };
    insert_scenario(sc, "j09_power_nav", "conversation_header", json!({"shifted_start_px": h09.header_rect.x as i32, "overlaps_timeline": h09.overlaps_timeline}));
    insert_scenario(sc, "j09_power_nav", "settings_and_overlays", json!({"shortcuts": {"last_refusal": refusal09}}));

    let f10 = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(1440.0, 900.0, light));
    let lt_ok = readable_chat_background("#e5a50a", OpenBotColorScheme::Light).unwrap_or_default();
    let act10_rm = compute_agent_activity_layout(true, 8, false, true);
    insert_scenario(sc, "j10_appearance_light_oklab", "shell", json!({"outer_frame_bg_hex": f10.frame_bg_hex}));
    insert_scenario(sc, "j10_appearance_light_oklab", "tokens", json!({"oklab_samples": [{"hex": "#e5a50a", "scheme": "light", "result_hex": lt_ok}]}));
    insert_scenario(sc, "j10_appearance_light_oklab", "chat_stream", json!({"activity": {"shimmer_frozen_reduced_motion": act10_rm.shimmer_frozen_reduced_motion}}));

    let mut sw11 = BotIdentityAutosaveState::new("bot-a", "BotA", "Bio");
    sw11.edit_name("BotA-Edited", 100);
    let _ = sw11.flush_before_agent_switch("human-alice", "Alice", "Human");
    let hum_btns: Vec<&str> = LAUNCHER_PRIMARY_PANES.iter().map(|p| p.launcher_label(SidePanelSettingsTarget::Contact, true)).collect();
    let mut sp11 = OpenBotRightSidePanelState::default();
    sp11.return_to_launcher();
    let slot11 = resolve_right_drawer_slot(wiring, true, false, dark.is_apple_style(), sp11.state.open, sp11.state.maximized, sp11.state.width_px, sp11.is_settings_visible(), true, true, SidePanelSettingsTarget::Contact);
    insert_scenario(sc, "j11_human_dm_and_flush", "side_panel", json!({"launcher_buttons": hum_btns, "active_right_drawer_count": slot11.active_right_drawer_count}));
    insert_scenario(sc, "j11_human_dm_and_flush", "agent_settings_profile", json!({
        "dirty_flushed_to_agent_id": sw11.last_flushed_agent_id, "flushed_name": sw11.last_flushed_name,
    }));
}
