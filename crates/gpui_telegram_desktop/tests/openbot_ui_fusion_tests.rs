//! Comprehensive M5 OpenBot 1:1 UI/UX, 7 Desktop OS Surfaces, Live `OpenBotHostRuntime` Bridge,
//! Bidirectional Telegram Fusion, All 15 `ui-probe` Scenarios, Headless GPUI Render, and
//! Baseline Telegram Preservation Integration Tests (`OB-F14`..`OB-F20`).

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::{App, AppContext, Application, Entity, WindowBounds, WindowOptions, bounds, point, px, size};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::{
    OpenBotColorScheme, TdesktopThemeMode, TelegramAssetSource, ThemePalette,
};
use gpui_telegram_desktop::views::canonical_composer::{MAX_COMPOSER_HEIGHT, MIN_COMPOSER_HEIGHT};
use gpui_telegram_desktop::views::chat_sidebar::DEFAULT_WIDTH as CHAT_SIDEBAR_DEFAULT_WIDTH;
use gpui_telegram_desktop::views::context_inspector::DRAWER_WIDTH as INSPECTOR_DRAWER_WIDTH;
use gpui_telegram_desktop::views::nav_rail::{NAV_RAIL_WIDTH, NavDestination, NavRail};
use gpui_telegram_desktop::views::openbot::agent_studio::{
    AgentSettingsState, FIRST_AGENT_COLOR_CHOICES_COUNT, FIRST_AGENT_FACE_COUNT,
    FIRST_AGENT_SUGGESTIONS_COUNT, FirstAgentSetupState, GlobalSearchState,
};
use gpui_telegram_desktop::views::openbot::ask_ui::{
    VAULT_MASKED_FIELDS, validate_and_evaluate_ui_block,
};
use gpui_telegram_desktop::views::openbot::channel_editor::{
    ChannelSettingsPaneState, ICON_CATALOG_COUNT, resolve_channel_icon,
};
use gpui_telegram_desktop::views::openbot::channel_view::{
    CHANNEL_ASSIGNMENT_LIMIT, ChannelConversationState, ChannelRoutingEvent,
    ChannelStoppedTasksState, merge_consecutive_routing_events,
};
use gpui_telegram_desktop::views::openbot::composer::{
    ComposerAttachmentDraft, ComposerMcpEntry, ComposerSkillEntry, ComposerState, handle_paste,
    resolve_at_picker, resolve_dollar_picker, validate_attachments,
};
use gpui_telegram_desktop::views::openbot::conversation::{
    ChatTimelineRow, ConversationViewState, SPEECH_SPEEDS, StreamingBodyPacer, VoicePlayerState,
    message_preview_surrogate_safe,
};
use gpui_telegram_desktop::views::openbot::os_surfaces::{
    AgentDockSeat, AgentDockState, BrowserDockState, ChatWindowState, CuaOverlayState,
    DynamicIslandPresentationItem, DynamicIslandState, MainWindowState,
    evaluate_desktop_os_surfaces_probe, is_external_url_blocked, should_trigger_recovery,
};
use gpui_telegram_desktop::views::openbot::queue_and_voice::{
    PILL_LEVELS, VoicePillPhase, VoicePillState, VoiceTalkMode, evaluate_steer,
};
use gpui_telegram_desktop::views::openbot::secondary_pages::{
    CRM_TABLES_COUNT, CalendarViewMode, CrmPageState, MarketplacePageState, RoutineCalendarState,
    TasksPanelState,
};
use gpui_telegram_desktop::views::openbot::settings_and_shortcuts::{
    ShortcutChord, ShortcutRegistry, resolve_app_settings_tab, server_settings_section_ids,
    visible_app_settings_tabs,
};
use gpui_telegram_desktop::views::openbot::shell_state::{
    AGENT_SETTINGS_MIN_PX, LEFT_PANEL_COMPACT, LEFT_PANEL_DEFAULT, LEFT_PANEL_MAX, LEFT_PANEL_MIN,
    OpenBotPage, OpenBotShellState, SIDEBAR_FOLDER_LEVELS, ServerPlatform, SidebarLayoutSnapshot,
    SidebarSection, resolve_folder_depths, validate_sidebar_layout,
};
use gpui_telegram_desktop::views::openbot::side_panel::{
    MAX_SIDE_SPLITS, SIDE_SPLIT_MIN, SidePanelPaneKind, SidePanelState, validate_split_config,
};
use gpui_telegram_desktop::views::openbot::steps_and_trace::{
    AgentStepsState, DOCK_IN_MS, DOCK_OUT_MS, STEPS_MAX_HEIGHT, SubagentDockState,
    TRACE_REPLAY_SPEEDS, TRACE_SPAN_KINDS, TraceWaterfallState,
};
use gpui_telegram_desktop::views::openbot::workflow_canvas::{
    WORKFLOW_ALL_NODE_KIND_STRS, WORKFLOW_EXPORT_FORMAT, WORKFLOW_EXPORT_VERSION,
    WorkflowCanvasState,
};
use gpui_telegram_desktop::views::openbot::{
    ALL_UI_PROBE_SCENARIOS, OpenBotRuntimeBridge, OpenBotWorkspaceShell, evaluate_ui_probe,
    load_live_fusion_snapshot,
};
use gpui_telegram_desktop::views::shell::{
    ActiveWorkspacePanel, AppShell, nav_destination_for_shortcut_digit, next_tdesktop_theme_cycle,
};
use gpui_telegram_desktop::views::stage::history_element::{MSG_MAX_WIDTH, MSG_MIN_WIDTH};
use openbot_contracts::agents::{AGENT_BACKUP_MODEL_LIMIT, AgentModelChoice};
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::channels::{
    Channel, ChannelBrowser, ChannelDraft, ChannelIconName, ChannelMember,
};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};
use openbot_contracts::crm::{CrmPrivateField, CrmProposal, CrmProposedChange};
use openbot_contracts::dynamic_island::DynamicIslandMode;
use openbot_contracts::providers::AgentProviderId;
use openbot_contracts::workflows::{WorkflowEdge, WorkflowPortType, workflow_loop_edges};
use openbot_runtime::{
    CreateAgentRequest, CreateChannelInput, OpenBotHostRuntime, PostChannelMessageInput,
};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn test_ob_f14_shell_rails_sidebar_hysteresis_persistence_and_studio() {
    let tmp = TempDir::new().expect("tempdir");

    // Sidebar width hysteresis: 280 default, [128, 400] clamp, <110 -> 88 compact, >118 expand, Cmd+B -> 0
    let mut shell = OpenBotShellState::new(ServerPlatform::Linux, 1280, false);
    assert_eq!(shell.sidebar.width_px, LEFT_PANEL_DEFAULT);
    shell.sidebar.snap_drag_width(500.0);
    assert_eq!(shell.sidebar.width_px, LEFT_PANEL_MAX);
    assert!(!shell.sidebar.compact);
    shell.sidebar.snap_drag_width(105.0);
    assert!(shell.sidebar.compact);
    assert_eq!(shell.effective_sidebar_px(), LEFT_PANEL_COMPACT);
    shell.sidebar.snap_drag_width(120.0);
    assert!(!shell.sidebar.compact);
    assert_eq!(shell.sidebar.width_px, LEFT_PANEL_MIN);
    shell.sidebar.toggle_closed();
    assert!(shell.sidebar.closed);
    assert_eq!(shell.effective_sidebar_px(), 0.0);
    shell.sidebar.toggle_closed();
    assert!(!shell.sidebar.closed);

    // SidebarLayoutSnapshot nesting depth <= 3 (levels 0..=2) & cycle breaking
    let cyclic_sections = vec![
        SidebarSection {
            id: "s1".into(),
            name: "Root".into(),
            parent_id: Some("s4".into()),
            collapsed: false,
        },
        SidebarSection {
            id: "s2".into(),
            name: "Child".into(),
            parent_id: Some("s1".into()),
            collapsed: false,
        },
        SidebarSection {
            id: "s3".into(),
            name: "Grandchild".into(),
            parent_id: Some("s2".into()),
            collapsed: false,
        },
        SidebarSection {
            id: "s4".into(),
            name: "DeepCycle".into(),
            parent_id: Some("s3".into()),
            collapsed: false,
        },
    ];
    let depths = resolve_folder_depths(&cyclic_sections);
    assert!(depths.values().all(|&d| d < SIDEBAR_FOLDER_LEVELS));

    let mut valid_layout = SidebarLayoutSnapshot::default();
    valid_layout.sections = vec![SidebarSection {
        id: "team".into(),
        name: "Team".into(),
        parent_id: None,
        collapsed: false,
    }];
    valid_layout.order.push("team".into());
    assert!(validate_sidebar_layout(&valid_layout).is_ok());

    // Atomic persistence & zero .tmp residue + corrupt JSON graceful fallback
    let saved = shell.save_to_data_dir(tmp.path()).expect("save shell prefs");
    assert!(!saved.is_empty());
    let has_tmp = std::fs::read_dir(tmp.path())
        .expect("read_dir")
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
    assert!(!has_tmp, "atomic save must leave zero .tmp files");

    std::fs::write(tmp.path().join("openbot-sidebar-v1.json"), b"{corrupt").unwrap();
    std::fs::write(tmp.path().join("openbot-rail-v1.json"), b"\xff\x00bad").unwrap();
    let mut recovered = OpenBotShellState::new(ServerPlatform::Linux, 1280, false);
    recovered.load_from_data_dir(tmp.path());
    assert_eq!(recovered.sidebar.width_px, LEFT_PANEL_DEFAULT);
    assert_eq!(OpenBotPage::ALL.len(), 11);

    // GlobalSearchPlush gaze & reduced-motion guard
    let mut search = GlobalSearchState::new(false);
    search.set_caret_x(160.0, 100);
    search.set_searching(true, 100);
    let m = search.plush_metrics();
    assert_eq!(m.idle_ms, 500);
    assert_eq!(m.look_px, 4);
    assert!(m.animated);
    search.set_reduced_motion(true);
    assert!(!search.plush_metrics().animated);

    // FirstAgentSetup zero starter bots, 12 faces, 17 colors, 6 suggestions, purpose cap, zero temperature
    let ws = OpenBotWorkspaceShell::new();
    assert!(ws.agents.is_empty());
    assert!(ws.channels.is_empty());
    assert_eq!(FIRST_AGENT_FACE_COUNT, 12);
    assert_eq!(FIRST_AGENT_COLOR_CHOICES_COUNT, 17);
    assert_eq!(FIRST_AGENT_SUGGESTIONS_COUNT, 6);
    let mut setup = FirstAgentSetupState::first_run();
    assert!(setup.set_purpose("x".repeat(32_001)).is_err());
    assert!(setup.set_purpose("Help triage PRs").is_ok());

    // AgentSettingsPanel 360px min width & <= 8 backup models
    let mut settings = AgentSettingsState::new("agent-1", "Atlas");
    settings.set_panel_width(240.0);
    assert!(settings.panel_width_px >= AGENT_SETTINGS_MIN_PX);
    for idx in 0..AGENT_BACKUP_MODEL_LIMIT {
        settings
            .add_backup_model(AgentModelChoice {
                provider: AgentProviderId::Codex,
                model: format!("gpt-5-{idx}"),
                provider_account_id: None,
            })
            .expect("within backup cap");
    }
    assert!(
        settings
            .add_backup_model(AgentModelChoice {
                provider: AgentProviderId::Codex,
                model: "overflow".into(),
                provider_account_id: None,
            })
            .is_err()
    );
}

