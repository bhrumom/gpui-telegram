use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use gpui_telegram_desktop::views::app_shell::shell_state::ShellComputersState;
use gpui_telegram_desktop::views::computer_workbench::{
    decode_data_url_image, ComputerWorkbench, WorkbenchCommand, WorkbenchTab,
};
use gpui_telegram_desktop::views::context_inspector::ContextInspector;
use gpui_telegram_desktop::views::drawer::inspector_tabs::InspectorSubpage;
use gpui_telegram_desktop::views::settings::sections::{
    CloudFormField, SettingsComputersEvent, SettingsComputersPage, SettingsComputersSections,
};
use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
use gpui_telegram_proto::computer_contracts::{
    BotComputerProfileDto, CloudAuthMethod, ComputerNodeDto, ComputerNodeKind, ComputerRunPolicy,
};

fn sample_cloud_node() -> ComputerNodeDto {
    ComputerNodeDto {
        node_id: "cloud:1:build-box".to_string(),
        owner_user_id: 1,
        kind: ComputerNodeKind::CloudServer,
        display_name: "Build Ubuntu".to_string(),
        ip_address: "203.0.113.45".to_string(),
        host: Some("203.0.113.45".to_string()),
        ssh_port: Some(2222),
        ssh_username: Some("deploy".to_string()),
        auth_method: Some(CloudAuthMethod::SshPassword),
        agent_endpoint_url: None,
        workdir: Some("/srv/app".to_string()),
        has_saved_credential: true,
        os_name: "Ubuntu 24.04".to_string(),
        resolution: "1920x1080".to_string(),
        online: true,
        is_current_device: false,
        device_id: None,
        tier_badge: "云电脑".to_string(),
        run_policy: ComputerRunPolicy::AlwaysAllowAuthorized,
        authorized_controllers: Vec::new(),
        bound_bot_ids: vec![777001],
        last_seen_ms: 1_700_000_000_000,
        latest_frame_data_url: None,
    }
}

#[test]
fn settings_section_computers_metadata_works() {
    let sec = SettingsSectionType::Computers;
    assert_eq!(sec.id_str(), "computers");
    assert_eq!(sec.display_title_localized(true), "电脑");
    assert_eq!(sec.display_title(), "Computers");
}

#[test]
fn settings_computers_cloud_form_validation_and_submission() {
    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
    let mut view = SettingsComputersSections::new_live(pal, "zh-CN");
    assert_eq!(view.page, SettingsComputersPage::Overview);

    view.open_cloud_form(Some(777001), None);
    assert_eq!(view.page, SettingsComputersPage::CloudForm);
    assert_eq!(view.form_focus, CloudFormField::DisplayName);
    assert!(view.form_bind_to_current_bot);

    // Submitting without host produces localized status message and emits no event
    assert!(!view.submit_cloud_form());
    assert!(view.status_message.is_some());
    assert!(view.take_event().is_none());

    // Fill host/name/port/credential and submit
    view.form_name = "Prod Server".to_string();
    view.form_host = "198.51.100.8".to_string();
    view.form_port = "2202".to_string();
    view.form_username = "ubuntu".to_string();
    view.form_auth_method = CloudAuthMethod::AgentHttpToken;
    view.form_credential = "tok_secret_123".to_string();
    view.form_agent_url = "http://198.51.100.8:8099".to_string();
    view.form_workdir = "/opt/work".to_string();

    assert!(view.submit_cloud_form());
    match view.take_event() {
        Some(SettingsComputersEvent::SaveCloudServer(req)) => {
            assert_eq!(req.display_name, "Prod Server");
            assert_eq!(req.host, "198.51.100.8");
            assert_eq!(req.port, 2202);
            assert_eq!(req.username, "ubuntu");
            assert_eq!(req.auth_method, CloudAuthMethod::AgentHttpToken);
            assert_eq!(req.secret.as_deref(), Some("tok_secret_123"));
            assert_eq!(
                req.agent_endpoint_url.as_deref(),
                Some("http://198.51.100.8:8099")
            );
            assert_eq!(req.workdir.as_deref(), Some("/opt/work"));
            assert_eq!(req.bind_bot_id, Some(777001));
        }
        other => panic!("expected SaveCloudServer event, got {other:?}"),
    }
}

