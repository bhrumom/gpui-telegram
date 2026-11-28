//! Tier 3 Cross-Feature Pairwise live runtime scenarios (`pair01`..`pair24`).

use std::collections::BTreeMap;

use gpui_telegram_desktop::app::preferences::AppPreferences;
use gpui_telegram_desktop::components::bloub_avatar::{
    ChannelAvatarLayout, compute_working_badge_layout, resolve_channel_cluster,
};
use gpui_telegram_desktop::theme::openbot_palette::{
    OpenBotColorScheme, OpenBotPalette, readable_chat_background, solid_bubble_text_hex,
};
use gpui_telegram_desktop::views::app_shell::openbot_shell_layout::{
    OpenBotShellFrameInput, compute_openbot_shell_frame_layout,
    compute_shell_account_footer_audit, configure_openbot_sidebar_for_shell,
    default_openbot_shell_wiring, evaluate_headless_shell_layout_suite, resolve_right_drawer_slot,
};
use gpui_telegram_desktop::views::app_shell::shell_bot_fusion::OpenBotRightSidePanelState;
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleAppSidebar, AppleSidebarSection, OPENBOT_PAGE_RAIL_WIDTH, PINNABLE_SECTIONS,
    load_page_rail_pins, save_page_rail_prefs,
};
use gpui_telegram_desktop::views::boxes::modal_theme::{
    ConfirmDialogState, compute_confirm_dialog_layout,
};
use gpui_telegram_desktop::views::boxes::peers::create_managed_bot_box::CreateManagedBotBoxPeers;
use gpui_telegram_desktop::views::chat_sidebar::{
    ChatSidebar, DialogDeliveryStatus, DialogItemModel,
};
use gpui_telegram_desktop::views::composer::composer_ui::{
    compute_openbot_composer_strip_layout, extract_composer_inline_chips,
};
use gpui_telegram_desktop::views::content_hub::ContentHubSection;
use gpui_telegram_desktop::views::content_hub::apple_primitives::compute_content_hub_shell_layout;
use gpui_telegram_desktop::views::context_inspector::{
    BotIdentityAutosaveState, ContextInspector, DRAWER_WIDTH as LEGACY_INSPECTOR_W,
};
use gpui_telegram_desktop::views::conversation_stage::{
    MessageContent, MessageDirection, MessageItemModel, compute_agent_activity_layout,
};
use gpui_telegram_desktop::views::drawer::bot_computer_inspector::compute_openbot_bot_settings_layout;
use gpui_telegram_desktop::views::drawer::inspector_panels::compute_openbot_channel_settings_layout;
use gpui_telegram_desktop::views::drawer::inspector_tabs::BotPublishedCreationSummary;
use gpui_telegram_desktop::views::openbot::agent_studio::{
    FirstAgentSetupMode, FirstAgentSetupState, GlobalSearchResultItem, GlobalSearchSectionKind,
    GlobalSearchState, GlobalSearchTab, compute_first_agent_setup_layout,
    compute_global_search_modal_layout,
};
use gpui_telegram_desktop::views::openbot::channel_view::{
    ChannelRoutingEvent, compute_routing_action_markers, merge_consecutive_routing_events,
};
use gpui_telegram_desktop::views::openbot::composer::{
    ComposerState, ComposerTrigger, DollarPickerItem, DollarPickerItemKind,
};
use gpui_telegram_desktop::views::openbot::queue_and_voice::{
    ComposerStackInput, compute_composer_stack_layout, evaluate_steer,
};
use gpui_telegram_desktop::views::openbot::side_panel::{
    LAUNCHER_PRIMARY_PANES, SidePanelPaneKind, SidePanelSettingsTarget,
};
use gpui_telegram_desktop::views::openbot::workspace_shell::OpenBotWorkspaceShell;
use gpui_telegram_desktop::views::settings::SettingsRouter;
use gpui_telegram_desktop::views::settings::settings_theme_ui::compute_settings_page_layout;
use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
use gpui_telegram_desktop::views::sidebar::dialogs_row_view::{
    SidebarDialogMeta, compute_openbot_dialog_row_layout,
};
use gpui_telegram_desktop::views::sidebar::openbot_sidebar_chrome::{
    compute_openbot_sidebar_search_row_layout, compute_openbot_sidebar_topbar_layout,
};
use gpui_telegram_desktop::views::stage::cards::models::DeliveryStatus;
use gpui_telegram_desktop::views::stage::entities_render::extract_rendered_message_reference_chips;
use gpui_telegram_desktop::views::stage::stage_bars::{
    compute_chat_search_bar_layout, compute_openbot_header_layout,
};
use gpui_telegram_desktop::views::stage::stage_bubble::{
    BubbleGroupLayout, compute_openbot_bubble_layout,
};
use openbot_contracts::channels::ChannelMember;
use serde_json::{Map, Value, json};