#[test]
fn test_ob_f15_conversation_streaming_preview_boundary_steps_trace_and_ask_ui() {
    // 40ms paced streaming vs instant reduced-motion
    let normal_pacer = StreamingBodyPacer::new(false).timings();
    assert_eq!(normal_pacer.step_ms, 40);
    assert_eq!(normal_pacer.fade_ms, 300);
    assert_eq!(normal_pacer.catch_up_ms, 400);
    let mut reduced_pacer = StreamingBodyPacer::new(true);
    assert_eq!(reduced_pacer.timings().step_ms, 40);
    reduced_pacer.push_target("Instant reduced-motion stream");
    assert_eq!(reduced_pacer.committed_text, "Instant reduced-motion stream");
    assert!(reduced_pacer.trailing_chunk.is_empty());

    // >20,000 UTF-16 units with 4-byte emoji surrogate pairs collapses safely to <=3,000
    let long_emoji = "🚀".repeat(12_000); // 24,000 UTF-16 code units
    let collapsed = message_preview_surrogate_safe(&long_emoji, false, false);
    assert!(collapsed.collapsed);
    assert!(collapsed.preview_utf16_len <= 3_000);
    assert!(collapsed.surrogate_valid);
    assert!(collapsed.show_more_available);
    let streaming = message_preview_surrogate_safe(&long_emoji, true, false);
    assert!(!streaming.collapsed);
    let expanded = message_preview_surrogate_safe(&long_emoji, false, true);
    assert!(!expanded.collapsed);

    // ChatRowBoundary isolates corrupt row while rendering sibling rows
    let mut conv = ConversationViewState::new("a1", "Atlas", "a1");
    for idx in 1..=2 {
        conv.rows.push(ChatTimelineRow {
            message: ConversationMessage::new_simple(
                format!("m-{idx}"),
                ConversationMessageAuthor::Assistant,
                format!("Sibling message {idx}"),
                "2026-11-26T00:00:00Z",
                ConversationMessageStatus::Completed,
            ),
            timestamp_ms: 1_700_000_000_000 + idx * 1_000,
            corrupt_override: None,
            steps: None,
            ui_block_card: None,
            runtime_cards: Vec::new(),
            voice_player: None,
        });
    }
    let boundary = conv.evaluate_row_boundaries(true);
    assert!(boundary.row_boundary_isolated);
    assert!(boundary.sibling_rows_rendered >= 2);

    // AgentSteps 220px max height, strikethrough memory diffs, expandable failed stderr
    assert_eq!(STEPS_MAX_HEIGHT as u32, 220);
    let step_vals = vec![
        json!({"kind": "memory", "action": "updated", "oldText": "v1", "text": "v2"}),
        json!({"kind": "memory", "action": "forgotten", "oldText": "stale"}),
        json!({"kind": "progress", "status": "failed", "stderr": "exit status 1"}),
    ];
    let steps = AgentStepsState::from_probe_values(&step_vals, false, false);
    assert_eq!(steps.strikethrough_count(), 2);
    assert!(steps.has_expandable_failure());

    // TraceWaterfall 4 span kinds & replay speeds; SubagentDock FLIP 340/260ms
    assert_eq!(TRACE_SPAN_KINDS, ["model", "tool", "waiting", "handoff"]);
    assert_eq!(TRACE_REPLAY_SPEEDS, ["1", "2", "5", "10"]);
    let trace = TraceWaterfallState::from_probe_values(&[
        json!({"kind": "model", "label": "plan", "startMs": 0, "durationMs": 420}),
        json!({"kind": "tool", "label": "search", "startMs": 420, "durationMs": 180}),
    ]);
    assert_eq!(trace.spans.len(), 2);
    assert_eq!((DOCK_IN_MS, DOCK_OUT_MS), (340, 260));
    let dock = SubagentDockState::from_probe_values(
        &[
            json!({"id": "s1", "status": "running"}),
            json!({"id": "s2", "status": "finished"}),
        ],
        false,
    );
    assert_eq!((dock.running_count(), dock.finished_count()), (1, 1));

    // All 4 ask_ui cards + hold duration + float rejection + calendar validation + secret redaction
    let confirm_spec = json!({
        "type": "confirm",
        "title": "Deploy production",
        "confirmHold": 1200,
        "actions": [{"id": "go", "label": "Deploy"}]
    });
    let confirm_resp = json!({"actionId": "go"});
    let short_hold = validate_and_evaluate_ui_block(&confirm_spec, Some(&confirm_resp), Some(600))
        .expect("short hold eval");
    assert!(!short_hold.hold_accepted);
    let full_hold = validate_and_evaluate_ui_block(&confirm_spec, Some(&confirm_resp), Some(1200))
        .expect("full hold eval");
    assert!(full_hold.hold_accepted);

    let float_hold_spec = json!({
        "type": "confirm",
        "title": "Bad float hold",
        "confirmHold": 800.5,
        "actions": [{"id": "go", "label": "Deploy"}]
    });
    assert!(validate_and_evaluate_ui_block(&float_hold_spec, None, None).is_err());

    let qr_spec = json!({
        "type": "quick_replies",
        "title": "Next",
        "options": [{"id": "ok", "label": "Yes"}]
    });
    assert!(validate_and_evaluate_ui_block(&qr_spec, Some(&json!({"actionId": "ok"})), None).is_ok());

    let choice_spec = json!({
        "type": "choice",
        "title": "Env",
        "multiple": false,
        "options": [{"id": "staging", "label": "Staging"}, {"id": "prod", "label": "Prod"}]
    });
    assert!(
        validate_and_evaluate_ui_block(
            &choice_spec,
            Some(&json!({"actionId": "submit", "values": {"selected": ["staging", "prod"]}})),
            None
        )
        .is_err()
    );
    assert!(
        validate_and_evaluate_ui_block(
            &choice_spec,
            Some(&json!({"actionId": "submit", "values": {"selected": ["staging"]}})),
            None
        )
        .is_ok()
    );

    let form_spec = json!({
        "type": "form",
        "title": "Credentials",
        "fields": [
            {"id": "apiKey", "kind": "text", "label": "API Key", "secret": true},
            {"id": "dueDate", "kind": "date", "label": "Due Date"}
        ]
    });
    assert!(
        validate_and_evaluate_ui_block(
            &form_spec,
            Some(&json!({"actionId": "submit", "values": {"apiKey": "sk-secret", "dueDate": "2025-02-29"}})),
            None
        )
        .is_err()
    );
    let ok_form = validate_and_evaluate_ui_block(
        &form_spec,
        Some(&json!({"actionId": "submit", "values": {"apiKey": "sk-secret", "dueDate": "2025-02-28"}})),
        None,
    )
    .expect("valid secret form");
    assert!(ok_form.secret_redacted_in_storage);
    assert_eq!(ok_form.vault_masked_fields, VAULT_MASKED_FIELDS);
}

