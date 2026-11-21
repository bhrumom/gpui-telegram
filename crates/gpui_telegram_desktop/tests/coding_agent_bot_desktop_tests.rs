//! Desktop UI, Anti-Hijack Routing Guard, Caching, and Local Host Guard tests for Coding Agent Bots.

use std::sync::Arc;

use gpui::{App, AppContext, Application, Entity};
use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
use gpui_telegram_client::{AuthResponse, ChatSummary, ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::TelegramAssetSource;
use gpui_telegram_desktop::views::app_shell::shell_coding_host::should_accept_coding_host_command;
use gpui_telegram_desktop::views::app_shell::shell_miniapps_bots::fabushi_summary_from_bot_registry;
use gpui_telegram_desktop::views::app_shell::shell_state::ShellCodingHostState;
use gpui_telegram_desktop::views::app_shell::shell_truth::{
    apply_authoritative_chat_list, select_authoritative_dialog,
};
use gpui_telegram_desktop::views::boxes::peers::CreateManagedBotBoxPeers;
use gpui_telegram_desktop::views::drawer::bot_computer_inspector::render_bot_profile_main;
use gpui_telegram_desktop::views::drawer::inspector_tabs::InspectorAction;
use gpui_telegram_desktop::views::AppShell;
use gpui_telegram_proto::miniapp_contracts::{
    BotPublicProfileDto, BotRegistryEntry, FabushiBotSummaryDto,
};
use gpui_telegram_proto::{
    CodingAgentBotConfig, CodingHostCommand, CodingPermissionMode, CodingProviderKind,
    DetectedCodingProvider,
};

const HEADLESS_CHILD_ENV: &str = "GPUI_CODING_AGENT_HEADLESS_CHILD";
const HEADLESS_TARGET_ENV: &str = "GPUI_CODING_AGENT_HEADLESS_TARGET";

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_HEADLESS_CODING_AGENT_ENTRY: extern "C" fn() = macos_mod_init_coding_agent_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_coding_agent_entry() {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(HEADLESS_TARGET_ENV) {
            dispatch_headless_target(&target);
        }
    }
}