use super::f09_f10::default_header_props;
use super::f18_f20::{verify_disk_ledger_and_capabilities, workspace_root};
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

    let f_dk = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(1440.0, 900.0, dark));
    let f_lt = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(1440.0, 900.0, light));
    insert_scenario(sc, "pair01_dark", "shell", json!({"outer_frame_bg_hex": f_dk.frame_bg_hex, "canvas_card": {"bg_hex": f_dk.canvas_bg_hex, "radius_px": f_dk.workspace_card_radius_px as i32}}));
    insert_scenario(sc, "pair01_light", "tokens", json!({"scheme": light.openbot_palette().scheme.as_str()}));
    insert_scenario(sc, "pair01_light", "shell", json!({"outer_frame_bg_hex": f_lt.frame_bg_hex, "canvas_card": {"bg_hex": f_lt.canvas_bg_hex, "radius_px": f_lt.workspace_card_radius_px as i32}}));

    let y_bg = readable_chat_background("#e5a50a", OpenBotColorScheme::Dark).unwrap_or_default();
    let y_txt = solid_bubble_text_hex("#e5a50a");
    insert_scenario(sc, "pair02_yellow_stream", "tokens", json!({"oklab_samples": [{"hex": "#e5a50a", "scheme": "dark", "result_hex": y_bg}]}));
    insert_scenario(sc, "pair02_yellow_stream", "chat_stream", json!({"bubbles": [{"author": "user", "resolved_text_hex": y_txt}]}));

    let mut ws04 = OpenBotWorkspaceShell::new();
    let mut st04 = FirstAgentSetupState::first_run();
    st04.name = "TriangleBot".into();
    let _ = st04.set_purpose("Helps with research");
    st04.select_face(6);
    let l04 = compute_first_agent_setup_layout(&st04, &dark_ob, "zh-CN");
    ws04.first_agent_setup = st04;
    let cid04 = ws04.submit_first_agent_setup().ok();
    insert_scenario(sc, "pair04_studio_to_plush", "agent_studio", json!({"hero": {"size_px": l04.hero_size_px as i32, "shape": l04.hero_shape}, "created_agent_id": cid04}));
    insert_scenario(sc, "pair04_studio_to_plush", "plush", json!({"profile": {"silhouette": l04.hero_shape}}));

    let sh700 = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(700.0, 800.0, dark));
    let sh1280 = compute_openbot_shell_frame_layout(&OpenBotShellFrameInput::desktop_default(1280.0, 800.0, dark));
    insert_scenario(sc, "pair05_viewport_700", "shell", json!({"canvas_card": {"full_bleed": sh700.narrow_viewport}}));
    insert_scenario(sc, "pair05_viewport_700", "page_rail", json!({"visible": sh700.page_rail_visible}));
    insert_scenario(sc, "pair05_viewport_1280", "shell", json!({"canvas_card": {"radius_px": sh1280.workspace_card_radius_px as i32}}));
    insert_scenario(sc, "pair05_viewport_1280", "page_rail", json!({"visible": sh1280.page_rail_visible, "width_px": sh1280.page_rail_width_px as i32}));

    let tmp07 = tempfile::tempdir().expect("tempdir pair07");
    let mut r07 = AppleAppSidebar::new(dark, "zh-CN");
    r07.toggle_pin_local(AppleSidebarSection::MyCreations);
    save_page_rail_prefs(tmp07.path(), &r07.pinned_sections, &r07.hidden_bottom_items).expect("save pair07 pins");
    let pins07: Vec<String> = load_page_rail_pins(tmp07.path()).into_iter().map(sec_id).collect();
    let entries07: Vec<String> = PINNABLE_SECTIONS.iter().copied().map(sec_id).collect();
    insert_scenario(sc, "pair07_rail_to_hubs", "page_rail", json!({"persisted_pins": pins07, "more_menu": {"entries": entries07}}));

    let mut ws09 = OpenBotWorkspaceShell::new();
    ws09.first_agent_setup = FirstAgentSetupState::new(FirstAgentSetupMode::Additional);
    ws09.first_agent_setup.select_suggestion("trip-planner");
    let l09 = compute_first_agent_setup_layout(&ws09.first_agent_setup, &dark_ob, "zh-CN");
    let cid09 = ws09.submit_first_agent_setup().ok();
    insert_scenario(sc, "pair09_plus_to_studio", "agent_studio", json!({"recommendations": {"hydrated": l09.rec_hydrated}, "created_agent_id": cid09}));

    let mut sb10 = ChatSidebar::new();
    configure_openbot_sidebar_for_shell(&mut sb10, wiring);
    sb10.toggle_closed_from_user();
    let mut inp10 = OpenBotShellFrameInput::desktop_default(1440.0, 900.0, dark);
    inp10.sidebar_closed = sb10.is_sidebar_closed;
    inp10.sidebar_width_px = sb10.rendered_sidebar_width();
    let f10 = compute_openbot_shell_frame_layout(&inp10);
    let tb10 = compute_openbot_sidebar_topbar_layout(
        false,
        false,
        f10.page_kind.as_str() == "agents",
        f10.page_rail_visible,
    );
    let mut p10 = default_header_props(&empty_tabs);
    p10.sidebar_closed = sb10.is_sidebar_closed;
    p10.topbar_controls_right_edge_px = 44.0;
    let h10 = compute_openbot_header_layout(&p10, &dark);
    insert_scenario(sc, "pair10_cmd_b_header_shift", "sidebar_chrome", json!({"mode": if sb10.is_sidebar_closed { "closed" } else { "expanded" }, "topbar": {"toggle_visible": tb10.toggle_visible}}));
    insert_scenario(sc, "pair10_cmd_b_header_shift", "conversation_header", json!({"shifted_start_px": h10.header_rect.x as i32, "overlaps_timeline": h10.overlaps_timeline}));

    let m5: Vec<ChannelMember> = (0..5).map(|i| ChannelMember { agent_id: format!("g-{i}") }).collect();
    let ev5 = resolve_channel_cluster(&m5, ChannelAvatarLayout::Cluster, None, None, false);
    let mut p11 = default_header_props(&empty_tabs);
    p11.is_bot_chat = false; p11.is_group_chat = true;
    let h11 = compute_openbot_header_layout(&p11, &dark);
    insert_scenario(sc, "pair11_group_row_to_stage", "channel_stage", json!({"avatar": {"visible_faces": ev5.shown_faces, "overflow_count": ev5.hidden_count, "overflow_pill_tone": ev5.overflow_pill_tone.as_str()}}));
    insert_scenario(sc, "pair11_group_row_to_stage", "conversation_header", json!({"model_picker": {"visible": h11.model_picker_visible}}));

    let mut gs12 = GlobalSearchState::new(false);
    gs12.open_palette();
    gs12.results = vec![GlobalSearchResultItem { id: "settings:general".into(), section: GlobalSearchSectionKind::Agents, title: "Settings".into(), subtitle: "Open".into(), badge: None, avatar_seed: None, shortcut_hint: None }];
    let _ = gs12.execute_selected();
    let r12 = SettingsRouter::new(dark, true, AppPreferences::default());
    let sp12_l = compute_settings_page_layout(&r12);
    insert_scenario(sc, "pair12_search_to_settings", "global_search", json!({"open": gs12.open}));
    insert_scenario(sc, "pair12_search_to_settings", "settings_and_overlays", json!({"settings_page": {"open": sp12_l.open}}));

    let mut sp14 = OpenBotRightSidePanelState::default();
    sp14.open_pane(SidePanelPaneKind::Settings);
    let w640 = sp14.clamp_width_for_workspace(640.0, 1108.0);
    let cs640 = compute_chat_search_bar_layout(true, "query", 0, 1, w640);
    insert_scenario(sc, "pair14_side_panel_640_cascade", "side_panel", json!({"open": sp14.state.open, "width_px": w640 as i32}));
    insert_scenario(sc, "pair14_side_panel_640_cascade", "chat_stream", json!({"chat_search": {"right_offset_px": cs640.right_offset_px as i32}}));

    let l15 = compute_openbot_composer_strip_layout("", 24.0, 0, true, false, false, true, &dark);
    insert_scenario(sc, "pair15_submit_turn_active", "composer", json!({"action_circle_kind": l15.action_circle_kind}));

    let st17 = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None,
        awaiting_tasks: &["BotA".into()], awaiting_all_settled: false,
        stopped_tasks: &["t1".into()], open_waits_count: 0, notice: None,
    });
    insert_scenario(sc, "pair17_group_workers_stack", "composer_stack", json!({
        "awaiting_replies": {"visible": st17.awaiting_visible},
        "stopped_tasks": {"visible": st17.stopped_visible, "tasks": st17.stopped_tasks},
    }));

    let mut c18 = ComposerState::new(false);
    c18.accept_dollar_item(&DollarPickerItem { kind: DollarPickerItemKind::Skill, id: "web-search".into(), name: "Search".into(), display_label: "Search".into(), slug: Some("web-search".into()), rank: 0 });
    c18.text.push_str("\nLine 2");
    let l18 = compute_openbot_composer_strip_layout(&c18.text, 64.0, 0, false, false, false, true, &dark);
    let chips18: Vec<Value> = extract_composer_inline_chips(&c18.text, &dark).into_iter().map(|c| json!({"kind": c.kind, "height_px": c.height_px as i32})).collect();
    insert_scenario(sc, "pair18_add_skill_multiline", "composer", json!({"compact": l18.compact, "border_radius_px": l18.border_radius_px as i32, "min_height_px": l18.min_height_px as i32}));
    insert_scenario(sc, "pair18_add_skill_multiline", "mention_picker", json!({"inserted_chips": chips18}));

    let steer19 = evaluate_steer(false, true);
    let idle19 = compute_composer_stack_layout(&ComposerStackInput {
        queue_items: &[], queue_can_steer: true, reply_sender: None, awaiting_tasks: &[],
        awaiting_all_settled: false, stopped_tasks: &[], open_waits_count: 0, notice: None,
    });
    insert_scenario(sc, "pair19_queued_steered", "composer_stack", json!({"queue": {"steered_on_tab": steer19.steered_on_tab}}));
    insert_scenario(sc, "pair19_collapsed_idle", "composer_stack", json!({"visible": idle19.visible, "height_px": idle19.height_px as i32}));

    let mut sp20 = OpenBotRightSidePanelState::default();
    sp20.open_pane(SidePanelPaneKind::Question);
    sp20.split_down(SidePanelPaneKind::Files);
    let slot20 = resolve_right_drawer_slot(wiring, true, false, dark.is_apple_style(), sp20.state.open, sp20.state.maximized, sp20.state.width_px, sp20.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant);
    insert_scenario(sc, "pair20_stack_to_side_panel", "composer_stack", json!({"awaiting_replies": {"visible": st17.awaiting_visible}}));
    insert_scenario(sc, "pair20_stack_to_side_panel", "side_panel", json!({"open": sp20.state.open, "active_right_drawer_count": slot20.active_right_drawer_count, "active_splits": sp20.state.active_split_ids()}));

    let mut sw22 = BotIdentityAutosaveState::new("bot-a", "BotA", "Bio");
    sw22.edit_name("BotA-Updated", 100);
    let _ = sw22.flush_before_agent_switch("group-1", "Squad", "Group");
    let grp_btns: Vec<&str> = LAUNCHER_PRIMARY_PANES.iter().map(|p| p.launcher_label(SidePanelSettingsTarget::Channel, true)).collect();
    let hum_btns: Vec<&str> = LAUNCHER_PRIMARY_PANES.iter().map(|p| p.launcher_label(SidePanelSettingsTarget::Contact, true)).collect();
    insert_scenario(sc, "pair22_switch_to_group", "side_panel", json!({"launcher_buttons": grp_btns}));
    insert_scenario(sc, "pair22_switch_to_group", "agent_settings_profile", json!({"dirty_flushed_to_agent_id": sw22.last_flushed_agent_id}));
    insert_scenario(sc, "pair22_switch_to_human", "side_panel", json!({"launcher_buttons": hum_btns}));

    // Explicit remaining 8 pairwise scenarios (zero root fallback)
    let mut c03 = CreateManagedBotBoxPeers::new();
    c03.open_fabushi_creator();
    let wb03 = compute_working_badge_layout(true, false, 240);
    let d03 = DialogItemModel {
        id: 303, peer_name: "法布施规划助手".into(), avatar_initials: "法".into(), avatar_color: 0x3B93F0,
        last_message: "季度路线图与自动化交付清单整理完毕👩‍👩‍👧‍👦".into(), timestamp: "10:42".into(),
        unread_count: 1, is_muted: false, is_pinned: false, is_online: true, typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read, folder: "All".into(),
    };
    let r03 = compute_openbot_dialog_row_layout(&d03, &SidebarDialogMeta::default(), 280.0, "zh-CN");
    insert_scenario(sc, "pair03_sidebar_plush_badges", "plush", json!({
        "default_bot_avatar": {"is_plush_3d": c03.avatar_shape.starts_with("plush:")},
        "badges": {"working": {"width_px": wb03.width_px as i32}},
    }));
    insert_scenario(sc, "pair03_sidebar_plush_badges", "sidebar_list", json!({
        "rows": [{"height_px": r03.height_px as i32, "avatar_px": r03.avatar_px as i32, "min_w_0": r03.min_w_0, "whitespace_nowrap": r03.whitespace_nowrap}]
    }));

    let rail06 = AppleAppSidebar::new(dark, "zh-CN");
    let mut sb06 = ChatSidebar::new();
    configure_openbot_sidebar_for_shell(&mut sb06, wiring);
    let fa06 = compute_shell_account_footer_audit(&rail06, &sb06);
    let suite_d = evaluate_headless_shell_layout_suite().defects;
    insert_scenario(sc, "pair06_sole_footer_compat", "page_rail", json!({
        "width_px": OPENBOT_PAGE_RAIL_WIDTH as i32,
        "total_account_footers_in_shell": fa06.total_account_footers_in_shell,
    }));
    insert_scenario(sc, "pair06_sole_footer_compat", "sidebar_list", json!({
        "has_duplicate_account_footer": fa06.has_duplicate_account_footer,
    }));
    insert_scenario(sc, "pair06_sole_footer_compat", "defects_audit", json!({
        "d1_wide_apple_sidebar": suite_d.d1_wide_apple_sidebar,
        "d2_duplicate_account_footer": suite_d.d2_duplicate_account_footer,
    }));

    let sr08 = compute_openbot_sidebar_search_row_layout(false, false, 2, false, &dark, "zh-CN");
    let mut gs08 = GlobalSearchState::new(false);
    let _ = gs08.try_open_palette(false);
    let gs08_l = compute_global_search_modal_layout(1440.0, 900.0, gs08.open);
    insert_scenario(sc, "pair08_pill_to_global_search", "sidebar_chrome", json!({
        "search": {"height_px": sr08.height_px as i32}
    }));
    insert_scenario(sc, "pair08_pill_to_global_search", "global_search", json!({
        "open": gs08.open, "width_px": gs08_l.width_px as i32,
        "plush": {"size_px": gs08_l.plush_size_px as i32},
        "tabs": GlobalSearchTab::ALL.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
    }));

    let h13 = compute_openbot_header_layout(&default_header_props(&empty_tabs), &dark);
    let m13_err = MessageItemModel {
        id: 13, sender_id: 2, sender_name: "Bot".into(), avatar_initials: "B".into(), avatar_color: 0x3B93F0,
        direction: MessageDirection::Incoming, timestamp: "10:42".into(), delivery_status: DeliveryStatus::Read,
        reply_to: None, reactions: vec![],
        content: MessageContent::Text { text: r#"{"error":"rate_limit","status":429}"#.into(), is_edited: false },
        bot_keyboard: None, is_pinned: false,
    };
    let b13 = compute_openbot_bubble_layout(
        &m13_err,
        BubbleGroupLayout {
            attached_to_prev: false, attached_to_next: false, show_sender_name: false,
            show_bottom_avatar: true, show_tail: false, reserve_avatar_column: true,
        },
        false,
        false,
        &dark,
        "en",
    );
    let act13 = compute_agent_activity_layout(true, 6, false, false);
    insert_scenario(sc, "pair13_header_above_stream", "conversation_header", json!({
        "height_px": h13.height_px as i32, "overlaps_timeline": h13.overlaps_timeline,
        "tab_shape": {"visible": h13.tab_shape_visible},
    }));
    insert_scenario(sc, "pair13_header_above_stream", "chat_stream", json!({
        "steps": {"error_drawer_open": b13.error_drawer_open},
        "activity": {"bubble_less": act13.bubble_less},
    }));

    let mut c16 = ComposerState::new(true);
    c16.available_agents = vec![("a1".into(), "ResearchBot".into()), ("a2".into(), "WriterBot".into())];
    c16.open_picker(ComposerTrigger::At, "every");
    let at16 = c16.resolve_active_at_picker();
    let chips16: Vec<Value> = extract_rendered_message_reference_chips(
        "Ping @[ResearchBot](agent:a1) and @everyone",
        &dark,
    )
    .iter()
    .map(|c| json!({"kind": c.kind, "id": c.id, "clickable": c.clickable}))
    .collect();
    let merged16 = merge_consecutive_routing_events(&[
        ChannelRoutingEvent { action: "asked".into(), agent_id: "a1".into() },
        ChannelRoutingEvent { action: "asked".into(), agent_id: "a2".into() },
    ]);
    let mut names16 = BTreeMap::new();
    names16.insert("a1".into(), "ResearchBot".into());
    names16.insert("a2".into(), "WriterBot".into());
    let markers16: Vec<Value> = compute_routing_action_markers(&merged16, &names16)
        .iter()
        .map(|m| json!({"kind": "channel-routing", "action": m.action, "label": m.label, "clickable_pills": m.clickable_pills}))
        .collect();
    insert_scenario(sc, "pair16_group_mention_routing", "mention_picker", json!({
        "everyone_shown": at16.everyone_shown, "rendered_chips": chips16,
    }));
    insert_scenario(sc, "pair16_group_mention_routing", "channel_stage", json!({
        "action_markers": markers16,
    }));

    let mut sp21 = OpenBotRightSidePanelState::default();
    sp21.open_pane(SidePanelPaneKind::Settings);
    let slot21 = resolve_right_drawer_slot(
        wiring, true, false, dark.is_apple_style(), sp21.state.open, sp21.state.maximized,
        sp21.state.width_px, sp21.is_settings_visible(), true, true, SidePanelSettingsTarget::Assistant,
    );
    let mut insp21 = ContextInspector::new();
    insp21.sync_inspector_for_unified_side_panel(true);
    insp21.profile.is_bot = true;
    insp21.profile.name = "Research Assistant".into();
    let bot_l21 = compute_openbot_bot_settings_layout(&insp21, &dark);
    insert_scenario(sc, "pair21_launcher_to_bot_profile", "side_panel", json!({
        "active_right_drawer_count": slot21.active_right_drawer_count,
        "legacy_standalone_inspector_mounted": slot21.mount_legacy_standalone_inspector,
    }));
    insert_scenario(sc, "pair21_launcher_to_bot_profile", "agent_settings_profile", json!({
        "mounted": bot_l21.mounted, "host": bot_l21.host,
        "context_inspector_live_reachable": bot_l21.context_inspector_live_reachable,
    }));

    let mut insp23 = ContextInspector::new();
    insp23.sync_inspector_for_unified_side_panel(true);
    insp23.profile.is_bot = true;
    insp23.published_creations = vec![BotPublishedCreationSummary {
        creation_id: "b1".into(), title: "OpenBot Guide".into(),
        kind_label: "book".into(), status_label: "Published".into(), kind_code: "book",
    }];
    let bot_l23 = compute_openbot_bot_settings_layout(&insp23, &dark);
    let mut f23_in = OpenBotShellFrameInput::desktop_default(1440.0, 900.0, dark);
    f23_in.active_panel = ActiveWorkspacePanel::ContentHub(ContentHubSection::Books);
    let f23 = compute_openbot_shell_frame_layout(&f23_in);
    let hub23 = compute_content_hub_shell_layout(&dark, 1440.0, "books");
    insert_scenario(sc, "pair23_works_to_hub", "agent_settings_profile", json!({
        "creations_counts": {"books": bot_l23.creations_counts.books},
    }));
    insert_scenario(sc, "pair23_works_to_hub", "content_hub_pages", json!({
        "active_page": hub23.active_page,
        "mounted_inside_inset_card": f23.content_hub_mounted_inside_inset_card,
        "legacy_blue_pill_count": hub23.legacy_blue_pill_count,
    }));

    let cd24 = ConfirmDialogState::open_dialog(true);
    let cd24_l = compute_confirm_dialog_layout(&cd24, &dark);
    let mut ch24 = ContextInspector::new();
    ch24.sync_inspector_for_unified_side_panel(true);
    ch24.members.is_group_or_channel = true;
    let ch24_l = compute_openbot_channel_settings_layout(&ch24);
    let (_, ledger24_ok, _) = verify_disk_ledger_and_capabilities(&workspace_root());
    insert_scenario(sc, "pair24_confirm_and_ledger", "settings_and_overlays", json!({
        "confirm_dialog": {"max_width_px": cd24_l.max_width_px as i32, "initial_focus": cd24_l.initial_focus}
    }));
    insert_scenario(sc, "pair24_confirm_and_ledger", "agent_settings_profile", json!({
        "context_inspector_live_reachable": bot_l21.context_inspector_live_reachable,
    }));
    insert_scenario(sc, "pair24_confirm_and_ledger", "channel_settings", json!({
        "context_actions_visible": ch24_l.context_actions_visible,
    }));
    insert_scenario(sc, "pair24_confirm_and_ledger", "compat_and_ledger", json!({
        "preserved_consts": {"context_inspector_drawer_width": LEGACY_INSPECTOR_W},
        "ledger_closure_pass": ledger24_ok,
    }));
}