#[test]
fn test_ob_f16_composer_pickers_paste_attachments_pi_guard_queue_voice_and_side_panel() {
    // @ picker: 5 sections; omits messages when 0 < query < 3; @everyone iff in_channel && >1 agents
    let multi_agents = vec![
        ("a1".to_string(), "Alpha".to_string()),
        ("a2".to_string(), "Beta".to_string()),
    ];
    let solo_agent = vec![("a1".to_string(), "Alpha".to_string())];
    let short_q = resolve_at_picker("au", true, &multi_agents, &[], &[], &[], &[]);
    assert!(!short_q.at_sections.iter().any(|s| s == "messages"));
    assert!(!short_q.everyone_shown);
    let long_q = resolve_at_picker("every", true, &multi_agents, &[], &[], &[], &[]);
    assert!(long_q.at_sections.iter().any(|s| s == "messages"));
    assert!(long_q.everyone_shown);
    let solo_q = resolve_at_picker("every", true, &solo_agent, &[], &[], &[], &[]);
    assert!(!solo_q.everyone_shown);
    let dm_q = resolve_at_picker("every", false, &multi_agents, &[], &[], &[], &[]);
    assert!(!dm_q.everyone_shown);

    // $ picker: enabled skills & MCP + separate skill_problems
    let skills = vec![
        ComposerSkillEntry { slug: "pdf".into(), name: "PDF".into(), status: "ok".into() },
        ComposerSkillEntry { slug: "broken".into(), name: "Broken".into(), status: "error".into() },
    ];
    let mcps = vec![
        ComposerMcpEntry { id: "gh".into(), name: "GitHub".into(), enabled: true },
        ComposerMcpEntry { id: "off".into(), name: "Off".into(), enabled: false },
    ];
    let dollar = resolve_dollar_picker("", &skills, &mcps);
    assert_eq!(dollar.dollar_items.len(), 2);
    assert_eq!(dollar.skill_problems.len(), 1);

    // >4,000 char paste-to-file
    let paste = handle_paste(4_500);
    assert!(paste.pasted_as_file);
    assert_eq!(paste.pasted_file_name.as_deref(), Some("pasted-text.txt"));
    assert_eq!(paste.inline_text_length, 0);

    // Attachment 100 MiB single cap
    let too_big = vec![ComposerAttachmentDraft::from_raw("dump.bin", 104_857_601, "file")];
    assert!(validate_attachments(&too_big, false).is_err());
    let ok_img = vec![ComposerAttachmentDraft::from_raw("shot.png", 1_024, "image")];
    assert!(validate_attachments(&ok_img, false).is_ok());

    // Custom pi text-only image guard & 1-click allowImages
    let mut composer = ComposerState::new(false);
    composer.set_custom_pi_text_only(true);
    composer.attachments = ok_img;
    assert!(composer.image_model_warning());
    assert!(composer.image_warning_banner_visible());
    composer.click_allow_images();
    assert!(composer.image_inputs_enabled);
    assert!(!composer.image_warning_banner_visible());

    // QueuePanel steerRefused routine guard
    let routine_steer = evaluate_steer(true, true);
    assert!(!routine_steer.can_steer);
    assert!(routine_steer.steer_refused);
    assert_eq!(routine_steer.tooltip.as_deref(), Some("Routine run"));
    let normal_steer = evaluate_steer(false, true);
    assert!(normal_steer.can_steer);
    assert!(normal_steer.steered_on_tab);

    // VoicePill 12 bars & VoicePlayer speeds [0.75..2.0]
    let pill = VoicePillState::new(VoicePillPhase::Listening, VoiceTalkMode::Open);
    assert_eq!(PILL_LEVELS, 12);
    assert!(pill.is_live());
    assert_eq!(SPEECH_SPEEDS, [0.75, 1.0, 1.25, 1.5, 2.0]);
    let mut player = VoicePlayerState::new("v1", 20.0);
    assert_eq!(player.cycle_speed(), 1.25);

    // Splittable SidePanel MAX_SIDE_SPLITS = 2, SIDE_SPLIT_MIN = 120
    assert_eq!(MAX_SIDE_SPLITS, 2);
    assert_eq!(SIDE_SPLIT_MIN as u32, 120);
    assert!(
        validate_split_config(
            &[SidePanelPaneKind::Browser, SidePanelPaneKind::Files, SidePanelPaneKind::Threads],
            0.5,
            Some(200),
        )
        .is_err()
    );
    assert!(
        validate_split_config(
            &[SidePanelPaneKind::Browser, SidePanelPaneKind::Files],
            0.5,
            Some(100),
        )
        .is_err()
    );
    let panel = SidePanelState::new(vec![SidePanelPaneKind::Browser, SidePanelPaneKind::Files])
        .expect("valid 2-way split");
    assert_eq!(panel.active_split_ids(), vec!["browser", "files"]);
}