fn dispatch_headless_target(target: &str) -> ! {
    let body: fn(&mut App) = match target {
        "headless_coding_agent_anti_hijack_cache_and_inspector" => {
            body_headless_coding_agent_anti_hijack_cache_and_inspector
        }
        other => {
            eprintln!("unknown headless coding_agent test target: {other}");
            std::process::exit(101);
        }
    };
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    body(cx);
                }));
                match inner {
                    Ok(()) => std::process::exit(0),
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                            .unwrap_or("panic");
                        eprintln!("[headless child panic] {msg}");
                        std::process::exit(101);
                    }
                }
            });
    }));
    if outer.is_err() {
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[test]
fn test_coding_agent_modal_validation_and_request_generation() {
    let mut modal = CreateManagedBotBoxPeers::new();
    modal.open_coding_agent_creator();

    assert!(modal.is_coding_agent_mode);
    assert!(!modal.is_fabushi_mode);
    assert!(!modal.is_ai_mode);
    assert!(!modal.coding_use_worktree);
    assert_eq!(modal.coding_permission_mode, CodingPermissionMode::Ask);
    assert!(!modal.coding_allow_group_members);

    modal.set_detected_coding_providers(
        vec![
            DetectedCodingProvider {
                kind: CodingProviderKind::ClaudeCode,
                display_name: "Claude Code".to_string(),
                binary_path: Some("/usr/local/bin/claude".to_string()),
                version: Some("1.0.5".to_string()),
                installed: true,
                recommended_username: "ClaudeCodeBot".to_string(),
                supports_acp: true,
                login_hint: String::new(),
            },
            DetectedCodingProvider {
                kind: CodingProviderKind::Codex,
                display_name: "Codex".to_string(),
                binary_path: Some("/usr/local/bin/codex".to_string()),
                version: Some("0.22.0".to_string()),
                installed: true,
                recommended_username: "CodexBot".to_string(),
                supports_acp: true,
                login_hint: String::new(),
            },
            DetectedCodingProvider {
                kind: CodingProviderKind::OpenCode,
                display_name: "OpenCode".to_string(),
                binary_path: None,
                version: None,
                installed: false,
                recommended_username: "OpenCodeBot".to_string(),
                supports_acp: true,
                login_hint: "Install opencode CLI".to_string(),
            },
        ],
        Some("dev-macbook-pro".to_string()),
    );
    assert_eq!(
        modal.coding_bound_device_id.as_deref(),
        Some("dev-macbook-pro")
    );

    modal.select_coding_provider(CodingProviderKind::Codex);
    assert_eq!(modal.selected_coding_provider, CodingProviderKind::Codex);
    assert_eq!(
        modal.bot_name,
        CodingProviderKind::Codex.display_name().to_string()
    );
    assert_eq!(
        modal.bot_username,
        CodingProviderKind::Codex.recommended_username().to_string()
    );
    assert!(!modal.avatar_shape.is_empty());
    assert!(!modal.avatar_color.is_empty());

    // Missing workspace path fails validation and does not emit submit request.
    assert!(!modal.is_valid());
    modal.submit();
    assert!(modal.take_submit_fabushi().is_none());
    assert!(modal.status_message.is_some());

    // Valid configuration emits CreateFabushiBotRequest with CodingAgentBotConfig.
    modal.coding_workspace_path = "  /Users/dev/project-alpha  ".to_string();
    modal.coding_use_worktree = true;
    modal.coding_permission_mode = CodingPermissionMode::AutoEdit;
    modal.coding_allow_group_members = true;
    modal.coding_extra_args = "--verbose --profile \"ci-fast\"".to_string();
    modal.preferred_model = "o3-codex".to_string();
    assert!(modal.is_valid());

    modal.submit();
    let req = modal
        .take_submit_fabushi()
        .expect("valid coding agent modal should emit CreateFabushiBotRequest");
    assert_eq!(req.display_name, modal.bot_name);
    assert_eq!(req.username.as_deref(), Some(modal.bot_username.as_str()));
    assert_eq!(req.preferred_model.as_deref(), Some("o3-codex"));
    let cfg = req
        .coding_agent
        .expect("CreateFabushiBotRequest.coding_agent must be populated");
    assert_eq!(cfg.provider_kind, CodingProviderKind::Codex);
    assert_eq!(cfg.workspace_path, "/Users/dev/project-alpha");
    assert!(cfg.use_worktree);
    assert_eq!(cfg.permission_mode, CodingPermissionMode::AutoEdit);
    assert_eq!(cfg.executable_override, None);
    assert_eq!(
        cfg.extra_args,
        vec!["--verbose", "--profile", "ci-fast"]
    );
    assert_eq!(cfg.bound_device_id.as_deref(), Some("dev-macbook-pro"));
    assert_eq!(cfg.model_override.as_deref(), Some("o3-codex"));
    assert!(cfg.allow_group_members);

    // AcpCustom requires an explicit executable_override.
    modal.select_coding_provider(CodingProviderKind::AcpCustom);
    modal.coding_executable_override.clear();
    assert!(!modal.is_valid());
    modal.coding_executable_override = "/usr/local/bin/custom-acp".to_string();
    assert!(modal.is_valid());

    // Regular Fabushi mode continues to submit with coding_agent: None.
    modal.open_fabushi_creator();
    modal.bot_name = "Normal Assistant".to_string();
    modal.bot_username = "NormalAssistBot".to_string();
    modal.nl_prompt = "Help me organize daily notes".to_string();
    assert!(modal.is_valid());
    modal.submit();
    let fab_req = modal
        .take_submit_fabushi()
        .expect("regular fabushi mode must still submit");
    assert_eq!(fab_req.coding_agent, None);
}

#[test]
fn test_coding_host_command_routing_guard_and_state_reset() {
    let cmd_any_device = CodingHostCommand::ProbeProviders {
        req_id: "r1".to_string(),
        owner_user_id: 100_001,
        target_device_id: None,
    };
    let cmd_targeted = CodingHostCommand::ProbeProviders {
        req_id: "r2".to_string(),
        owner_user_id: 100_001,
        target_device_id: Some("dev-mac-1".to_string()),
    };

    assert!(!should_accept_coding_host_command(
        false,
        Some(100_001),
        "dev-mac-1",
        &cmd_any_device
    ));
    assert!(!should_accept_coding_host_command(
        true,
        Some(999_999),
        "dev-mac-1",
        &cmd_any_device
    ));
    assert!(!should_accept_coding_host_command(
        true,
        Some(100_001),
        "dev-other",
        &cmd_targeted
    ));
    assert!(should_accept_coding_host_command(
        true,
        Some(100_001),
        "dev-mac-1",
        &cmd_any_device
    ));
    assert!(should_accept_coding_host_command(
        true,
        Some(100_001),
        "dev-mac-1",
        &cmd_targeted
    ));

    let mut host_state = ShellCodingHostState::new();
    host_state.registered_for_auth_gen = Some(7);
    host_state.detected_providers.push(DetectedCodingProvider {
        kind: CodingProviderKind::ClaudeCode,
        display_name: "Claude Code".to_string(),
        installed: true,
        ..Default::default()
    });
    assert!(host_state.reset_on_logout().is_none());
    assert!(host_state.registered_for_auth_gen.is_none());
    assert!(host_state.detected_providers.is_empty());
}

#[test]
fn test_headless_gpui_coding_agent_anti_hijack_and_inspector() {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target("headless_coding_agent_anti_hijack_cache_and_inspector");
    }
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(HEADLESS_CHILD_ENV, "1")
        .env(
            HEADLESS_TARGET_ENV,
            "headless_coding_agent_anti_hijack_cache_and_inspector",
        )
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg("test_headless_gpui_coding_agent_anti_hijack_and_inspector")
        .arg("--nocapture")
        .output()
        .expect("spawn headless child");
    assert!(
        output.status.success(),
        "headless gpui child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn sample_coding_cfg(workspace: &str) -> CodingAgentBotConfig {
    CodingAgentBotConfig {
        provider_kind: CodingProviderKind::ClaudeCode,
        workspace_path: workspace.to_string(),
        use_worktree: false,
        permission_mode: CodingPermissionMode::Ask,
        executable_override: None,
        extra_args: vec!["--verbose".to_string()],
        bound_device_id: Some("dev-mac-1".to_string()),
        bound_node_id: None,
        model_override: None,
        allow_group_members: false,
    }
}

fn body_headless_coding_agent_anti_hijack_cache_and_inspector(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let owner_uid = 100_001u64;
    let auth = AuthResponse {
        token: "tok_coding_test".to_string(),
        session_id: "sess_coding_test".to_string(),
        user_id: owner_uid,
        username: "devuser".to_string(),
        first_name: Some("Dev".to_string()),
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth.clone());

    let fresh_host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("ephemeral fabushi"),
    );

    let coding_chat_fab_id = 710_001u64;
    let coding_bot_fab_id = 601_001i64;
    let normal_fab_chat_id = 710_002u64;
    let normal_fab_bot_id = 601_002i64;
    let coding_chat_prof_id = 710_003u64;
    let coding_bot_prof_id = 601_003i64;
    let coding_chat_my_id = 710_004u64;
    let coding_bot_my_id = 601_004i64;

    let coding_cfg = sample_coding_cfg("/tmp/project-repo");

    // Verify BotRegistryEntry -> FabushiBotSummaryDto preserves coding_agent in cache conversions.
    let reg_coding_entry = BotRegistryEntry {
        bot_user_id: coding_bot_fab_id,
        owner_user_id: owner_uid as i64,
        agent_id: "claude-coder-1".to_string(),
        username: "ClaudeCodeBot".to_string(),
        display_name: "Claude Code".to_string(),
        is_fabushi_bot: true,
        coding_agent: Some(coding_cfg.clone()),
        ..Default::default()
    };
    let summary_from_registry =
        fabushi_summary_from_bot_registry(&reg_coding_entry, Some(coding_chat_fab_id as i64), None, None);
    assert_eq!(summary_from_registry.coding_agent.as_ref(), Some(&coding_cfg));

    let shell: Entity<AppShell> = cx.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.session_sync.begin_session(
            auth.token.clone(),
            auth.session_id.clone(),
            auth.user_id,
            auth.username.clone(),
        );
        s.is_authenticated = true;
        s.attach_local_fabushi_host(Arc::clone(&fresh_host));

        s.session_sync
            .chat_peer_user_ids
            .insert(coding_chat_fab_id, coding_bot_fab_id as u64);
        s.session_sync
            .chat_peer_user_ids
            .insert(normal_fab_chat_id, normal_fab_bot_id as u64);
        s.session_sync
            .chat_peer_user_ids
            .insert(coding_chat_prof_id, coding_bot_prof_id as u64);
        s.session_sync
            .chat_peer_user_ids
            .insert(coding_chat_my_id, coding_bot_my_id as u64);

        s.miniapps.fabushi_bots = vec![
            summary_from_registry.clone(),
            FabushiBotSummaryDto {
                bot_user_id: normal_fab_bot_id,
                owner_user_id: owner_uid as i64,
                agent_id: "normal-fab-1".to_string(),
                username: "NormalFabBot".to_string(),
                display_name: "Normal Fabushi".to_string(),
                companion_chat_id: Some(normal_fab_chat_id as i64),
                coding_agent: None,
                ..Default::default()
            },
        ];

        s.miniapps.bot_profiles_cache.insert(
            "profilecodedbot".to_string(),
            BotPublicProfileDto {
                bot: BotRegistryEntry {
                    bot_user_id: coding_bot_prof_id,
                    owner_user_id: owner_uid as i64,
                    username: "ProfileCodedBot".to_string(),
                    display_name: "Profile Coded Bot".to_string(),
                    is_fabushi_bot: true,
                    coding_agent: Some(coding_cfg.clone()),
                    ..Default::default()
                },
                existing_chat_id: Some(coding_chat_prof_id as i64),
                has_started_chat: true,
                allows_write_to_pm: true,
                ..Default::default()
            },
        );

        s.miniapps.my_bots.push(BotRegistryEntry {
            bot_user_id: coding_bot_my_id,
            owner_user_id: owner_uid as i64,
            username: "MyCodedBot".to_string(),
            display_name: "My Coded Bot".to_string(),
            is_fabushi_bot: true,
            coding_agent: Some(coding_cfg.clone()),
            ..Default::default()
        });

        let chats = vec![
            ChatSummary {
                chat_id: coding_chat_fab_id,
                title: "Claude Code".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0xD97757),
                updated_at: Some(1_700_000_000_000),
            },
            ChatSummary {
                chat_id: normal_fab_chat_id,
                title: "Normal Fabushi".to_string(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x2AABEE),
                updated_at: Some(1_700_000_000_001),
            },
        ];
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &chats);
                    select_authoritative_dialog(sb, st, insp, coding_chat_fab_id);
                });
            });
        });

        s.sync_fabushi_bots_to_views(cx);
        s.sync_composer_and_inspector_miniapp_state(cx);
        s
    });

    shell.update(cx, |s, cx| {
        // CRITICAL ANTI-HIJACK GUARD: coding agent bots must NEVER resolve for local Hermes takeover,
        // regardless of whether they sit in `fabushi_bots`, `bot_profiles_cache`, or `my_bots`.
        assert!(
            s.resolve_fabushi_bot_target(coding_chat_fab_id, cx).is_none(),
            "coding agent bot in fabushi_bots must never resolve as local Fabushi target"
        );
        assert!(
            s.resolve_fabushi_bot_target(coding_chat_prof_id, cx).is_none(),
            "coding agent bot in bot_profiles_cache must never resolve as local Fabushi target"
        );
        assert!(
            s.resolve_fabushi_bot_target(coding_chat_my_id, cx).is_none(),
            "coding agent bot in my_bots must never resolve as local Fabushi target"
        );
        // Normal Fabushi Bot must continue to resolve normally.
        assert!(
            s.resolve_fabushi_bot_target(normal_fab_chat_id, cx).is_some(),
            "non-coding Fabushi Bot should still resolve as local Fabushi target"
        );

        // Composer Hermes slash commands must stay disabled on Coding Agent Bots.
        assert!(
            !s.composer.read(cx).hermes_slash_enabled,
            "composer hermes slash menu must be disabled on Coding Agent Bot"
        );

        // Inspector received CodingAgentBotConfig and renders + saves updates cleanly.
        let palette = s.theme_state.palette;
        s.inspector.update(cx, |insp, insp_cx| {
            let active = insp
                .active_fabushi_bot
                .as_ref()
                .expect("active_fabushi_bot synced to inspector");
            assert_eq!(active.coding_agent.as_ref(), Some(&coding_cfg));
            assert_eq!(insp.coding_draft_bot_id, coding_bot_fab_id);
            assert_eq!(insp.coding_draft_workspace, "/tmp/project-repo");
            assert!(!insp.coding_draft_use_worktree);
            assert_eq!(insp.coding_draft_permission_mode, CodingPermissionMode::Ask);
            assert!(!insp.coding_draft_allow_group_members);

            // Render profile + Coding Agent section without panicking.
            let _rendered = render_bot_profile_main(insp, palette, "zh-CN", insp_cx);

            // Edit draft settings & trigger Save -> PatchFabushiBotRequest with coding_agent.
            insp.coding_draft_workspace = "/tmp/project-repo-v2".to_string();
            insp.coding_draft_use_worktree = true;
            insp.coding_draft_permission_mode = CodingPermissionMode::FullAccess;
            insp.coding_draft_allow_group_members = true;
            insp.coding_draft_model_override = "claude-sonnet-4".to_string();
            assert!(insp.save_coding_agent_config_action());

            match insp.take_action() {
                Some(InspectorAction::PatchFabushiBot { bot_id, req }) => {
                    assert_eq!(bot_id, coding_bot_fab_id as u64);
                    assert_eq!(req.preferred_model.as_deref(), Some("claude-sonnet-4"));
                    let patched = req
                        .coding_agent
                        .expect("PatchFabushiBotRequest.coding_agent must be Some");
                    assert_eq!(patched.provider_kind, CodingProviderKind::ClaudeCode);
                    assert_eq!(patched.workspace_path, "/tmp/project-repo-v2");
                    assert!(patched.use_worktree);
                    assert_eq!(patched.permission_mode, CodingPermissionMode::FullAccess);
                    assert!(patched.allow_group_members);
                    assert_eq!(patched.model_override.as_deref(), Some("claude-sonnet-4"));
                }
                other => panic!("expected InspectorAction::PatchFabushiBot, got {other:?}"),
            }
        });
    });
}