#[test]
fn computer_workbench_node_binding_frame_decode_and_terminal_commands() {
    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Day, 0x5288C1, 14, 12);
    let mut wb = ComputerWorkbench::new_live("Fallback".to_string(), pal, "en-US");
    let node = sample_cloud_node();
    wb.set_target_node(&node);

    assert_eq!(wb.node_id.as_deref(), Some("cloud:1:build-box"));
    assert_eq!(wb.device.name, "Build Ubuntu");
    assert_eq!(wb.device.ip_address, "203.0.113.45");
    assert_eq!(wb.remote_cwd, "/srv/app");
    assert_eq!(wb.active_tab, WorkbenchTab::DesktopScreen);

    // 1x1 PNG base64 data URL decodes into gpui::Image without panicking
    let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
    let data_url = format!("data:image/png;base64,{png_b64}");
    assert!(decode_data_url_image(&data_url).is_some());
    assert!(decode_data_url_image("not-a-data-url").is_none());

    wb.update_frame_url(Some(data_url));
    assert!(wb.frame_source.is_some());

    // Terminal command queue + log recording
    assert!(!wb.submit_terminal_draft());
    wb.terminal_draft = "uname -a".to_string();
    assert!(wb.submit_terminal_draft());
    assert!(wb.terminal_draft.is_empty());
    match wb.take_command() {
        Some(WorkbenchCommand::RunShell { command, cwd }) => {
            assert_eq!(command, "uname -a");
            assert_eq!(cwd.as_deref(), Some("/srv/app"));
        }
        other => panic!("expected RunShell command, got {other:?}"),
    }

    wb.push_terminal_output(
        "uname -a".to_string(),
        "Linux build-box 6.8.0".to_string(),
        String::new(),
        0,
        Some("/srv/app/repo".to_string()),
    );
    assert_eq!(wb.remote_cwd, "/srv/app/repo");
    assert_eq!(wb.terminal_history.len(), 1);
    wb.set_remote_files("/srv/app/repo".to_string(), vec!["Cargo.toml".to_string()]);
    wb.set_file_preview(
        "/srv/app/repo/Cargo.toml".to_string(),
        "[package]\nname = \"demo\"".to_string(),
    );
    assert_eq!(wb.remote_files, vec!["Cargo.toml".to_string()]);
    assert!(wb.file_preview.is_some());
}

#[test]
fn context_inspector_and_shell_computers_state_sync_and_reset() {
    let mut insp = ContextInspector::new();
    assert_eq!(insp.subpage, InspectorSubpage::Main);

    let node = sample_cloud_node();
    insp.sync_computers(vec![node.clone()]);
    assert_eq!(insp.computers.len(), 1);

    let prof = BotComputerProfileDto {
        bot_id: 777001,
        bound_node_id: Some(node.node_id.clone()),
        persona_prompt: "Ops expert".to_string(),
        preferred_model: "gpt-5.4".to_string(),
        approval_mode: ComputerRunPolicy::AllowReadOnly,
        allow_cross_device_control: true,
        allow_cloud_self_config: true,
        updated_at_ms: 1_700_000_000_000,
    };
    insp.sync_bot_computer_profile(Some(prof));
    assert_eq!(insp.bot_draft_persona, "Ops expert");
    assert_eq!(insp.bot_draft_model, "gpt-5.4");
    assert_eq!(insp.bot_draft_approval_mode, ComputerRunPolicy::AllowReadOnly);
    assert!(insp.bot_draft_allow_cross_device);
    assert!(insp.bot_draft_allow_cloud_self_config);

    let mut state = ShellComputersState::new();
    let gen0 = state.relay_generation;
    state.computers.push(node);
    state.relay_loop_started = true;
    state.current_host_node_id = Some("local:1:dev".to_string());
    state.reset_on_logout();
    assert!(state.computers.is_empty());
    assert!(!state.relay_loop_started);
    assert!(state.current_host_node_id.is_none());
    assert_eq!(state.relay_generation, gen0 + 1);
}