#[test]
fn test_ob_f17_channel_conversation_avatars_routing_recovery_and_settings() {
    // 20 canonical icons, invalid hue normalized to 215, unknown icon rejected -> None
    assert_eq!(ICON_CATALOG_COUNT, 20);
    let (icon_name, hue) = resolve_channel_icon("code", Some(999)).expect("valid icon");
    assert_eq!(icon_name, ChannelIconName::Code);
    assert_eq!(hue, AvatarHue::Blue);
    assert_eq!(hue.degrees(), 215);
    assert!(resolve_channel_icon("nonexistent-icon", Some(215)).is_none());

    // ChannelAvatar 5..12 (3 faces + +N) vs >=13 (2 faces + +N)
    let members_8: Vec<String> = (1..=8).map(|i| format!("a{i}")).collect();
    let ch8 = ChannelConversationState::new("c8", "Squad 8", members_8, Some("a1".into()));
    let cl8 = ch8.sidebar_cluster_eval(40.0);
    assert_eq!(cl8.shown_faces, 3);
    assert_eq!(cl8.plus_pill.as_deref(), Some("+5"));

    let members_15: Vec<String> = (1..=15).map(|i| format!("a{i}")).collect();
    let ch15 = ChannelConversationState::new("c15", "Squad 15", members_15, Some("a1".into()));
    let cl15 = ch15.sidebar_cluster_eval(40.0);
    assert_eq!(cl15.shown_faces, 2);
    assert_eq!(cl15.plus_pill.as_deref(), Some("+13"));

    // Consecutive channel-routing rows merge & deduplicate agentId in encounter order
    let merged = merge_consecutive_routing_events(&[
        ChannelRoutingEvent { action: "assign".into(), agent_id: "a1".into() },
        ChannelRoutingEvent { action: "assign".into(), agent_id: "a2".into() },
        ChannelRoutingEvent { action: "assign".into(), agent_id: "a1".into() },
        ChannelRoutingEvent { action: "transfer".into(), agent_id: "a3".into() },
    ]);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].agent_ids, vec!["a1", "a2"]);
    assert_eq!(merged[1].agent_ids, vec!["a3"]);

    // ChannelStoppedTasks Continue / Reassign (excludes ownerAgentId) / Cancel & limit = 8
    assert_eq!(CHANNEL_ASSIGNMENT_LIMIT, 8);
    assert_eq!(
        ChannelStoppedTasksState::action_labels(),
        ["Continue", "Reassign", "Cancel"]
    );
    let candidates = ChannelStoppedTasksState::reassign_candidates(
        &["a1".into(), "a2".into(), "a3".into()],
        "a1",
    );
    assert_eq!(candidates, vec!["a2", "a3"]);

    // ChannelSettingsPane archive read-only & browser policy reset on member removal
    let channel = Channel {
        id: "chan-ops".into(),
        draft: ChannelDraft {
            name: "Ops".into(),
            title: "Operations".into(),
            instructions: "Coordinate incidents".into(),
            members: vec![
                ChannelMember { agent_id: "a1".into() },
                ChannelMember { agent_id: "a2".into() },
            ],
            lead_agent_id: Some("a1".into()),
            isolated: Some(true),
            browser: Some(ChannelBrowser::Member { agent_id: "a2".into() }),
            icon: None,
        },
        archived: false,
        revision: 1,
        created_at: "2026-11-26T00:00:00Z".into(),
        image_url: None,
        folders: Some(vec![]),
    };
    let mut pane = ChannelSettingsPaneState::from_channel(&channel);
    assert!(!pane.read_only());
    assert!(pane.remove_member_and_sync_browser("a2"));
    assert_eq!(pane.browser, ChannelBrowser::None);
    pane.archive().expect("archive channel");
    assert!(pane.read_only());
    assert!(pane.add_memory("Should fail while archived").is_err());
}

#[test]
fn test_ob_f18_workflow_canvas_calendar_tasks_marketplace_crm_settings_and_shortcuts() {
    // WorkflowCanvas: 12 node kinds, loop edge detection, tidy layout, .openbot-workflow v1 roundtrip
    assert_eq!(WORKFLOW_ALL_NODE_KIND_STRS.len(), 12);
    let mut canvas = WorkflowCanvasState::with_sample_pipeline();
    canvas.graph.edges.push(WorkflowEdge {
        id: "e-loop".into(),
        from: "n-appr".into(),
        to: "n-agent".into(),
        port_type: WorkflowPortType::Text,
        branch: None,
    });
    let loop_ids = workflow_loop_edges(&canvas.graph.nodes, &canvas.graph.edges);
    assert!(loop_ids.contains("e-loop"));
    canvas.apply_tidy_layout();
    assert!(canvas.tidy_applied);

    let exported = canvas.export_portable_json();
    assert_eq!(exported["format"], WORKFLOW_EXPORT_FORMAT);
    assert_eq!(exported["version"], WORKFLOW_EXPORT_VERSION);
    let mut imported = WorkflowCanvasState::new("Empty");
    imported.import_portable_json(&exported).expect("import portable workflow");
    assert_eq!(imported.graph.nodes.len(), canvas.graph.nodes.len());

    // RoutineCalendar 44-day DST window (week: 7d/4, month: 42d/3, day: 1d/10)
    let cal_week = RoutineCalendarState::new(CalendarViewMode::Week, "2026-03-29", "Europe/Warsaw");
    assert_eq!(cal_week.build_window_dates().len(), 44);
    assert_eq!(cal_week.build_grid().len(), 7);
    assert_eq!(cal_week.probe_summary()["day_cell_limit"], 4);
    let cal_month = RoutineCalendarState::new(CalendarViewMode::Month, "2026-03-29", "UTC");
    assert_eq!(cal_month.build_grid().len(), 42);
    assert_eq!(cal_month.probe_summary()["day_cell_limit"], 3);
    let cal_day = RoutineCalendarState::new(CalendarViewMode::Day, "2026-03-29", "UTC");
    assert_eq!(cal_day.build_grid().len(), 1);
    assert_eq!(cal_day.probe_summary()["day_cell_limit"], 10);

    // TasksPanel 50-task cap
    let tasks_panel = TasksPanelState::default();
    assert!(tasks_panel.probe_summary(Some(50)).is_ok());
    assert!(tasks_panel.probe_summary(Some(51)).is_err());

    // Marketplace fishing alpha >= 128 & reduced-motion fallback
    let mp = MarketplacePageState::new(false);
    assert_eq!(mp.probe_summary(200, false)["no_match_asset"], "fishing-still.webp");
    assert_eq!(mp.probe_summary(0, true)["no_match_asset"], "fishing-still.webp");
    assert_eq!(mp.probe_summary(0, false)["no_match_asset"], "fishing.webm");

    // CrmPage 13 tables, private field redaction & crm_propose review
    assert_eq!(CRM_TABLES_COUNT, 13);
    let mut crm = CrmPageState::default();
    crm.overview.private_fields.push(CrmPrivateField::ContactEmail);
    assert!(crm.redacted_field_display(CrmPrivateField::ContactEmail, "ceo@acme.io").contains("private"));
    crm.overview.proposals.push(CrmProposal {
        id: "prop-1".into(),
        agent_id: "a1".into(),
        customer_id: Some("cust-1".into()),
        thread_id: None,
        reason: "Add VIP tag".into(),
        change: CrmProposedChange::CustomerTags {
            customer_id: "cust-1".into(),
            tag_ids: vec!["vip".into()],
        },
        created_at: "2026-11-26T00:00:00Z".into(),
    });
    assert_eq!(crm.pending_proposals_count(), 1);
    assert!(crm.approve_proposal("prop-1").is_some());
    assert_eq!(crm.pending_proposals_count(), 0);

    // 14 vs 12 App Settings tabs, hosted-servers -> billing alias, 18 Server Settings sections
    assert_eq!(visible_app_settings_tabs(false).len(), 14);
    assert_eq!(visible_app_settings_tabs(true).len(), 12);
    assert_eq!(resolve_app_settings_tab("hosted-servers").unwrap(), "billing");
    assert!(resolve_app_settings_tab("invalid-tab").is_err());
    assert_eq!(server_settings_section_ids().len(), 18);

    // 11+4 shortcuts, fixed override & duplicate collision rejection
    let default_sc = ShortcutRegistry::probe_summary("darwin", &BTreeMap::new()).unwrap();
    assert_eq!(default_sc["configurable_count"], 11);
    assert_eq!(default_sc["fixed_count"], 4);
    let mut fixed_override = BTreeMap::new();
    fixed_override.insert("openSettings".into(), ShortcutChord::new(false, false, false, true, "KeyX"));
    assert!(ShortcutRegistry::probe_summary("darwin", &fixed_override).is_err());
    let mut collision = BTreeMap::new();
    collision.insert("toggleSidebar".into(), ShortcutChord::new(false, false, false, true, "KeyK"));
    assert!(ShortcutRegistry::probe_summary("darwin", &collision).is_err());
}

#[test]
fn test_ob_f19_all_seven_desktop_os_surfaces() {
    // MainWindow: close guard & <721px auto-zoom
    let mut main = MainWindowState::new(720, 640);
    main.working_agents_count = 2;
    main.attempt_close = true;
    assert!(main.auto_zoomed());
    assert!(main.close_guard_prompted());
    assert!(!MainWindowState::new(721, 640).auto_zoomed());

    // ChatWindow: rails/sidebar hidden & auto-close on target agent deletion
    let mut chat_win = ChatWindowState::new("agent-42");
    let rep_before = chat_win.report();
    assert!(rep_before.rails_hidden && rep_before.sidebar_hidden && !rep_before.auto_closed);
    assert!(chat_win.on_agent_deleted("agent-42"));
    assert!(chat_win.report().auto_closed);

    // DynamicIsland: interactionLocksPresentation critical lock & release on resolvedRequestId
    let mut island = DynamicIslandState::new();
    island.current = DynamicIslandPresentationItem {
        mode: DynamicIslandMode::Approval,
        request_id: Some("req-9".into()),
        title: "Approve command".into(),
        ..Default::default()
    };
    island.interacting = true;
    let locked = island.apply_incoming(DynamicIslandPresentationItem {
        mode: DynamicIslandMode::Message,
        title: "New message".into(),
        ..Default::default()
    });
    assert!(locked.locked);
    assert!(locked.critical_lock);
    let released = island.apply_incoming(DynamicIslandPresentationItem {
        mode: DynamicIslandMode::Idle,
        resolved_request_id: Some("req-9".into()),
        ..Default::default()
    });
    assert!(!released.locked);

    // AgentDock: 900ms tuck, 4px strip, >24 seat cap rejected
    let mut dock = AgentDockState::new();
    for idx in 0..24 {
        dock.add_seat(AgentDockSeat::agent(format!("a{idx}"), format!("Agent {idx}")))
            .expect("within 24 seats");
    }
    assert!(dock.add_seat(AgentDockSeat::agent("a24", "Overflow")).is_err());
    dock.idle_ms = 900;
    let dock_rep = dock.report();
    assert!(dock_rep.tucked);
    assert_eq!(dock_rep.strip_px, 4);

    // BrowserDock: 6000ms auto-close vs needs_answer hold
    let mut bd = BrowserDockState::new();
    bd.elapsed_ms = 6_000;
    assert!(!bd.is_open());
    bd.has_question = true;
    assert!(bd.needs_answer());
    assert!(bd.is_open());

    // CUA: 60_000ms idle hide & 33ms poll
    let mut cua = CuaOverlayState::new();
    cua.active = true;
    cua.idle_ms = 59_999;
    assert!(cua.is_visible());
    cua.idle_ms = 60_000;
    assert!(!cua.is_visible());

    // Recovery: >=4 crashes in 60s + cloe SVG + data:-only URL guard
    assert!(!should_trigger_recovery(&[1_000, 10_000, 20_000]));
    assert!(should_trigger_recovery(&[1_000, 10_000, 20_000, 59_000]));
    assert!(is_external_url_blocked("https://evil.example"));
    assert!(!is_external_url_blocked("data:image/svg+xml;base64,PHN2Zz4="));

    let probe = evaluate_desktop_os_surfaces_probe(&json!({
        "crash_timestamps_ms": [1_000, 10_000, 20_000, 59_000],
        "recovery_request_url": "https://example.com"
    }))
    .expect("desktop-os-surfaces probe");
    assert_eq!(probe["surfaces"].as_array().unwrap().len(), 7);
    assert_eq!(probe["recovery"]["triggered"], true);
    assert_eq!(probe["recovery"]["plush_character"], "cloe");
    assert_eq!(probe["recovery"]["plush_svg_present"], true);
    assert_eq!(probe["recovery"]["external_url_blocked"], true);
}

#[test]
fn test_ob_f20_live_runtime_bridge_all_15_ui_probes_and_baseline_invariants() {
    let tmp = TempDir::new().expect("tempdir");

    // Graceful empty snapshot when openbot.db does not exist yet
    let empty_snap = load_live_fusion_snapshot(tmp.path()).expect("empty snapshot");
    assert_eq!(empty_snap.projected_agent_peers, 0);
    assert_eq!(empty_snap.projected_channel_peers, 0);
    assert!(empty_snap.mention_candidates.is_empty());

    // Live OpenBotHostRuntime + SQLite openbot.db CRUD -> OpenBotRuntimeBridge hydration
    let runtime = OpenBotHostRuntime::open(tmp.path()).expect("open OpenBotHostRuntime");
    let alpha = runtime
        .agents()
        .create_agent_full(CreateAgentRequest {
            name: "FusionAlpha".into(),
            title: "Lead Engineer".into(),
            description: "Leads architecture reviews".into(),
            avatar_seed: Some("alpha-seed".into()),
            ..Default::default()
        })
        .expect("create alpha");
    let beta = runtime
        .agents()
        .create_agent_full(CreateAgentRequest {
            name: "FusionBeta".into(),
            title: "QA Auditor".into(),
            description: "Audits regressions".into(),
            avatar_seed: Some("beta-seed".into()),
            ..Default::default()
        })
        .expect("create beta");

    let channel = runtime
        .channels()
        .create(CreateChannelInput {
            name: "Release Room".into(),
            title: "Multi-bot release room".into(),
            instructions: "Coordinate release checks".into(),
            members: vec![
                ChannelMember { agent_id: alpha.id.clone() },
                ChannelMember { agent_id: beta.id.clone() },
            ],
            lead_agent_id: Some(alpha.id.clone()),
            isolated: Some(false),
            ..Default::default()
        })
        .expect("create channel");

    let _posted = runtime
        .channels()
        .post(PostChannelMessageInput {
            channel_id: channel.id.clone(),
            text: format!("@{} please audit M5", alpha.name),
            actor_name: Some("Tester".into()),
            ..Default::default()
        })
        .expect("post channel message");

    let live_snap = load_live_fusion_snapshot(tmp.path()).expect("live fusion snapshot");
    assert_eq!(live_snap.projected_agent_peers, 2);
    assert_eq!(live_snap.projected_channel_peers, 1);
    assert_eq!(
        live_snap.mention_candidates,
        vec!["FusionBeta".to_string(), "FusionAlpha".to_string()]
    );
    assert_eq!(
        live_snap.channel_members_by_id.get(&channel.id).map(Vec::len),
        Some(2)
    );

    let bridge = OpenBotRuntimeBridge::from_runtime(Arc::clone(&runtime));
    let mut ws = OpenBotWorkspaceShell::with_data_dir(tmp.path());
    let proj = bridge
        .hydrate_workspace_shell(&mut ws)
        .expect("hydrate workspace shell");
    assert_eq!(ws.agents.len(), 2);
    assert_eq!(ws.channels.len(), 1);
    assert!(!ws.channel_conversation.messages.is_empty());
    assert_eq!(proj.projected_agent_peers.len(), 2);
    assert_eq!(proj.projected_channel_peers.len(), 1);
    assert!(proj.avatar_profiles.iter().any(|p| p.avatar_spec == "plush:alpha-seed"));

    // All 15 canonical ui-probe scenarios succeed deterministically
    assert_eq!(ALL_UI_PROBE_SCENARIOS.len(), 15);
    for scenario in ALL_UI_PROBE_SCENARIOS {
        let out = evaluate_ui_probe(scenario, tmp.path(), Some(&live_snap))
            .unwrap_or_else(|err| panic!("ui-probe '{scenario}' failed: {err}"));
        assert!(out.is_object(), "ui-probe '{scenario}' must return JSON object");
    }

    let fusion_out = evaluate_ui_probe(
        r#"{"scenario":"telegram-bidirectional-fusion","shortcut":"Cmd+0","avatar_specs":["classic","blob","bloub:goutte","plush:alpha-seed"]}"#,
        tmp.path(),
        Some(&live_snap),
    )
    .expect("bidirectional probe");
    assert_eq!(fusion_out["active_panel"], "OpenBot");
    assert_eq!(fusion_out["bidirectional"]["projected_agent_peers"], 2);
    assert_eq!(fusion_out["bidirectional"]["projected_channel_peers"], 1);
    assert_eq!(fusion_out["bidirectional"]["avatar_engines"]["plush:alpha-seed"], "bloub_plush");
    assert_ne!(fusion_out["bidirectional"]["resolved_goutte_shape"], "goutte");

    // Archiving channel & deleting agent reflects live in load_live_fusion_snapshot
    runtime.channels().archive(&channel.id, None).expect("archive channel");
    runtime.agents().delete_agent(&beta.id).expect("delete beta");
    let after_snap = load_live_fusion_snapshot(tmp.path()).expect("after archive/delete");
    assert_eq!(after_snap.projected_agent_peers, 1);
    assert_eq!(after_snap.projected_channel_peers, 0);
    assert_eq!(after_snap.mention_candidates, vec!["FusionAlpha".to_string()]);

    // Explicit baseline Telegram preservation assertions (TRAPS #1, #2, #3)
    let rail = NavRail::new();
    assert_eq!(rail.items.len(), 13, "TRAP #1: NavRail::items must remain unchanged by OpenBot");
    assert!(rail.items.iter().all(|it| it.unread_badge == 0));
    assert_eq!(nav_destination_for_shortcut_digit("0"), None, "TRAP #2: digit '0' must stay None");
    for (digit, expected) in [
        ("1", NavDestination::Chats),
        ("2", NavDestination::Folders),
        ("3", NavDestination::Calls),
        ("4", NavDestination::TaskCenter),
        ("5", NavDestination::McpTools),
        ("6", NavDestination::TalkMode),
        ("7", NavDestination::SavedMessages),
        ("8", NavDestination::Settings),
        ("9", NavDestination::Browser),
    ] {
        assert_eq!(nav_destination_for_shortcut_digit(digit), Some(expected));
    }
    assert_eq!(NAV_RAIL_WIDTH, 52.0);
    assert_eq!(CHAT_SIDEBAR_DEFAULT_WIDTH, 288.0);
    assert_eq!(INSPECTOR_DRAWER_WIDTH, 320.0);
    assert_eq!(MIN_COMPOSER_HEIGHT, 44.0);
    assert_eq!(MAX_COMPOSER_HEIGHT, 192.0);
    assert_eq!(MSG_MIN_WIDTH, 160.0);
    assert_eq!(MSG_MAX_WIDTH, 430.0);
    assert_eq!(ThemePalette::DAY_BLUE_WINDOW_BG, 0xFFFFFF);
    assert_eq!(ThemePalette::DAY_BLUE_ACTIVE, 0x40A7E3);
    assert_eq!(next_tdesktop_theme_cycle(TdesktopThemeMode::Night), TdesktopThemeMode::Tinted);
    assert_eq!(next_tdesktop_theme_cycle(TdesktopThemeMode::Classic), TdesktopThemeMode::Night);
}