#[test]
fn remap_message_id_moves_bubble_probes_without_refcell_panic() {
    use gpui::{bounds, point, px, size};
    use gpui_telegram_desktop::views::conversation_stage::ConversationStage;

    let mut stage = ConversationStage::new();
    let local_id = 900_001_u64;
    let server_id = 42_u64;
    let sample_bounds = bounds(point(px(10.0), px(20.0)), size(px(120.0), px(40.0)));

    stage.bubble_widths_probe.borrow_mut().insert(local_id, 184.5);
    stage
        .bubble_bounds_probe
        .borrow_mut()
        .insert(local_id, sample_bounds);
    stage.row_heights_probe.borrow_mut().insert(local_id, 92.0);
    stage
        .row_height_cache
        .measured_heights
        .insert(local_id, 92.0);

    stage.remap_message_id(local_id, server_id);

    let widths = stage.bubble_widths_probe.borrow();
    assert!(!widths.contains_key(&local_id));
    assert_eq!(widths.get(&server_id), Some(&184.5));

    let b_map = stage.bubble_bounds_probe.borrow();
    assert!(!b_map.contains_key(&local_id));
    assert_eq!(b_map.get(&server_id), Some(&sample_bounds));

    let row_heights = stage.row_heights_probe.borrow();
    assert!(!row_heights.contains_key(&local_id));
    assert_eq!(row_heights.get(&server_id), Some(&92.0));

    assert!(!stage
        .row_height_cache
        .measured_heights
        .contains_key(&local_id));
    assert_eq!(
        stage.row_height_cache.measured_heights.get(&server_id),
        Some(&92.0)
    );
}