const HEADLESS_CHILD_ENV: &str = "OPENBOT_UI_FUSION_HEADLESS_CHILD";
const HEADLESS_TARGET_ENV: &str = "OPENBOT_UI_FUSION_HEADLESS_TARGET";

fn dispatch_headless_fusion_target(target: &str) -> ! {
    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match target_owned.as_str() {
                    "test_ob_f20_gpui_headless_appshell_and_openbot_workspace_render_and_fusion" => {
                        body_headless_appshell_and_openbot_fusion(cx);
                    }
                    other => panic!("unknown headless test target: {other}"),
                }));
                match inner {
                    Ok(()) => std::process::exit(0),
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("panic");
                        eprintln!("[headless openbot fusion panic] {target_owned}: {msg}");
                        std::process::exit(101);
                    }
                }
            });
    }));
    if let Err(payload) = outer {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic");
        eprintln!("[headless outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_OPENBOT_FUSION_HEADLESS_ENTRY: extern "C" fn() = macos_openbot_fusion_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_openbot_fusion_headless_entry() {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(HEADLESS_TARGET_ENV) {
            dispatch_headless_fusion_target(&target);
        }
    }
}

fn run_headless_fusion_subprocess(test_fn_name: &str) {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_fusion_target(test_fn_name);
    }
    let cfg_dir = TempDir::new().expect("headless config tempdir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(HEADLESS_CHILD_ENV, "1")
        .env(HEADLESS_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", cfg_dir.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture")
        .output()
        .expect("spawn headless gpui child");
    assert!(
        output.status.success(),
        "headless gpui child failed for {test_fn_name}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_ob_f20_gpui_headless_appshell_and_openbot_workspace_render_and_fusion() {
    run_headless_fusion_subprocess(
        "test_ob_f20_gpui_headless_appshell_and_openbot_workspace_render_and_fusion",
    );
}

fn body_headless_appshell_and_openbot_fusion(cx: &mut App) {
    let data_dir = TempDir::new().expect("openbot runtime tempdir");
    let runtime = OpenBotHostRuntime::open(data_dir.path()).expect("open runtime");
    let alpha = runtime
        .agents()
        .create_agent_full(CreateAgentRequest {
            name: "HeadlessAtlas".into(),
            title: "Systems Bot".into(),
            avatar_seed: Some("atlas-plush".into()),
            ..Default::default()
        })
        .expect("create alpha");
    let _chan = runtime
        .channels()
        .create(CreateChannelInput {
            name: "Core Squad".into(),
            members: vec![ChannelMember { agent_id: alpha.id.clone() }],
            lead_agent_id: Some(alpha.id.clone()),
            ..Default::default()
        })
        .expect("create channel");

    // 1. Mount real OpenBotWorkspaceShell GPUI entity, hydrate from live runtime, and render all 11 pages
    let ws_entity: Entity<OpenBotWorkspaceShell> = cx.new(|_| {
        let mut ws = OpenBotWorkspaceShell::with_data_dir(data_dir.path());
        let bridge = OpenBotRuntimeBridge::from_runtime(Arc::clone(&runtime));
        bridge.hydrate_workspace_shell(&mut ws).expect("hydrate");
        ws
    });
    ws_entity.update(cx, |ws, cx| {
        assert_eq!(ws.agents.len(), 1);
        assert_eq!(ws.channels.len(), 1);
        for page in OpenBotPage::ALL {
            ws.select_page(page);
            let _root_div = ws.render_workspace_root();
        }
        ws.select_page(OpenBotPage::Agents);
        cx.notify();
    });

    // On platforms where GPUI Application::headless() supports offscreen window drawing (macOS),
    // also open a window and execute a full window.draw(cx) frame. Linux HeadlessClient returns
    // an unsupported error on open_window while still running full GPUI Entity lifecycle above.
    if cfg!(target_os = "macos") {
        let ws_win = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    show: false,
                    ..Default::default()
                },
                |_window, cx| {
                    cx.new(|_| {
                        let mut ws = OpenBotWorkspaceShell::with_data_dir(data_dir.path());
                        let bridge = OpenBotRuntimeBridge::from_runtime(Arc::clone(&runtime));
                        bridge.hydrate_workspace_shell(&mut ws).expect("hydrate");
                        ws
                    })
                },
            )
            .expect("open headless OpenBotWorkspaceShell window");
        cx.update_window(ws_win.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .expect("draw headless OpenBotWorkspaceShell frame");
    }

    // 2. Mount full AppShell and verify dual-surface routing, shortcuts, IME, theme sync, and fusion
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.is_authenticated = true;
        s
    });

    shell.update(cx, |s, cx| {
        assert_eq!(s.nav_rail.read(cx).items.len(), 13);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);

        // Cmd+0 activates OpenBotWorkspaceShell without touching nav_destination_for_shortcut_digit("0")
        assert!(s.handle_shortcut_key("0", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::OpenBot);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::OpenBot);
        assert!(s.openbot_workspace.is_some());

        let tree = s.inspect_state(cx);
        assert_eq!(tree.window.active_panel, "OpenBot");
        assert_eq!(tree.nav_rail.active_destination, "OpenBot");

        // Forwarded OpenBot shortcuts & IME delegation
        assert!(s.handle_shortcut_key("k", false, cx));
        assert!(s.openbot_workspace.as_ref().unwrap().read(cx).global_search.open);
        s.insert_committed_text("atlas", cx);
        assert_eq!(s.openbot_workspace.as_ref().unwrap().read(cx).global_search.query, "atlas");
        assert!(s.handle_non_char_key("backspace", false, cx));
        assert_eq!(s.openbot_workspace.as_ref().unwrap().read(cx).global_search.query, "atla");
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.openbot_workspace.as_ref().unwrap().read(cx).global_search.open);

        assert!(s.handle_shortcut_key("b", false, cx));
        assert!(s.openbot_workspace.as_ref().unwrap().read(cx).shell.sidebar.closed);
        assert!(s.handle_shortcut_key("e", true, cx));
        assert!(s.openbot_workspace.as_ref().unwrap().read(cx).side_panel.open);
        assert!(s.handle_shortcut_key(",", false, cx));
        assert_eq!(
            s.openbot_workspace.as_ref().unwrap().read(cx).active_page,
            OpenBotPage::Settings
        );

        // Host dark/light theme sync updates OpenBotColorScheme while keeping TdesktopThemePalette intact
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
        assert_eq!(
            s.openbot_workspace.as_ref().unwrap().read(cx).color_scheme,
            OpenBotColorScheme::Light
        );
        assert_eq!(s.theme_state.theme_mode, TdesktopThemeMode::Day);

        // Bidirectional Telegram projection into ChatSidebar & CanonicalComposer @mentions
        let proj = s
            .sync_openbot_peers_into_telegram_sidebar(data_dir.path(), cx)
            .expect("sync openbot peers");
        assert_eq!(proj.projected_agent_peers.len(), 1);
        assert_eq!(proj.projected_channel_peers.len(), 1);
        let sb = s.sidebar.read(cx);
        let agent_did = proj.projected_agent_peers[0].synthetic_dialog_id;
        assert!(sb.bot_ids.contains(&agent_did));
        assert_eq!(
            sb.bot_grok_avatars.get(&agent_did).map(|v| v.shape.as_str()),
            Some("plush:atlas-plush")
        );
        assert!(
            s.composer
                .read(cx)
                .autocomplete
                .all_mention_candidates
                .iter()
                .any(|m| m.user_id == agent_did)
        );

        // Cmd+Shift+O toggles back to Telegram Chats with 13-item NavRail intact
        assert!(s.handle_shortcut_key("o", true, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Chats);
        assert_eq!(s.nav_rail.read(cx).items.len(), 13);
    });
}

#[test]
fn test_m6_t2_workflow_canvas_load_graph_json_structural_validation() {
    let mut canvas = WorkflowCanvasState::with_sample_pipeline();
    let initial_rev = canvas.revision;
    let initial_nodes = canvas.graph.nodes.len();
    let initial_edges = canvas.graph.edges.len();

    // 1. Annotation edge (trigger -> note) rejected cleanly without mutating canvas
    let annotation_edge = json!({
        "nodes": [
            {"id": "n1", "kind": "trigger", "at": {"x": 0.0, "y": 0.0}, "fields": {}},
            {"id": "note1", "kind": "note", "at": {"x": 200.0, "y": 0.0}, "fields": {}}
        ],
        "edges": [
            {"id": "e1", "from": "n1", "to": "note1", "type": "text"}
        ],
        "strokes": []
    });
    assert!(
        canvas.load_graph_json(&annotation_edge).is_err(),
        "annotation edge must be rejected by load_graph_json"
    );
    assert_eq!(canvas.revision, initial_rev);
    assert_eq!(canvas.graph.nodes.len(), initial_nodes);
    assert_eq!(canvas.graph.edges.len(), initial_edges);

    // 2. Self-edge (a1 -> a1) rejected cleanly
    let self_edge = json!({
        "nodes": [
            {"id": "a1", "kind": "llm", "at": {"x": 0.0, "y": 0.0}, "fields": {"prompt": "loop"}}
        ],
        "edges": [
            {"id": "e_self", "from": "a1", "to": "a1", "type": "text"}
        ],
        "strokes": []
    });
    assert!(
        canvas.load_graph_json(&self_edge).is_err(),
        "self-edge must be rejected by load_graph_json"
    );
    assert_eq!(canvas.revision, initial_rev);

    // 3. Port mismatch (image -> condition) rejected for both image and text edge types
    for port_type in ["image", "text"] {
        let port_mismatch = json!({
            "nodes": [
                {"id": "img1", "kind": "image", "at": {"x": 0.0, "y": 0.0}, "fields": {}},
                {"id": "cond1", "kind": "condition", "at": {"x": 240.0, "y": 0.0}, "fields": {"rule": "ok"}}
            ],
            "edges": [
                {"id": "e_port", "from": "img1", "to": "cond1", "type": port_type}
            ],
            "strokes": []
        });
        assert!(
            canvas.load_graph_json(&port_mismatch).is_err(),
            "image -> condition port mismatch ({port_type}) must be rejected"
        );
    }

    // 4. Condition edge missing yes/no branch rejected cleanly
    let missing_branch = json!({
        "nodes": [
            {"id": "cond1", "kind": "condition", "at": {"x": 0.0, "y": 0.0}, "fields": {"rule": "ok"}},
            {"id": "out1", "kind": "output", "at": {"x": 240.0, "y": 0.0}, "fields": {}}
        ],
        "edges": [
            {"id": "e_cond", "from": "cond1", "to": "out1", "type": "text"}
        ],
        "strokes": []
    });
    assert!(
        canvas.load_graph_json(&missing_branch).is_err(),
        "condition edge without yes/no branch must be rejected"
    );

    // 5. Valid DAG with condition branches + disconnected annotation note accepted
    let valid_dag = json!({
        "nodes": [
            {"id": "t1", "kind": "trigger", "at": {"x": 0.0, "y": 0.0}, "fields": {"trigger": "manual"}},
            {"id": "c1", "kind": "condition", "at": {"x": 200.0, "y": 0.0}, "fields": {"rule": "ready"}},
            {"id": "o1", "kind": "output", "at": {"x": 420.0, "y": -60.0}, "fields": {}},
            {"id": "o2", "kind": "output", "at": {"x": 420.0, "y": 60.0}, "fields": {}},
            {"id": "note1", "kind": "note", "at": {"x": 0.0, "y": -140.0}, "fields": {"text": "Standalone annotation"}}
        ],
        "edges": [
            {"id": "e1", "from": "t1", "to": "c1", "type": "text"},
            {"id": "e2", "from": "c1", "to": "o1", "type": "text", "branch": "yes"},
            {"id": "e3", "from": "c1", "to": "o2", "type": "text", "branch": "no"}
        ],
        "strokes": []
    });
    canvas
        .load_graph_json(&valid_dag)
        .expect("valid DAG with condition branches and standalone note must load");
    assert_eq!(canvas.graph.nodes.len(), 5);
    assert_eq!(canvas.graph.edges.len(), 3);
    assert!(canvas.detect_loop_edges().is_empty());

    // 6. Valid multi-node cycle accepted and loop_edges detected (preserving TEST-T2-OB-F18-02)
    let cyclic_graph = json!({
        "nodes": [
            {"id": "s1", "kind": "llm", "at": {"x": 100.0, "y": 100.0}, "fields": {"prompt": "Step 1"}},
            {"id": "s2", "kind": "llm", "at": {"x": 360.0, "y": 100.0}, "fields": {"prompt": "Step 2"}}
        ],
        "edges": [
            {"id": "e_fwd", "from": "s1", "to": "s2", "type": "text"},
            {"id": "e_back", "from": "s2", "to": "s1", "type": "text"}
        ],
        "strokes": []
    });
    canvas
        .load_graph_json(&cyclic_graph)
        .expect("multi-node cyclic graph must load for loop_edges detection");
    let loops = canvas.detect_loop_edges();
    assert_eq!(loops, vec!["e_back".to_string()]);

    // 7. End-to-end ui-probe parity: rejects all 4 invalid graphs, accepts cyclic graph with loop_edges
    let port_mismatch_img = json!({
        "nodes": [
            {"id": "img1", "kind": "image", "at": {"x": 0.0, "y": 0.0}, "fields": {}},
            {"id": "cond1", "kind": "condition", "at": {"x": 240.0, "y": 0.0}, "fields": {"rule": "ok"}}
        ],
        "edges": [
            {"id": "e_port", "from": "img1", "to": "cond1", "type": "image"}
        ],
        "strokes": []
    });
    let tmp = TempDir::new().expect("tempdir");
    for bad_graph in [&annotation_edge, &self_edge, &port_mismatch_img, &missing_branch] {
        let scenario = json!({
            "scenario": "workflow-canvas-pages",
            "workflow_graph": bad_graph
        })
        .to_string();
        assert!(
            evaluate_ui_probe(&scenario, tmp.path(), None).is_err(),
            "ui-probe workflow-canvas-pages must reject invalid workflow_graph"
        );
    }
    let valid_cycle_scenario = json!({
        "scenario": "workflow-canvas-pages",
        "workflow_graph": cyclic_graph
    })
    .to_string();
    let probe_ok = evaluate_ui_probe(&valid_cycle_scenario, tmp.path(), None)
        .expect("ui-probe workflow-canvas-pages must accept valid multi-node cycle");
    assert_eq!(
        probe_ok["pages_probe"]["workflow_canvas"]["loop_edges"],
        json!(["e_back"])
    );
}