fn sample_text_message(
    id: u64,
    text: &str,
    bot_keyboard: Option<Vec<Vec<gpui_telegram_desktop::views::conversation_stage::InlineKeyboardButton>>>,
) -> gpui_telegram_desktop::views::conversation_stage::MessageItemModel {
    use gpui_telegram_desktop::views::conversation_stage::{
        DeliveryStatus, MessageContent, MessageDirection, MessageItemModel,
    };
    MessageItemModel {
        id,
        sender_id: 2,
        sender_name: "Bot".to_string(),
        avatar_initials: "BT".to_string(),
        avatar_color: 0x5288C1,
        direction: MessageDirection::Incoming,
        timestamp: "09:36".to_string(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: Vec::new(),
        content: MessageContent::Text {
            text: text.to_string(),
            is_edited: false,
        },
        bot_keyboard,
        is_pinned: false,
    }
}

#[test]
fn row_height_estimator_scales_for_multi_row_bot_keyboard_and_cjk_lines() {
    use gpui_telegram_desktop::views::conversation_stage::{
        InlineButtonType, InlineKeyboardButton, RowHeightCache,
    };

    let short = sample_text_message(1, "hi", None);
    let short_h = RowHeightCache::estimate_row_height(&short, false);

    let kb = vec![
        vec![InlineKeyboardButton {
            text: "打开 AA 记账分账".to_string(),
            button_type: InlineButtonType::WebApp("https://t.me/SplitBillBot/split".to_string()),
        }],
        vec![
            InlineKeyboardButton {
                text: "/help".to_string(),
                button_type: InlineButtonType::Callback("help".to_string()),
            },
            InlineKeyboardButton {
                text: "Talk Mode".to_string(),
                button_type: InlineButtonType::Callback("talk".to_string()),
            },
        ],
    ];
    let cjk_bot = sample_text_message(
        2,
        "你好！我是 AA 记账分账助手 (@SplitBillBot)。\n聚餐拼单、旅行 AA 分账与群组账单自动核算 AI 助手。\n已关联伴生小程序：多笔餐饮账单自动按人数 AA 分摊。",
        Some(kb),
    );
    let bot_h = RowHeightCache::estimate_row_height(&cjk_bot, false);

    assert!(
        bot_h >= short_h + 120.0,
        "Expected CJK multi-line bot message ({bot_h}) to estimate significantly taller than single-line ({short_h})"
    );
    assert!(
        bot_h <= 600.0,
        "Estimated height must remain bounded <= 600.0, got {bot_h}"
    );
}

#[test]
fn sync_measured_row_heights_updates_prefix_sums_and_prevents_bottom_overscroll_flicker() {
    use gpui::{point, px, size};
    use gpui_telegram_desktop::views::conversation_stage::ConversationStage;

    let mut stage = ConversationStage::new();
    stage.simulated_viewport_height = Some(600.0);
    stage.messages = (1..=80)
        .map(|i| {
            sample_text_message(
                i,
                "长文本与多行内联按钮测试消息\n第二行中文描述内容确保估算高度与真实测量存在差异",
                None,
            )
        })
        .collect();
    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);

    // Record exact measured outer heights taller than estimator fallback.
    {
        let mut probe = stage.row_heights_probe.borrow_mut();
        for i in 1..=80_u64 {
            probe.insert(i, 165.0);
        }
    }

    assert!(stage.sync_measured_row_heights());
    stage.row_height_cache.rebuild_prefix_sums(&stage.messages);

    assert!(
        (stage.row_height_cache.total_height - 80.0 * 165.0).abs() < 0.5,
        "Unexpected measured total_height: {}",
        stage.row_height_cache.total_height
    );
    assert_eq!(stage.row_height_cache.prefix_sums[0], 165.0);

    // Simulate rubber-band overscroll past bottom: clamped visible_range must match max_scroll_top.
    let viewport_h = 600.0;
    let overscan_px = 600.0;
    let max_scroll_top = (stage.row_height_cache.total_height + 60.0 - viewport_h).max(0.0);
    let at_bottom =
        stage
            .row_height_cache
            .visible_range(max_scroll_top, viewport_h, overscan_px, stage.messages.len());
    let overscrolled_top = (max_scroll_top + 900.0).min(max_scroll_top);
    let during_overscroll = stage.row_height_cache.visible_range(
        overscrolled_top,
        viewport_h,
        overscan_px,
        stage.messages.len(),
    );
    assert_eq!(at_bottom, during_overscroll);

    // Verify unread_anchor is not wiped purely by transient scroll-to-bottom check setup.
    stage.unread_anchor_msg_id = Some(40);
    stage.set_simulated_scroll_offset(
        point(px(0.0), px(-max_scroll_top)),
        size(px(0.0), px(max_scroll_top)),
    );
    assert!(stage.is_at_bottom());
    assert_eq!(stage.unread_anchor_msg_id, Some(40));
}

#[test]
fn bot_profile_model_syncs_with_settings_and_never_injects_fake_gpt54() {
    use gpui_telegram_client::net::hermes_client::{
        MagpiePresetDto, MagpieProviderDto, MagpieRoutingGroupDto, MagpieStateDto,
        MagpieSubscriptionDto,
    };
    use gpui_telegram_desktop::views::drawer::bot_computer_inspector::format_bot_effective_model_label;
    use gpui_telegram_desktop::views::drawer::InspectorAiModelCatalog;
    use gpui_telegram_desktop::views::settings::sections::settings_ai_providers::SubscriptionAccountDto;

    // 1. Fresh ContextInspector with no BotComputerProfileDto must NOT inject fake "gpt-5.4".
    let mut insp = ContextInspector::new();
    insp.sync_bot_computer_profile(None);
    assert!(
        insp.bot_draft_model.is_empty(),
        "sync_bot_computer_profile(None) must keep bot_draft_model empty instead of injecting fake gpt-5.4"
    );

    // 2. Sync real Settings state (Google Vertex AI active with Gemini models + routing group).
    let magpie_state = MagpieStateDto {
        active_provider: "google_vertex:my-proj".to_string(),
        active_model: "gemini-2.5-pro".to_string(),
        needs_provider_selection: false,
        has_active_subscription: true,
        has_configured_provider: true,
        subscriptions: vec![MagpieSubscriptionDto {
            id: "google_vertex:my-proj".to_string(),
            kind: "google_vertex".to_string(),
            label: "Magpie Google Vertex AI (Magpie)".to_string(),
            active: true,
            default_model: Some("gemini-2.5-pro".to_string()),
            ..Default::default()
        }],
        providers: vec![MagpieProviderDto {
            provider_id: "google_vertex:my-proj".to_string(),
            name: "Google Vertex AI".to_string(),
            default_model: "gemini-2.5-pro".to_string(),
            models: vec!["gemini-2.5-pro".to_string(), "gemini-2.5-flash".to_string()],
            enabled: true,
            is_active: true,
            ..Default::default()
        }],
        routing_groups: vec![MagpieRoutingGroupDto {
            id: "smart-pool".to_string(),
            name: "Smart Pool".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let local_subs = vec![SubscriptionAccountDto {
        id: "google_vertex:my-proj".to_string(),
        kind: "google_vertex".to_string(),
        label: "Google Vertex AI".to_string(),
        models: vec![
            "gemini-2.5-pro".to_string(),
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-flash-lite".to_string(),
        ],
        active: true,
        ..Default::default()
    }];

    let catalog = InspectorAiModelCatalog::from_magpie_state(&magpie_state, &[], &local_subs);
    assert!(catalog.synced);
    assert_eq!(catalog.active_provider_name, "Google Vertex AI");
    assert_eq!(catalog.active_model, "gemini-2.5-pro");
    assert_eq!(
        catalog.active_summary().as_deref(),
        Some("Google Vertex AI · gemini-2.5-pro")
    );
    let selectable = catalog.selectable_models();
    assert!(selectable.contains(&"gemini-2.5-pro".to_string()));
    assert!(selectable.contains(&"gemini-2.5-flash".to_string()));
    assert!(selectable.contains(&"gemini-2.5-flash-lite".to_string()));
    assert!(selectable.contains(&"group/smart-pool".to_string()));
    assert!(
        !selectable.iter().any(|m| m == "gpt-5.4" || m == "claude-sonnet-4.6" || m == "deepseek-reasoner"),
        "selectable models must reflect Settings only, never hardcoded fake presets: {selectable:?}"
    );
    assert!(catalog.is_model_consistent_with_settings(""));
    assert!(catalog.is_model_consistent_with_settings("gemini-2.5-flash"));
    assert!(catalog.is_model_consistent_with_settings("group/smart-pool"));
    assert!(!catalog.is_model_consistent_with_settings("gpt-5.4"));

    insp.sync_ai_models(catalog);

    // 3. When bot has no explicit override, label reflects Settings' live active model.
    assert_eq!(
        format_bot_effective_model_label(&insp, "zh-CN"),
        "跟随设置 · gemini-2.5-pro"
    );
    assert_eq!(
        format_bot_effective_model_label(&insp, "en"),
        "Settings · gemini-2.5-pro"
    );

    // 4. Stale override inconsistent with active Settings provider is explicitly flagged.
    insp.sync_bot_computer_profile(Some(BotComputerProfileDto {
        bot_id: 777001,
        bound_node_id: None,
        persona_prompt: String::new(),
        preferred_model: "gpt-5.4".to_string(),
        approval_mode: ComputerRunPolicy::AskEveryTime,
        allow_cross_device_control: true,
        allow_cloud_self_config: true,
        updated_at_ms: 1_700_000_000_000,
    }));
    assert_eq!(
        format_bot_effective_model_label(&insp, "zh-CN"),
        "gpt-5.4 (与设置不一致)"
    );

    // 5. Consistent per-bot override displays cleanly without warning.
    insp.sync_bot_computer_profile(Some(BotComputerProfileDto {
        bot_id: 777001,
        bound_node_id: None,
        persona_prompt: String::new(),
        preferred_model: "gemini-2.5-flash".to_string(),
        approval_mode: ComputerRunPolicy::AskEveryTime,
        allow_cross_device_control: true,
        allow_cloud_self_config: true,
        updated_at_ms: 1_700_000_000_001,
    }));
    assert_eq!(
        format_bot_effective_model_label(&insp, "zh-CN"),
        "gemini-2.5-flash"
    );

    // 6. Preset-backed API provider pulls suggested models from Settings' active provider preset.
    let deepseek_state = MagpieStateDto {
        active_provider: "deepseek".to_string(),
        active_model: "deepseek-chat".to_string(),
        needs_provider_selection: false,
        has_configured_provider: true,
        providers: vec![MagpieProviderDto {
            provider_id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            default_model: "deepseek-chat".to_string(),
            preset_id: Some("deepseek".to_string()),
            enabled: true,
            is_active: true,
            ..Default::default()
        }],
        presets: vec![MagpiePresetDto {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            default_model: "deepseek-chat".to_string(),
            suggested_models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let ds_catalog = InspectorAiModelCatalog::from_magpie_state(&deepseek_state, &[], &[]);
    assert_eq!(
        ds_catalog.selectable_models(),
        vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()]
    );

    // 7. Switching peers keeps account-wide Settings model catalog; logout clears it.
    insp.sync_ai_models(ds_catalog);
    insp.clear_peer_facts();
    assert_eq!(insp.ai_models.active_model, "deepseek-chat");
    insp.configure_for_cloud_workspace("http://127.0.0.1:8080");
    assert!(!insp.ai_models.synced);
    assert!(insp.ai_models.active_model.is_empty());
}

#[test]
fn test_stage_header_model_picker_and_bot_creator_never_use_fake_models() {
    use gpui_telegram_client::net::hermes_client::{
        MagpieProviderDto, MagpieRoutingGroupDto, MagpieStateDto, MagpieSubscriptionDto,
    };
    use gpui_telegram_desktop::views::boxes::peers::create_managed_bot_box::CreateManagedBotBoxPeers;
    use gpui_telegram_desktop::views::conversation_stage::{ConversationStage, StageAction};
    use gpui_telegram_desktop::views::drawer::InspectorAiModelCatalog;
    use gpui_telegram_desktop::views::settings::sections::settings_ai_providers::SubscriptionAccountDto;
    use gpui_telegram_desktop::views::stage::stage_bars::{
        build_openbot_model_picker_rows, format_thinking_effort_pill,
        OpenBotModelPickerPopoverProps, TopBarUiAction,
    };

    // 1. Unsynced / empty catalog must NEVER synthesize fake claude-sonnet-4-5 / gpt-5 / gemini-2.5-pro rows.
    let empty_props = OpenBotModelPickerPopoverProps {
        active_model: "",
        active_effort: "",
        models: &[],
        active_provider_name: "",
        settings_default_model: "",
        bot_override_model: None,
        catalog_synced: false,
        needs_provider_selection: false,
        can_override_bot_model: false,
        show_thinking_effort: false,
    };
    assert!(
        build_openbot_model_picker_rows(&empty_props, "zh-CN").is_empty(),
        "empty model list must never fabricate fake fallback models"
    );

    // 2. Sync real Settings state (Google Vertex AI with gemini-3.8-flash + live models + routing group).
    let magpie_state = MagpieStateDto {
        active_provider: "google_vertex:bhrum21@gmail.com".to_string(),
        active_model: "gemini-3.8-flash".to_string(),
        needs_provider_selection: false,
        has_active_subscription: true,
        has_configured_provider: true,
        subscriptions: vec![MagpieSubscriptionDto {
            id: "google_vertex:bhrum21@gmail.com".to_string(),
            kind: "google_vertex".to_string(),
            label: "Magpie Google Vertex AI (Magpie)".to_string(),
            active: true,
            default_model: Some("gemini-3.8-flash".to_string()),
            ..Default::default()
        }],
        providers: vec![MagpieProviderDto {
            provider_id: "google_vertex:bhrum21@gmail.com".to_string(),
            name: "Google Vertex AI".to_string(),
            default_model: "gemini-3.8-flash".to_string(),
            models: vec![
                "gemini-3.8-flash".to_string(),
                "gemini-3.7-flash".to_string(),
            ],
            enabled: true,
            is_active: true,
            ..Default::default()
        }],
        routing_groups: vec![MagpieRoutingGroupDto {
            id: "fast-pool".to_string(),
            name: "Fast Pool".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let local_subs = vec![SubscriptionAccountDto {
        id: "google_vertex:bhrum21@gmail.com".to_string(),
        kind: "google_vertex".to_string(),
        label: "Google Vertex AI".to_string(),
        models: vec![
            "gemini-3.8-flash".to_string(),
            "gemini-3.7-flash".to_string(),
            "gemini-2.5-flash".to_string(),
        ],
        active: true,
        ..Default::default()
    }];
    let catalog = InspectorAiModelCatalog::from_magpie_state(&magpie_state, &[], &local_subs);

    // Official/non-owned bot (e.g. 全球法布施): shows real Settings active model directly, never "默认模型" or fake list.
    let mut stage = ConversationStage::new();
    stage.set_chat(101, "全球法布施", "bot", "法", 0xE040FB);
    stage.sync_model_picker_context(&catalog, None, "medium".to_string(), false, true);
    assert_eq!(stage.active_model_label, "gemini-3.8-flash");
    assert_eq!(stage.settings_default_model, "gemini-3.8-flash");
    assert_eq!(stage.active_provider_name, "Google Vertex AI");
    assert!(stage.provider_connected);
    assert!(stage.model_catalog_synced);
    assert!(!stage.available_models.iter().any(|m| m == "claude-sonnet-4-5" || m == "gpt-5"));
    assert!(stage.available_models.contains(&"gemini-3.8-flash".to_string()));
    assert!(stage.available_models.contains(&"gemini-3.7-flash".to_string()));
    assert!(stage.available_models.contains(&"group/fast-pool".to_string()));

    let official_rows = build_openbot_model_picker_rows(
        &OpenBotModelPickerPopoverProps {
            active_model: &stage.active_model_label,
            active_effort: &stage.active_thinking_effort,
            models: &stage.available_models,
            active_provider_name: &stage.active_provider_name,
            settings_default_model: &stage.settings_default_model,
            bot_override_model: stage.bot_override_model.as_deref(),
            catalog_synced: stage.model_catalog_synced,
            needs_provider_selection: stage.model_needs_provider_selection,
            can_override_bot_model: stage.can_override_bot_model,
            show_thinking_effort: true,
        },
        "zh-CN",
    );
    assert!(!official_rows.is_empty());
    assert!(official_rows
        .iter()
        .all(|r| r.model_id != "claude-sonnet-4-5" && r.model_id != "gpt-5"));
    assert!(official_rows
        .iter()
        .any(|r| r.model_id == "gemini-3.8-flash" && r.is_selected));

    // Effort-only selection updates active_thinking_effort without clobbering active_model_label.
    stage.apply_topbar_action(TopBarUiAction::SelectModel {
        model_id: None,
        thinking_level: Some("high".to_string()),
    });
    assert_eq!(stage.active_model_label, "gemini-3.8-flash");
    assert_eq!(stage.active_thinking_effort, "high");
    assert_eq!(
        stage.pending_stage_action,
        Some(StageAction::SelectModel {
            model_id: None,
            thinking_level: Some("high".to_string()),
        })
    );
    assert_eq!(format_thinking_effort_pill("high", "zh-CN").as_deref(), Some("深度"));
    assert_eq!(format_thinking_effort_pill("off", "zh-CN"), None);

    // 3. Owned bot with Follow Settings row, stale mismatch detection, and reset-to-default action.
    stage.sync_model_picker_context(
        &catalog,
        Some("gpt-5".to_string()),
        "low".to_string(),
        true,
        true,
    );
    assert_eq!(stage.active_model_label, "gpt-5");
    let owned_rows = build_openbot_model_picker_rows(
        &OpenBotModelPickerPopoverProps {
            active_model: &stage.active_model_label,
            active_effort: &stage.active_thinking_effort,
            models: &stage.available_models,
            active_provider_name: &stage.active_provider_name,
            settings_default_model: &stage.settings_default_model,
            bot_override_model: stage.bot_override_model.as_deref(),
            catalog_synced: stage.model_catalog_synced,
            needs_provider_selection: stage.model_needs_provider_selection,
            can_override_bot_model: stage.can_override_bot_model,
            show_thinking_effort: true,
        },
        "zh-CN",
    );
    assert!(owned_rows[0].is_follow_settings);
    assert_eq!(owned_rows[0].badge.as_deref(), Some("gemini-3.8-flash"));
    assert!(owned_rows.iter().any(|r| r.model_id == "gpt-5" && r.is_mismatch));

    // Selecting Follow Settings ("") restores active_model_label to settings_default_model.
    stage.apply_topbar_action(TopBarUiAction::SelectModel {
        model_id: Some(String::new()),
        thinking_level: None,
    });
    assert_eq!(stage.bot_override_model, None);
    assert_eq!(stage.active_model_label, "gemini-3.8-flash");

    // 4. OpenAiProvidersSettings action closes popover and queues navigation.
    stage.model_picker_open = true;
    stage.apply_topbar_action(TopBarUiAction::OpenAiProvidersSettings);
    assert!(!stage.model_picker_open);
    assert_eq!(stage.pending_stage_action, Some(StageAction::OpenAiProvidersSettings));

    // 5. Disconnected / unconfigured Settings state sets provider_connected=false and emits zero rows.
    let unconfigured = InspectorAiModelCatalog::from_magpie_state(
        &MagpieStateDto {
            needs_provider_selection: true,
            ..Default::default()
        },
        &[],
        &[],
    );
    stage.sync_model_picker_context(&unconfigured, None, String::new(), false, false);
    assert!(!stage.provider_connected);
    assert!(stage.active_model_label.is_empty());
    assert!(stage.available_models.is_empty());

    // 6. Chat switch preserves user Settings catalog while clearing per-bot override; reset() clears all.
    stage.sync_model_picker_context(
        &catalog,
        Some("gemini-2.5-flash".to_string()),
        "high".to_string(),
        true,
        true,
    );
    stage.set_chat(202, "Other Bot", "bot", "OB", 0x2AABEE);
    assert_eq!(stage.bot_override_model, None);
    assert!(stage.active_thinking_effort.is_empty());
    assert_eq!(stage.active_model_label, "gemini-3.8-flash");
    stage.reset();
    assert!(!stage.model_catalog_synced);
    assert!(stage.available_models.is_empty());
    assert!(stage.active_model_label.is_empty());

    // 7. CreateManagedBotBoxPeers never shows fake "GPT-6-Luna" / "claude-sonnet-4-6" and syncs live Settings models.
    let mut creator = CreateManagedBotBoxPeers::new();
    assert_ne!(creator.studio_model_display_label("zh-CN"), "GPT-6-Luna");
    assert_eq!(creator.studio_model_display_label("zh-CN"), "跟随设置默认");
    assert!(creator
        .studio_model_options("zh-CN")
        .iter()
        .all(|(id, lbl)| id != "gpt-6-luna"
            && id != "claude-sonnet-4-6"
            && !lbl.contains("GPT-6-Luna")));

    creator.sync_ai_models(catalog);
    assert_eq!(creator.studio_model_display_label("zh-CN"), "gemini-3.8-flash");
    let opts = creator.studio_model_options("zh-CN");
    assert_eq!(opts[0].0, "");
    assert_eq!(opts[0].1, "跟随设置默认 (gemini-3.8-flash)");
    assert!(opts.iter().any(|(id, _)| id == "gemini-3.8-flash"));
    assert!(opts.iter().any(|(id, _)| id == "gemini-3.7-flash"));
    assert!(opts.iter().any(|(id, _)| id == "group/fast-pool"));
    assert!(opts
        .iter()
        .all(|(id, _)| id != "gpt-6-luna" && id != "claude-sonnet-4-6" && id != "deepseek-reasoner"));
}