#[test]
fn usage_and_credits_has_zero_fake_data_and_hydrates_from_live_sources() {
    use gpui_telegram_client::net::{HermesProviderUsageDto, HermesUsageSummaryDto};
    use gpui_telegram_desktop::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
    use gpui_telegram_desktop::views::apple_sidebar::AppleAppSidebar;
    use gpui_telegram_desktop::views::openbot::{
        CreditLedgerEntry, PageRailState, UsageAndCreditsState,
    };
    use openbot_storage::{UsageSample, UsageTokens};

    // 1. Fresh defaults MUST contain ZERO fabricated spend, tokens, credits, holds, or ring ratios.
    let fresh = UsageAndCreditsState::default();
    assert!(fresh.series.is_empty(), "default usage series must be empty, not fake Codex/Claude/Grok");
    assert_eq!(fresh.total_tokens(), 0);
    assert_eq!(fresh.total_cost_usd(), 0.0);
    assert_eq!(fresh.wallet_balance_credits, 0);
    assert_eq!(fresh.pending_hold_credits, 0);
    assert!(fresh.ledger.is_empty());
    assert_eq!(UsageAndCreditsState::format_rolling_price_digits(fresh.total_cost_usd()), vec!['$', '0', '.', '0', '0']);
    assert_eq!(UsageAndCreditsState::format_cost_usd(0.0042), "$0.0042");
    assert_eq!(UsageAndCreditsState::format_cost_usd(1.25), "$1.25");

    let rail = PageRailState::new();
    assert_eq!(rail.credits_balance_label, None);

    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3390ec, 14, 12);
    let sidebar = AppleAppSidebar::new(pal, "en");
    assert_eq!(sidebar.usage_ring_metric.primary_ratio, Some(0.0));
    assert_eq!(sidebar.usage_ring_metric.secondary_ratio, None);

    // 2. Hydrating from real local OpenBot SQLite AgentUsageStore populates exact real usage.
    let tmp = TempDir::new().expect("tempdir");
    let runtime = OpenBotHostRuntime::open(tmp.path()).expect("open runtime");
    let today = chrono::Utc::now().date_naive().format("%Y-%m-%d").to_string();
    let now_iso = chrono::Utc::now().to_rfc3339();
    runtime
        .database()
        .usage_store()
        .record(&UsageSample {
            agent_id: "agent-alpha".into(),
            session_id: "sess-1".into(),
            turn_id: "turn-1".into(),
            provider: "claude".into(),
            model: "claude-sonnet-4-6".into(),
            counter_id: "c1".into(),
            tokens: UsageTokens {
                uncached_input: Some(10_000),
                cached_input: Some(5_000),
                cache_creation: Some(0),
                output: Some(2_000),
            },
            baseline: false,
            reported_cost: Some(0.0615),
            occurred_at: Some(now_iso),
            last_request_input_tokens: None,
            smallest_request_input_tokens: None,
            previous_tokens: None,
            cost_counter_id: None,
            zero_cost_unpriced: false,
        })
        .expect("record real openbot usage sample");

    let bridge = OpenBotRuntimeBridge::from_runtime(Arc::clone(&runtime));
    let mut ws = OpenBotWorkspaceShell::with_data_dir(tmp.path());
    bridge.hydrate_workspace_shell(&mut ws).expect("hydrate");
    assert_eq!(ws.usage_credits.series.len(), 1);
    assert_eq!(ws.usage_credits.series[0].provider_id, "claude");
    assert_eq!(ws.usage_credits.series[0].input_tokens, 15_000);
    assert_eq!(ws.usage_credits.series[0].output_tokens, 2_000);
    assert_eq!(ws.usage_credits.series[0].cached_tokens, 5_000);
    assert!((ws.usage_credits.total_cost_usd() - 0.0615).abs() < 1e-6);
    assert!(!ws.usage_credits.series[0].has_explicit_quota);

    // 3. Merging real server HermesUsageSummaryDto + local OpenBot HostAnalytics + real Wallet works cleanly.
    let dates = UsageAndCreditsState::utc_date_window(7);
    assert_eq!(dates.last().map(String::as_str), Some(today.as_str()));
    let mut daily_cost = vec![0.0; 7];
    let mut daily_tokens = vec![0u64; 7];
    daily_cost[6] = 0.20;
    daily_tokens[6] = 25_000;
    let hermes = HermesUsageSummaryDto {
        total_turns: 3,
        prompt_tokens: 20_000,
        completion_tokens: 5_000,
        cached_tokens: 2_000,
        total_tokens: 25_000,
        estimated_cost_usd: 0.20,
        tool_calls: 1,
        range_days: 7,
        dates: dates.clone(),
        by_model: Vec::new(),
        by_provider: vec![HermesProviderUsageDto {
            provider_id: "codex".into(),
            display_name: "OpenAI / Codex".into(),
            input_tokens: 20_000,
            output_tokens: 5_000,
            cached_tokens: 2_000,
            total_tokens: 25_000,
            cost_usd: 0.20,
            turns: 3,
            daily_cost_usd: daily_cost,
            daily_tokens,
            models: vec!["gpt-4o".into()],
        }],
    };
    let ob_analytics = bridge.read_host_usage_analytics(7).expect("read analytics");
    ws.usage_credits
        .apply_live_sources(Some(&hermes), Some(&ob_analytics), 7);
    assert_eq!(ws.usage_credits.series.len(), 2);
    assert_eq!(ws.usage_credits.series[0].provider_id, "codex");
    assert_eq!(ws.usage_credits.series[1].provider_id, "claude");
    assert_eq!(ws.usage_credits.total_tokens(), 42_000);
    assert!((ws.usage_credits.total_cost_usd() - 0.2615).abs() < 1e-6);
    // Earlier zero days MUST stay at exact 0.0 (no fake stub bars)
    assert_eq!(ws.usage_credits.series[0].daily_bars[0], 0.0);
    assert!((ws.usage_credits.series[0].daily_bars[6] - 1.0).abs() < 1e-5);

    ws.sync_wallet_credits(
        350,
        1.5,
        0,
        vec![CreditLedgerEntry {
            id: "tx_1".into(),
            label: "Stars Top-Up".into(),
            delta_credits: 350,
            created_at: "Today".into(),
        }],
    );
    assert_eq!(ws.usage_credits.wallet_balance_credits, 350);
    assert_eq!(ws.usage_credits.wallet_ton_balance, 1.5);
    assert_eq!(ws.usage_credits.ledger.len(), 1);
    assert_eq!(
        ws.shell.page_rail.credits_balance_label.as_deref(),
        Some("350 cr")
    );

    ws.usage_credits.reset();
    assert!(ws.usage_credits.series.is_empty());
    assert_eq!(ws.usage_credits.wallet_balance_credits, 0);
}


