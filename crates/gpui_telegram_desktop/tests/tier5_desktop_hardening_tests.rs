//! Phase 2 Tier 5 Adversarial Coverage Hardening Tests (`[T5-01]` .. `[T5-49]`).
//!
//! Uses isolated headless GPUI subprocess execution so each GPUI test owns an independent
//! `Application::headless()` instance and `GPUI_TELEGRAM_CONFIG_DIR`.

use fabushi_remote_engine::ipc::LocalIpcClient;
use futures::FutureExt;
use gpui::{App, AppContext, Application, AsyncApp, Entity};
use gpui_telegram_client::net::http_client::ChatSummary;
use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_desktop::components::bots::bot_attach_web_view::BotWebViewPendingAction;
use gpui_telegram_desktop::theme::{TdesktopThemePalette, TelegramAssetSource};
use gpui_telegram_desktop::views::app_shell::shell_truth::apply_authoritative_chat_list;
use gpui_telegram_desktop::views::auth::auth_wizard_state::AuthWizardEvent;
use gpui_telegram_desktop::views::boxes::modal_types::{ModalActionOutcome, ModalDialogKind};
use gpui_telegram_desktop::views::chat_sidebar::{
    AddContactKind, BotGrokAvatarVisual, DialogDeliveryStatus, DialogItemModel,
};
use gpui_telegram_desktop::views::composer::bot_keyboard::{
    flags as kb_flags, BotKeyboard, BotKeyboardButton, BotKeyboardButtonKind,
};
use gpui_telegram_desktop::views::drawer::inspector_tabs::{InspectorAction, InspectorSubpage};
use gpui_telegram_desktop::views::main_menu_drawer::MainMenuCommand;
use gpui_telegram_desktop::views::nav_rail::NavDestination;
use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
use gpui_telegram_desktop::views::sidebar::dialogs_row_view::{
    resolve_dialog_avatar_kind, DialogAvatarKind, SidebarContextAction,
};
use gpui_telegram_desktop::views::stage::cards::{
    BotAssistCardAction, DeliveryStatus, MessageContent, MessageDirection, MessageItemModel,
};
use gpui_telegram_desktop::views::{AppShell, ChatSidebar};
use gpui_telegram_proto::computer_contracts::{BotComputerProfileDto, ComputerRunPolicy};
use gpui_telegram_proto::contracts::chats::ChatFolderDto;
use gpui_telegram_proto::miniapp_contracts::{
    BotCommandSpec, BotPublicProfileDto, BotRegistryEntry, FabushiBotSummaryDto,
    MiniAppActionResponse, MiniAppEntry, MiniAppLaunchMode, MiniAppThemeParams, MiniAppUiSchema,
};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

const CHILD_ENV: &str = "GPUI_TIER5_HARDENING_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_TIER5_HARDENING_TARGET";

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

fn dispatch_headless_target(target: &str) -> ! {
    enum Runner {
        Sync(fn(&mut App)),
        Async(fn(AsyncApp) -> Pin<Box<dyn std::future::Future<Output = ()> + 'static>>),
    }

    let runner = match target {
        "t5_01_user_switch_clears_bot_visual_caches" => Runner::Sync(t5_01_body),
        "t5_03_miniapp_same_turn_close_then_relaunch_survives" => {
            Runner::Async(|app| Box::pin(t5_03_body(app)))
        }
        "t5_04_leave_or_delete_chat_clears_last_chat_and_avoids_zombie_draft" => {
            Runner::Sync(t5_04_body)
        }
        "t5_05_compose_cancel_restores_bot_keyboard_and_escape_clears_isolation_sync" => {
            Runner::Async(|app| Box::pin(t5_05_body(app)))
        }
        "t5_06_prestart_stage_isolation_toast_queue_and_empty_keyboard_height" => {
            Runner::Sync(t5_06_body)
        }
        "t5_07_cold_chat_and_queued_localized_toast_do_not_leak_across_switch" => {
            Runner::Async(|app| Box::pin(t5_07_body(app)))
        }
        "t5_08_escape_contacts_and_modal_container_clear_compose_isolation_sync" => {
            Runner::Async(|app| Box::pin(t5_08_body(app)))
        }
        "t5_09_leave_non_last_chat_clears_reply_edit_attachments_and_keyboard" => {
            Runner::Sync(t5_09_body)
        }
        "t5_10_prestart_bot_flushes_outgoing_draft_and_resets_composer_context" => {
            Runner::Sync(t5_10_body)
        }
        "t5_12_sidebar_delete_chat_no_draft_resurrection_and_cleans_last_chat" => {
            Runner::Sync(t5_12_body)
        }
        "t5_13_chat_scoped_toasts_and_async_gating_prevent_cross_chat_leak" => {
            Runner::Async(|app| Box::pin(t5_13_body(app)))
        }
        "t5_14_sync_compose_isolation_exit_on_non_escape_and_prestart" => {
            Runner::Async(|app| Box::pin(t5_14_body(app)))
        }
        "t5_15_cmd_n_new_group_and_create_group_submitted_preserve_snapshot" => {
            Runner::Async(|app| Box::pin(t5_15_body(app)))
        }
        "t5_16_saved_messages_and_cmd_comma_exit_compose_and_isolation_sync" => {
            Runner::Async(|app| Box::pin(t5_16_body(app)))
        }
        "t5_17_delete_bot_chat_clears_bot_profiles_and_stage_caches_plus_computer_toast_scoping" => {
            Runner::Async(|app| Box::pin(t5_17_body(app)))
        }
        "t5_18_fabushi_turn_error_toast_gated_on_active_chat" => Runner::Sync(t5_18_body),
        "t5_19_open_bot_chat_by_username_exits_compose_and_isolation_sync" => {
            Runner::Async(|app| Box::pin(t5_19_body(app)))
        }
        "t5_20_hermes_callback_approval_and_history_toast_gated_on_switch" => {
            Runner::Async(|app| Box::pin(t5_20_body(app)))
        }
        "t5_21_sidebar_open_menu_same_turn_exits_compose_and_isolation_sync" => {
            Runner::Async(|app| Box::pin(t5_21_body(app)))
        }
        "t5_22_local_fabushi_error_card_strips_internal_prefix_and_classifies_provider_errors" => {
            Runner::Sync(t5_22_body)
        }
        "t5_23_toggle_bot_block_cold_chat_and_profile_box_toast_scoping" => {
            Runner::Async(|app| Box::pin(t5_23_body(app)))
        }
        "t5_24_delete_chat_during_plus_compose_preserves_isolation_and_next_draft" => {
            Runner::Async(|app| Box::pin(t5_24_body(app)))
        }
        "t5_25_remote_computers_hub_and_escape_popover_exit_compose_isolation_sync" => {
            Runner::Async(|app| Box::pin(t5_25_body(app)))
        }
        "t5_26_persist_bot_computer_none_chat_and_open_sender_dm_err_toast_scoping" => {
            Runner::Async(|app| Box::pin(t5_26_body(app)))
        }
        "t5_27_provider_missing_key_request_rejection_and_r8_6_non_chat_and_escape_cleanup" => {
            Runner::Async(|app| Box::pin(t5_27_body(app)))
        }
        "t5_28_disconnect_all_remote_control_auth_fence_and_non_chat_toast" => {
            Runner::Async(|app| Box::pin(t5_28_body(app)))
        }
        "t5_30_gap_r9_1_delayed_refresh_screen_cross_account_and_overlay_switch" => {
            Runner::Async(|app| Box::pin(t5_30_body(app)))
        }
        "t5_31_toggle_allow_remote_and_set_assist_allow_rollback_and_toast" => {
            Runner::Async(|app| Box::pin(t5_31_body(app)))
        }
        "t5_32_gap_r9_2_upload_and_send_paths_without_enter_guard_and_session_fence" => {
            Runner::Async(|app| Box::pin(t5_32_body(app)))
        }
        "t5_33_cancelled_plus_compose_create_chat_guards" => {
            Runner::Async(|app| Box::pin(t5_33_body(app)))
        }
        "t5_34_bot_profile_popup_stage_dl_block_fences_and_create_bot_toast" => {
            Runner::Async(|app| Box::pin(t5_34_body(app)))
        }
        "t5_35_logout_user_switch_magpie_cleanup_and_vault_unlock_err_fence" => {
            Runner::Async(|app| Box::pin(t5_35_body(app)))
        }
        "t5_36_compose_reopen_after_exit_and_commit_dm_epoch_guards" => {
            Runner::Async(|app| Box::pin(t5_36_body(app)))
        }
        "t5_37_upload_and_send_paths_mid_upload_logout_and_user_switch_no_leak" => {
            Runner::Async(|app| Box::pin(t5_37_body(app)))
        }
        "t5_38_set_ttl_secs_no_optimistic_toast_on_500_and_no_settings_or_compose_leak" => {
            Runner::Async(|app| Box::pin(t5_38_body(app)))
        }
        "t5_39_shell_modals_proxy_and_sibling_async_fences_and_toast_scoping" => {
            Runner::Async(|app| Box::pin(t5_39_body(app)))
        }
        "t5_40_shell_observers_settings_toast_scoping_ttl_loop_and_folders_fence" => {
            Runner::Async(|app| Box::pin(t5_40_body(app)))
        }
        "t5_41_hermes_studio_sync_fences_all_tasks_workspace_closures_on_logout_and_switch" => {
            Runner::Async(|app| Box::pin(t5_41_body(app)))
        }
        "t5_42_create_bot_box_close_race_and_task_wakeup_scoping_and_auth_guards" => {
            Runner::Async(|app| Box::pin(t5_42_body(app)))
        }
        "t5_43_vault_unlock_all_branches_bootstrap_keepalive_and_magpie_local_reset" => {
            Runner::Async(|app| Box::pin(t5_43_body(app)))
        }
        "t5_44_gap_r11_1_in_memory_outbox_transfer_e2ee_cleared_on_logout_and_switch" => {
            Runner::Async(|app| Box::pin(t5_44_body(app)))
        }
        "t5_45_gap_r11_2_3_4_refresh_chats_fence_toast_guard_and_delete_chat_fence" => {
            Runner::Async(|app| Box::pin(t5_45_body(app)))
        }
        "t5_46_coding_host_per_user_isolation_and_logout_fences" => {
            Runner::Async(|app| Box::pin(t5_46_body(app)))
        }
        "t5_47_gap_r12b_1_3_fabushi_archive_and_dynamic_token_cx_spawn_fences" => {
            Runner::Async(|app| Box::pin(t5_47_body(app)))
        }
        "t5_49_desktop_local_ipc_handler_updates_on_server_url_applied" => {
            Runner::Async(|app| Box::pin(t5_49_body(app)))
        }
        "t5_50_r13b_remaining_cx_spawn_modal_proxy_computer_and_toast_fences" => {
            Runner::Async(|app| Box::pin(t5_50_body(app)))
        }
        "t5_51_auth_wizard_single_client_sync_and_cross_server_session_races" => {
            Runner::Async(|app| Box::pin(t5_51_body(app)))
        }
        "t5_52_magpie_sync_per_step_epoch_fences_and_mid_probe_proxy_divergence" => {
            Runner::Async(|app| Box::pin(t5_52_body(app)))
        }
        "t5_53_gap_r15_1_proxy_validation_and_uncached_select_no_ui_divergence" => {
            Runner::Async(|app| Box::pin(t5_53_body(app)))
        }
        "t5_54_non_composer_sends_preserve_draft_and_toast_reconciliation" => {
            Runner::Async(|app| Box::pin(t5_54_body(app)))
        }
        "t5_55_panel_lock_toast_reconciliation_and_silent_schedule_send_errors" => {
            Runner::Async(|app| Box::pin(t5_55_body(app)))
        }
        "t5_58_blocking_overlays_and_modals_toast_reconciliation" => {
            Runner::Async(|app| Box::pin(t5_58_body(app)))
        }
        "t5_58b_send_silent_schedule_midflight_switch_user_and_logout_draft_isolation" => {
            Runner::Async(|app| Box::pin(t5_58b_body(app)))
        }
        "t5_59_open_telegram_link_and_companion_chat_miniapp_minimize_sync" => {
            Runner::Async(|app| Box::pin(t5_59_body(app)))
        }
        other => {
            eprintln!("unknown tier5 headless gpui test target: {other}");
            std::process::exit(101);
        }
    };

    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| match runner {
                Runner::Sync(body) => {
                    let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        body(cx);
                    }));
                    match inner {
                        Ok(()) => std::process::exit(0),
                        Err(payload) => {
                            let msg = panic_payload_to_string(payload);
                            eprintln!("[tier5 child panic] {target_owned}: {msg}");
                            std::process::exit(101);
                        }
                    }
                }
                Runner::Async(async_fn) => {
                    let t = target_owned.clone();
                    cx.spawn(async move |app: &mut AsyncApp| {
                        let app_clone = app.clone();
                        let res = std::panic::AssertUnwindSafe(async_fn(app_clone))
                            .catch_unwind()
                            .await;
                        match res {
                            Ok(()) => std::process::exit(0),
                            Err(payload) => {
                                let msg = panic_payload_to_string(payload);
                                eprintln!("[tier5 async child panic] {t}: {msg}");
                                std::process::exit(101);
                            }
                        }
                    })
                    .detach();
                }
            });
    }));
    if let Err(payload) = outer {
        let msg = panic_payload_to_string(payload);
        eprintln!("[tier5 outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_TIER5_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = macos_mod_init_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_headless_entry() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
            dispatch_headless_target(&target);
        }
    }
}

fn run_in_headless_subprocess(test_fn_name: &str) {
    run_in_headless_subprocess_impl(test_fn_name, true);
}

fn run_in_headless_subprocess_with_ipc(test_fn_name: &str) {
    run_in_headless_subprocess_impl(test_fn_name, false);
}

fn run_in_headless_subprocess_impl(test_fn_name: &str, offline_test: bool) {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(test_fn_name);
    }

    let tmp_dir = tempfile::Builder::new()
        .prefix("gpui_tg_tier5_")
        .tempdir()
        .expect("create isolated tempdir");
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp_dir.path())
        .env(
            "GPUI_TELEGRAM_STATE_PATH",
            tmp_dir.path().join("session_v1.json"),
        )
        .env("HOME", tmp_dir.path())
        .env("USERPROFILE", tmp_dir.path())
        .env("XDG_CONFIG_HOME", tmp_dir.path().join(".config"))
        .env("CODEX_HOME", tmp_dir.path().join(".codex"))
        .env("FABUSHI_IPC_SOCKET", tmp_dir.path().join("fabushi-ipc.sock"))
        .env_remove("GOOGLE_APPLICATION_CREDENTIALS")
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture");
    if offline_test {
        cmd.env("GPUI_OFFLINE_TEST", "1");
    } else {
        cmd.env_remove("GPUI_OFFLINE_TEST");
    }
    let output = cmd.output().expect("spawn headless gpui child");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "tier5 child test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }
}

async fn pump_until(app: &AsyncApp, timeout_ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let steps = (timeout_ms / 20).max(1);
    for _ in 0..steps {
        if cond() {
            return true;
        }
        app.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    cond()
}

async fn pump_until_settled(
    app: &AsyncApp,
    max_checks: usize,
    mut count_fn: impl FnMut() -> usize,
) {
    let mut prev = count_fn();
    let mut stable_ticks = 0usize;
    for _ in 0..max_checks {
        app.background_executor()
            .timer(Duration::from_millis(20))
            .await;
        let cur = count_fn();
        if cur == prev {
            stable_ticks += 1;
            if stable_ticks >= 3 {
                return;
            }
        } else {
            prev = cur;
            stable_ticks = 0;
        }
    }
}

fn new_isolated_client(server_url: String) -> Arc<TelegramClient> {
    let mut cfg = ClientConfig::default();
    cfg.server_url = server_url;
    let client = Arc::new(TelegramClient::new(cfg));
    client.set_token(Some("tok_tier5_hardening".to_string()));
    client
}

const CID_FABUSHI: u64 = 101;
const UID_VIEWER: u64 = 100_001;
const UID_FABUSHI: u64 = 700_101;

fn setup_authenticated_shell(client: Arc<TelegramClient>, cx: &mut App) -> Entity<AppShell> {
    let bot = BotRegistryEntry {
        bot_user_id: UID_FABUSHI as i64,
        owner_user_id: UID_VIEWER as i64,
        username: "fabushi_bot".to_string(),
        display_name: "全球法布施 · Fabushi".to_string(),
        description: "全球法布施与净心禅室 AI 助手".to_string(),
        about: "全球法布施与净心禅室".to_string(),
        ai_persona: "Fabushi Dharma Guide".to_string(),
        menu_button_text: "全球法布施".to_string(),
        menu_button_url: "/miniapps/fabushi_bot/fabushi".to_string(),
        is_official: true,
        is_inline: true,
        allow_attach_menu: true,
        attach_peer_types: vec!["users".into(), "bots".into()],
        commands: vec![BotCommandSpec {
            command: "start".to_string(),
            description: "开启对话".to_string(),
        }],
        created_at: 1_700_000_000_000,
        is_fabushi_bot: true,
        menu_button_type: "web_app".to_string(),
        has_main_app: true,
        main_app_short_name: "fabushi".to_string(),
        ..Default::default()
    };
    let app = MiniAppEntry {
        app_id: "fabushi_main_app".to_string(),
        bot_user_id: UID_FABUSHI as i64,
        bot_username: "fabushi_bot".to_string(),
        bot_display_name: "全球法布施".to_string(),
        short_name: "fabushi".to_string(),
        title: "全球法布施".to_string(),
        description: "全球法布施小程序".to_string(),
        category: "lifestyle".to_string(),
        icon_emoji: "🪷".to_string(),
        web_url: "/miniapps/fabushi_bot/fabushi".to_string(),
        ui_schema: MiniAppUiSchema {
            title: "全球法布施".to_string(),
            subtitle: "净心禅室".to_string(),
            theme: MiniAppThemeParams::default(),
            widgets: Vec::new(),
            main_button: None,
            secondary_button: None,
            back_button_visible: false,
            settings_button_visible: false,
            copilot_suggestions: Vec::new(),
        },
        peer_types: vec!["users".into(), "bots".into()],
        is_installed: true,
        is_main_app: true,
        direct_chat_id: Some(CID_FABUSHI as i64),
        ..Default::default()
    };
    let prof = BotPublicProfileDto {
        bot: bot.clone(),
        apps: vec![app.clone()],
        main_app: Some(app.clone()),
        has_started_chat: true,
        existing_chat_id: Some(CID_FABUSHI as i64),
        ..Default::default()
    };

    cx.new(|cx| {
        let mut s = AppShell::new_with_client(client, cx);
        s.is_authenticated = true;
        s.session_sync.user_id = Some(UID_VIEWER);
        s.session_sync.session_token = Some("tok_tier5_hardening".to_string());
        s.miniapps
            .bot_profiles_cache
            .insert("fabushi_bot".to_string(), prof);
        s.miniapps.attach_menu_bots = vec![bot];
        s.miniapps.installed_miniapps = vec![app];
        s.miniapps.fabushi_bots = vec![FabushiBotSummaryDto {
            bot_user_id: UID_FABUSHI as i64,
            owner_user_id: UID_VIEWER as i64,
            agent_id: "fabushi-main".to_string(),
            username: "fabushi_bot".to_string(),
            display_name: "全球法布施 · Fabushi".to_string(),
            description: "全球法布施与净心禅室".to_string(),
            companion_chat_id: Some(CID_FABUSHI as i64),
            ..Default::default()
        }];
        s.session_sync
            .chat_peer_user_ids
            .insert(CID_FABUSHI, UID_FABUSHI);

        let chats = vec![
            ChatSummary {
                chat_id: CID_FABUSHI,
                title: "全球法布施 · Fabushi".to_string(),
                chat_type: 4,
                last_message: Some("Bot ready".to_string()),
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x8F59F9),
                updated_at: Some(1_700_000_005_000),
            },
            ChatSummary {
                chat_id: 102,
                title: "Engineering Group".to_string(),
                chat_type: 1,
                last_message: Some("hi".to_string()),
                unread_count: 0,
                is_group: Some(true),
                is_bot: Some(false),
                avatar_color: Some(0x3A7AFE),
                updated_at: Some(1_700_000_004_000),
            },
        ];
        let stage_e = s.stage.clone();
        let insp_e = s.inspector.clone();
        s.sidebar.update(cx, |sb, sb_cx| {
            stage_e.update(sb_cx, |st, st_cx| {
                insp_e.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &chats);
                });
            });
        });
        s
    })
}

// [T5-01] User switch clears bot_grok_avatars and bot_fabushi_visuals across colliding chat IDs
#[test]
fn t5_01_user_switch_clears_bot_visual_caches() {
    run_in_headless_subprocess("t5_01_user_switch_clears_bot_visual_caches");
}

fn t5_01_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = cx.new(|cx| AppShell::new_with_client(client, cx));
    shell.update(cx, |shell, cx| {
        shell.apply_authenticated_session_with_vault(
            AuthResponse {
                user_id: 1001,
                username: "u1".into(),
                session_id: "s1".into(),
                token: "t1".into(),
                first_name: None,
                created_at: None,
                expires_at: None,
            },
            None,
            cx,
        );
        shell.sidebar.update(cx, |sb, _| {
            sb.bot_grok_avatars.insert(
                42,
                BotGrokAvatarVisual {
                    agent_id: "grok".into(),
                    shape: "helix".into(),
                    color: "#FF00AA".into(),
                    execution_state: "running".into(),
                },
            );
            sb.bot_fabushi_visuals.insert(43, ("lotus".into(), 0x8F59F9));
        });
        shell.apply_authenticated_session_with_vault(
            AuthResponse {
                user_id: 2002,
                username: "u2".into(),
                session_id: "s2".into(),
                token: "t2".into(),
                first_name: None,
                created_at: None,
                expires_at: None,
            },
            None,
            cx,
        );
        let stage_e = shell.stage.clone();
        let insp_e = shell.inspector.clone();
        shell.sidebar.update(cx, |sb, sb_cx| {
            assert!(
                sb.bot_grok_avatars.is_empty(),
                "bot_grok_avatars leaked across user switch"
            );
            assert!(
                sb.bot_fabushi_visuals.is_empty(),
                "bot_fabushi_visuals leaked across user switch"
            );
            let human_dm = ChatSummary {
                chat_id: 42,
                title: "Alice Human".into(),
                chat_type: 0,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(false),
                avatar_color: Some(0x0A84FF),
                updated_at: Some(1_700_000_000_000),
            };
            stage_e.update(sb_cx, |st, st_cx| {
                insp_e.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &[human_dm])
                });
            });
            let d = &sb.dialogs[0];
            let m = sb.dialog_meta(d);
            assert!(matches!(
                resolve_dialog_avatar_kind(d, &m, &TdesktopThemePalette::default()),
                DialogAvatarKind::ClassicInitials { .. }
            ));
        });
    });
}

// [T5-02] Explicit Group/Channel titles ending in "bot" are never misclassified as bots
#[test]
fn t5_02_explicit_group_or_channel_ending_with_bot_not_misclassified() {
    let mut sb = ChatSidebar::new();
    let group_d = DialogItemModel {
        id: 500,
        peer_name: "Team Grok Bot".into(),
        avatar_initials: "TG".into(),
        avatar_color: 0x3A7AFE,
        last_message: "hi".into(),
        timestamp: "12:00".into(),
        unread_count: 0,
        is_muted: false,
        is_pinned: false,
        is_online: false,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "Groups".into(),
    };
    let channel_d = DialogItemModel {
        id: 501,
        peer_name: "News Feed Bot".into(),
        avatar_initials: "NF".into(),
        avatar_color: 0x22C55E,
        last_message: "update".into(),
        timestamp: "12:01".into(),
        unread_count: 0,
        is_muted: false,
        is_pinned: false,
        is_online: false,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "Channels".into(),
    };
    sb.group_chat_ids.insert(500);
    sb.channel_chat_ids.insert(501);
    sb.bot_grok_avatars.insert(
        500,
        BotGrokAvatarVisual {
            agent_id: "grok".into(),
            shape: "orbit".into(),
            color: "#111".into(),
            execution_state: "idle".into(),
        },
    );
    sb.dialogs.push(group_d.clone());
    sb.dialogs.push(channel_d.clone());

    let group_meta = sb.dialog_meta(&group_d);
    let channel_meta = sb.dialog_meta(&channel_d);
    assert!(
        !group_meta.is_bot,
        "explicit group ending in 'Bot' must not set meta.is_bot=true"
    );
    assert!(
        !channel_meta.is_bot,
        "explicit channel ending in 'Bot' must not set meta.is_bot=true"
    );
    assert!(
        group_meta.grok_avatar.is_none(),
        "explicit group must not render a BotGrokAvatarVisual"
    );
    assert!(matches!(
        resolve_dialog_avatar_kind(&group_d, &group_meta, &TdesktopThemePalette::default()),
        DialogAvatarKind::CompositeGroup { seed_id: 500 }
    ));

    let bots_only = ChatFolderDto {
        id: 7,
        title: "AI Bots".into(),
        emoticon: None,
        include_chats: vec![],
        exclude_chats: vec![],
        pinned_chats: vec![],
        include_contacts: false,
        include_non_contacts: false,
        include_groups: false,
        include_channels: false,
        include_bots: true,
        exclude_muted: false,
        exclude_read: false,
        exclude_archived: false,
        sort_order: 0,
    };
    assert!(!sb.dialog_matches_custom_folder(&group_d, &bots_only));
    assert!(!sb.dialog_matches_custom_folder(&channel_d, &bots_only));
}

// [T5-03] Same-turn MiniApp close + relaunch clears stale PendingAction::Close
#[test]
fn t5_03_miniapp_same_turn_close_then_relaunch_survives() {
    run_in_headless_subprocess("t5_03_miniapp_same_turn_close_then_relaunch_survives");
}

async fn t5_03_body(app: AsyncApp) {
    // Keep launch HTTP requests in flight for >200ms so ECONNREFUSED does not tear down the overlay
    // while GPUI pumps `observe(&miniapp_overlay)` for stale `BotWebViewPendingAction::Close`.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock hold listener");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let _hold = stream;
                std::thread::sleep(Duration::from_millis(500));
            });
        }
    });
    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.launch_miniapp_with_url(
                "bot_a",
                "app_a",
                Some(101),
                MiniAppLaunchMode::FullSize,
                None,
                Some("https://example.com/a".into()),
                cx,
            );
            let ov = shell.miniapps.miniapp_overlay.clone().unwrap();
            ov.update(cx, |w, _| {
                w.apply_action_response(MiniAppActionResponse {
                    ok: true,
                    ui_schema: MiniAppUiSchema::default(),
                    state_json: serde_json::Value::Null,
                    companion_bot_message: None,
                    companion_chat_id: None,
                    toast: None,
                    close_app: true,
                    ai_reply: None,
                });
            });
            shell.launch_miniapp_with_url(
                "bot_b",
                "app_b",
                Some(102),
                MiniAppLaunchMode::FullSize,
                None,
                Some("https://example.com/b".into()),
                cx,
            );
            let ov2 = shell.miniapps.miniapp_overlay.clone().unwrap();
            assert!(!ov2
                .read(cx)
                .pending_actions
                .iter()
                .any(|a| matches!(a, BotWebViewPendingAction::Close)));
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    let alive = shell
        .read_with(&app, |s, _| s.miniapps.miniapp_overlay.is_some())
        .unwrap();
    assert!(alive, "newly launched miniapp must not be killed by stale Close");
}

// [T5-04] ConfirmLeaveOrDeleteChat clears composer/inspector on last chat and never resurrects deleted chat draft
#[test]
fn t5_04_leave_or_delete_chat_clears_last_chat_and_avoids_zombie_draft() {
    run_in_headless_subprocess("t5_04_leave_or_delete_chat_clears_last_chat_and_avoids_zombie_draft");
}

fn t5_04_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |shell, cx| {
        let stage_e = shell.stage.clone();
        let insp_e = shell.inspector.clone();
        let c1 = ChatSummary {
            chat_id: 201,
            title: "A".into(),
            chat_type: 1,
            last_message: None,
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: Some(2),
        };
        let c2 = ChatSummary {
            chat_id: 202,
            title: "B".into(),
            chat_type: 1,
            last_message: None,
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: Some(1),
        };
        shell.sidebar.update(cx, |sb, sb_cx| {
            stage_e.update(sb_cx, |st, st_cx| {
                insp_e.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &[c1, c2])
                })
            });
            sb.select_dialog(201);
        });
        shell.sync_selected_chat_inspector_and_stage(201, cx);
        shell.inspector.update(cx, |insp, _| {
            insp.profile.bio = "Old Bot Bio".into();
        });
        shell.composer.update(cx, |comp, _| {
            comp.set_bot_menu(7001, "bot", "App", "https://example.com");
            comp.hermes_slash_enabled = true;
            comp.set_bot_reply_keyboard(
                vec![vec![BotKeyboardButton::new(
                    "K",
                    BotKeyboardButtonKind::Default,
                )]],
                kb_flags::RESIZE,
                None,
            );
            comp.set_text("zombie draft");
        });
        shell.handle_modal_outcome(ModalActionOutcome::ConfirmLeaveOrDeleteChat, cx);
        assert_eq!(
            shell.sidebar.read(cx).drafts.get(&201),
            None,
            "deleted chat draft must not be flushed back"
        );
        shell.composer.update(cx, |comp, _| {
            comp.set_bot_menu(7002, "bot2", "App2", "https://example.com/2");
            comp.hermes_slash_enabled = true;
            comp.set_bot_reply_keyboard(
                vec![vec![BotKeyboardButton::new(
                    "K2",
                    BotKeyboardButtonKind::Default,
                )]],
                kb_flags::RESIZE,
                None,
            );
            comp.set_text("secret bot draft");
        });
        shell.handle_modal_outcome(ModalActionOutcome::ConfirmLeaveOrDeleteChat, cx);
        let comp = shell.composer.read(cx);
        assert!(!comp.bot_menu_controls.is_active);
        assert!(!comp.hermes_slash_enabled);
        assert!(!comp.bot_keyboard.has_markup());
        assert!(comp.text().is_empty());
        assert!(shell.inspector.read(cx).profile.bio.is_empty());
    });
}

// [T5-05] + Compose cancel restores BotKeyboard; Escape modal dismiss clears isolation synchronously
#[test]
fn t5_05_compose_cancel_restores_bot_keyboard_and_escape_clears_isolation_sync() {
    run_in_headless_subprocess(
        "t5_05_compose_cancel_restores_bot_keyboard_and_escape_clears_isolation_sync",
    );
}

async fn t5_05_body(app: AsyncApp) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| {
                comp.set_bot_reply_keyboard(
                    vec![vec![BotKeyboardButton::new(
                        "Persist",
                        BotKeyboardButtonKind::Default,
                    )]],
                    kb_flags::RESIZE | kb_flags::PERSISTENT,
                    None,
                );
            });
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.toggle_plus_menu();
                sb_cx.notify();
            });
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.composer.read(cx).is_compose_isolated);
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.exit_compose_mode();
                sb_cx.notify();
            });
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .read_with(&app, |shell, cx| {
            let comp = shell.composer.read(cx);
            assert!(
                comp.bot_keyboard.has_markup(),
                "BotKeyboard must be restored on + compose cancel"
            );
            assert!(
                comp.is_bot_keyboard_open,
                "BotKeyboard open state must be restored on + compose cancel"
            );
        })
        .unwrap();
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.open_add_contact_modal(AddContactKind::AiBot);
                sb_cx.notify();
            });
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.handle_non_char_key("escape", false, cx));
            assert!(
                !shell.composer.read(cx).is_compose_isolated,
                "Escape must clear is_compose_isolated synchronously"
            );
        })
        .unwrap();
}

// [T5-06] open_bot_message_chat_or_prestart clears prior stage flags/toasts; AppShell::show_toast scopes queued toasts to active chat; empty BotKeyboard effective_height == 0
#[test]
fn t5_06_prestart_stage_isolation_toast_queue_and_empty_keyboard_height() {
    run_in_headless_subprocess(
        "t5_06_prestart_stage_isolation_toast_queue_and_empty_keyboard_height",
    );
}

fn t5_06_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |shell, cx| {
        shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
        shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        shell.show_chat_toast(CID_FABUSHI, "Chat A active", 3000, cx);
        shell.show_toast("Chat A queued via show_toast", 3000, cx);
        assert_eq!(
            shell
                .window_runtime
                .controller
                .toast_queue
                .front()
                .and_then(|t| t.chat_id),
            Some(CID_FABUSHI),
            "Chat A queued show_toast must be scoped to Chat A"
        );

        shell.stage.update(cx, |st, _| {
            st.toast_banner = Some("Group toast".into());
            st.copied_code_msg = Some(999);
            st.is_muted_chat = true;
            st.is_pinned_chat = true;
            st.is_peer_blocked = true;
            st.message_ttl_seconds = 86400;
        });
        shell.inspector.update(cx, |insp, _| {
            insp.set_active_fabushi_bot(Some(FabushiBotSummaryDto {
                bot_user_id: UID_FABUSHI as i64,
                owner_user_id: UID_VIEWER as i64,
                username: "fabushi_bot".into(),
                ..Default::default()
            }));
        });

        shell.open_bot_message_chat_or_prestart(
            777,
            "brand_new_bot",
            "Brand New Bot",
            "desc",
            None,
            false,
            cx,
        );
        let st = shell.stage.read(cx);
        assert_eq!(st.toast_banner, None);
        assert_eq!(st.copied_code_msg, None);
        assert!(!st.is_muted_chat);
        assert!(!st.is_pinned_chat);
        assert!(!st.is_peer_blocked);
        assert_eq!(st.message_ttl_seconds, 0);
        assert!(shell.inspector.read(cx).active_fabushi_bot.is_none());

        assert!(
            shell.window_runtime.controller.current_toast.is_none(),
            "Chat A active and queued show_toast must be cleared synchronously on prestart switch"
        );
        assert!(shell.window_runtime.controller.toast_queue.is_empty());
    });

    let mut kb = BotKeyboard::new();
    kb.set_markup(vec![], 0, None);
    assert_eq!(kb.effective_height(), 0.0);
}

// [T5-07] R2-1: Cold-chat switch before observer latch + queued enqueue_localized_toast do not leak into Chat B
#[test]
fn t5_07_cold_chat_and_queued_localized_toast_do_not_leak_across_switch() {
    run_in_headless_subprocess(
        "t5_07_cold_chat_and_queued_localized_toast_do_not_leak_across_switch",
    );
}

async fn t5_07_body(app: AsyncApp) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock hold listener");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let _hold = stream;
                std::thread::sleep(Duration::from_millis(500));
            });
        }
    });
    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Case A (Cold-chat switch): active_chat = 101 with last_observed_toast_chat_id == None
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.stage.update(cx, |st, _| st.active_chat_id = Some(CID_FABUSHI));
            shell.last_observed_toast_chat_id = None;
            shell.show_toast("Chat 101 cold toast", 3000, cx);
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "cold-chat switch must clear Chat 101 toast synchronously"
            );
            assert!(
                shell.window_runtime.controller.toast_queue.is_empty(),
                "cold-chat switch must leave toast queue empty"
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "cold-chat toast must remain cleared after async observers run"
            );
        })
        .unwrap();

    // Case B (Queued enqueue_localized_toast): Toast 1 active + Toast 2 queued on Chat 101 -> switch to 102
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.show_toast("Chat 101 Toast 1", 3000, cx);
            shell.enqueue_localized_toast("已触发链接按钮", "Link triggered", 1800, cx);
            assert_eq!(shell.window_runtime.controller.toast_queue.len(), 1);
            assert_eq!(
                shell
                    .window_runtime
                    .controller
                    .toast_queue
                    .front()
                    .and_then(|t| t.chat_id),
                Some(CID_FABUSHI),
                "queued enqueue_localized_toast inside Chat 101 must be stamped with Chat 101"
            );
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Chat 101 queued localized toast must NOT promote into Chat 102"
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();
}

// [T5-08] R2-2: Escape dismissal of Contacts modal and modal_container creation dialogs clears isolation sync
#[test]
fn t5_08_escape_contacts_and_modal_container_clear_compose_isolation_sync() {
    run_in_headless_subprocess(
        "t5_08_escape_contacts_and_modal_container_clear_compose_isolation_sync",
    );
}

async fn t5_08_body(app: AsyncApp) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| {
                comp.set_text("hello bot");
                comp.set_bot_reply_keyboard(
                    vec![vec![BotKeyboardButton::new(
                        "Persist",
                        BotKeyboardButtonKind::Default,
                    )]],
                    kb_flags::RESIZE | kb_flags::PERSISTENT,
                    None,
                );
            });
            shell.flush_pending_draft_for_chat(CID_FABUSHI, cx);
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.open_contacts_modal();
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.composer.read(cx).is_compose_isolated);
            assert!(shell.handle_non_char_key("escape", false, cx));
            assert!(!shell.sidebar.read(cx).is_contacts_modal_open);
            let comp = shell.composer.read(cx);
            assert!(
                !comp.is_compose_isolated,
                "Escape closing contacts modal must clear is_compose_isolated synchronously"
            );
            assert_eq!(comp.text(), "hello bot");
            assert!(
                comp.bot_keyboard.has_markup(),
                "Escape closing contacts modal must restore BotKeyboard synchronously"
            );

            // Now test modal_container creation dialog (ModalDialogKind::NewChannel)
            shell.open_modal_dialog("New Channel", ModalDialogKind::NewChannel, 400.0, 300.0, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.modal_container.read(cx).is_modal_active());
            assert!(shell.composer.read(cx).is_compose_isolated);
            assert!(shell.handle_non_char_key("escape", false, cx));
            assert!(!shell.modal_container.read(cx).is_modal_active());
            let comp = shell.composer.read(cx);
            assert!(
                !comp.is_compose_isolated,
                "Escape closing modal_container creation dialog must clear is_compose_isolated synchronously"
            );
            assert_eq!(comp.text(), "hello bot");
            assert!(comp.bot_keyboard.has_markup());
        })
        .unwrap();
}

// [T5-09] R2-3: ConfirmLeaveOrDeleteChat on non-last chat clears reply/edit/attachments/BotKeyboard
#[test]
fn t5_09_leave_non_last_chat_clears_reply_edit_attachments_and_keyboard() {
    run_in_headless_subprocess("t5_09_leave_non_last_chat_clears_reply_edit_attachments_and_keyboard");
}

fn t5_09_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |shell, cx| {
        let stage_e = shell.stage.clone();
        let insp_e = shell.inspector.clone();
        let c1 = ChatSummary {
            chat_id: 201,
            title: "Group A".into(),
            chat_type: 1,
            last_message: None,
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: Some(2),
        };
        let c2 = ChatSummary {
            chat_id: 202,
            title: "Group B".into(),
            chat_type: 1,
            last_message: None,
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: Some(1),
        };
        shell.sidebar.update(cx, |sb, sb_cx| {
            stage_e.update(sb_cx, |st, st_cx| {
                insp_e.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &[c1, c2])
                })
            });
            sb.select_dialog(201);
        });
        shell.sync_selected_chat_inspector_and_stage(201, cx);
        shell.composer.update(cx, |comp, _| {
            comp.begin_reply(999, "Alice".into(), "old chat A msg".into());
            comp.editor.attachments.push(PathBuf::from("/tmp/secret_chat_a.pdf"));
            comp.submitted_attachment = Some("/tmp/secret_chat_a.pdf".to_string());
            comp.pending_attached_files = Some(vec![PathBuf::from("/tmp/secret_chat_a.pdf")]);
            comp.set_bot_reply_keyboard(
                vec![vec![BotKeyboardButton::new("K", BotKeyboardButtonKind::Default)]],
                kb_flags::RESIZE,
                None,
            );
        });

        shell.handle_modal_outcome(ModalActionOutcome::ConfirmLeaveOrDeleteChat, cx);

        assert_eq!(shell.stage.read(cx).active_chat_id, Some(202));
        let comp = shell.composer.read(cx);
        assert!(comp.replying_to.is_none(), "replying_to leaked to next chat");
        assert!(comp.editing_message.is_none(), "editing_message leaked to next chat");
        assert!(comp.editor.attachments.is_empty(), "attachments leaked to next chat");
        assert!(comp.submitted_attachment.is_none());
        assert!(comp.pending_attached_files.is_none());
        assert!(!comp.bot_keyboard.has_markup(), "BotKeyboard leaked to next chat");
    });
}

// [T5-10] R2-4: open_bot_message_chat_or_prestart flushes outgoing draft and resets composer context
#[test]
fn t5_10_prestart_bot_flushes_outgoing_draft_and_resets_composer_context() {
    run_in_headless_subprocess("t5_10_prestart_bot_flushes_outgoing_draft_and_resets_composer_context");
}

fn t5_10_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |shell, cx| {
        shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
        shell.sync_selected_chat_inspector_and_stage(102, cx);
        shell.composer.update(cx, |comp, _| {
            comp.set_text("unsent group 102 draft");
            comp.begin_reply(888, "Bob".into(), "group msg".into());
            comp.editor.attachments.push(PathBuf::from("/tmp/group_file.png"));
            comp.set_bot_reply_keyboard(
                vec![vec![BotKeyboardButton::new("Kb", BotKeyboardButtonKind::Default)]],
                kb_flags::RESIZE,
                None,
            );
        });

        shell.open_bot_message_chat_or_prestart(
            777,
            "brand_new_bot",
            "Brand New Bot",
            "desc",
            None,
            false,
            cx,
        );

        assert_eq!(shell.stage.read(cx).active_chat_id, None);
        assert_eq!(
            shell.sidebar.read(cx).drafts.get(&102).map(String::as_str),
            Some("unsent group 102 draft"),
            "outgoing chat draft must be flushed before entering prestart bot"
        );
        let comp = shell.composer.read(cx);
        assert!(comp.text().is_empty(), "prestart bot composer text must be empty");
        assert!(comp.replying_to.is_none(), "replying_to leaked into prestart bot");
        assert!(comp.editing_message.is_none());
        assert!(comp.editor.attachments.is_empty(), "attachments leaked into prestart bot");
        assert!(!comp.bot_keyboard.has_markup(), "BotKeyboard leaked into prestart bot");
        assert!(comp.send_as_options.is_empty());
    });
}

// [T5-11] R2-5: Human 1v1 DM whose name ends in "bot" (e.g. "John Abbot") is never misclassified as a bot
#[test]
fn t5_11_human_dm_ending_with_bot_not_misclassified() {
    let mut sb = ChatSidebar::new();
    let john = DialogItemModel {
        id: 601,
        peer_name: "John Abbot".into(),
        avatar_initials: "JA".into(),
        avatar_color: 0x0A84FF,
        last_message: "hey".into(),
        timestamp: "12:00".into(),
        unread_count: 0,
        is_muted: false,
        is_pinned: false,
        is_online: false,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "Personal".into(),
    };
    let jane = DialogItemModel {
        id: 602,
        peer_name: "Jane Abbot".into(),
        avatar_initials: "JA".into(),
        avatar_color: 0x30D158,
        last_message: "hi".into(),
        timestamp: "12:01".into(),
        unread_count: 0,
        is_muted: false,
        is_pinned: false,
        is_online: false,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: "All".into(),
    };
    sb.contact_chat_ids.insert(601);
    sb.dialogs.push(john.clone());
    sb.dialogs.push(jane.clone());

    let john_meta = sb.dialog_meta(&john);
    let jane_meta = sb.dialog_meta(&jane);
    assert!(!john_meta.is_bot, "John Abbot contact must have is_bot=false");
    assert!(!jane_meta.is_bot, "Jane Abbot DM must have is_bot=false");
    assert!(matches!(
        resolve_dialog_avatar_kind(&john, &john_meta, &TdesktopThemePalette::default()),
        DialogAvatarKind::ClassicInitials { .. }
    ));

    let bots_only = ChatFolderDto {
        id: 8,
        title: "AI Bots".into(),
        emoticon: None,
        include_chats: vec![],
        exclude_chats: vec![],
        pinned_chats: vec![],
        include_contacts: false,
        include_non_contacts: false,
        include_groups: false,
        include_channels: false,
        include_bots: true,
        exclude_muted: false,
        exclude_read: false,
        exclude_archived: false,
        sort_order: 0,
    };
    let contacts_only = ChatFolderDto {
        id: 9,
        title: "Contacts".into(),
        emoticon: None,
        include_chats: vec![],
        exclude_chats: vec![],
        pinned_chats: vec![],
        include_contacts: true,
        include_non_contacts: false,
        include_groups: false,
        include_channels: false,
        include_bots: false,
        exclude_muted: false,
        exclude_read: false,
        exclude_archived: false,
        sort_order: 1,
    };
    assert!(!sb.dialog_matches_custom_folder(&john, &bots_only));
    assert!(!sb.dialog_matches_custom_folder(&jane, &bots_only));
    assert!(sb.dialog_matches_custom_folder(&john, &contacts_only));
}

// [T5-12] R3-1: SidebarContextAction::DeleteChat never resurrects deleted drafts and cleans last chat completely
#[test]
fn t5_12_sidebar_delete_chat_no_draft_resurrection_and_cleans_last_chat() {
    run_in_headless_subprocess(
        "t5_12_sidebar_delete_chat_no_draft_resurrection_and_cleans_last_chat",
    );
}

fn t5_12_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |shell, cx| {
        let stage_e = shell.stage.clone();
        let insp_e = shell.inspector.clone();
        let c1 = ChatSummary {
            chat_id: 201,
            title: "Group A".into(),
            chat_type: 1,
            last_message: None,
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: Some(2),
        };
        let c2 = ChatSummary {
            chat_id: 202,
            title: "Group B".into(),
            chat_type: 1,
            last_message: None,
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: Some(1),
        };
        shell.sidebar.update(cx, |sb, sb_cx| {
            stage_e.update(sb_cx, |st, st_cx| {
                insp_e.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &[c1, c2])
                })
            });
            sb.select_dialog(201);
        });
        shell.sync_selected_chat_inspector_and_stage(201, cx);

        // Populate draft, reply, attachment, keyboard, and chat-201 toast before sidebar delete.
        shell.composer.update(cx, |comp, _| {
            comp.set_text("secret deleted draft");
            comp.begin_reply(501, "Alice".into(), "msg 201".into());
            comp.editor.attachments.push(PathBuf::from("/tmp/del_201.png"));
            comp.submitted_attachment = Some("/tmp/del_201.png".to_string());
            comp.set_bot_reply_keyboard(
                vec![vec![BotKeyboardButton::new("K", BotKeyboardButtonKind::Default)]],
                kb_flags::RESIZE,
                None,
            );
        });
        shell.flush_pending_draft_for_chat(201, cx);
        shell.show_chat_toast(201, "Chat 201 error", 3000, cx);

        // Simulate `ChatSidebar::apply_context_action(DeleteChat(201))` + `AppShell::handle_sidebar_context_action`.
        shell.sidebar.update(cx, |sb, _| sb.delete_dialog(201));
        shell.handle_sidebar_context_action(SidebarContextAction::DeleteChat(201), cx);

        assert_eq!(shell.stage.read(cx).active_chat_id, Some(202));
        assert_eq!(
            shell.sidebar.read(cx).drafts.get(&201),
            None,
            "sidebar delete must not resurrect deleted draft into sb.drafts"
        );
        assert_eq!(
            shell.client.local_store.get_draft(ChatId(201)),
            None,
            "sidebar delete must clear local_store draft for deleted chat"
        );
        assert!(!shell.session_sync.last_draft_text_by_chat.contains_key(&201));
        assert!(!shell.session_sync.last_saved_draft_by_chat.contains_key(&201));
        assert!(shell.window_runtime.controller.current_toast.is_none());
        assert!(shell.window_runtime.controller.toast_queue.is_empty());
        {
            let comp = shell.composer.read(cx);
            assert!(comp.replying_to.is_none());
            assert!(comp.editor.attachments.is_empty());
            assert!(comp.submitted_attachment.is_none());
            assert!(!comp.bot_keyboard.has_markup());
        }

        // Now delete the last remaining chat (202) via SidebarContextAction::DeleteChat(202).
        shell.inspector.update(cx, |insp, _| {
            insp.profile.bio = "Stale Group Bio".into();
        });
        shell.set_inspector_open(true, cx);
        shell.composer.update(cx, |comp, _| {
            comp.set_text("zombie last draft");
            comp.set_bot_menu(7002, "bot2", "App2", "https://example.com/2");
            comp.hermes_slash_enabled = true;
            comp.begin_reply(555, "Bob".into(), "last msg".into());
            comp.editor.attachments.push(PathBuf::from("/tmp/last.pdf"));
            comp.set_bot_reply_keyboard(
                vec![vec![BotKeyboardButton::new("K2", BotKeyboardButtonKind::Default)]],
                kb_flags::RESIZE,
                None,
            );
        });
        shell.show_chat_toast(202, "Chat 202 toast", 3000, cx);

        shell.sidebar.update(cx, |sb, _| sb.delete_dialog(202));
        shell.handle_sidebar_context_action(SidebarContextAction::DeleteChat(202), cx);

        assert_eq!(shell.stage.read(cx).active_chat_id, None);
        assert_eq!(shell.sidebar.read(cx).drafts.get(&202), None);
        assert_eq!(shell.client.local_store.get_draft(ChatId(202)), None);
        assert!(shell.window_runtime.controller.current_toast.is_none());
        assert!(shell.window_runtime.controller.toast_queue.is_empty());
        assert!(shell.miniapps.pending_prestart_bot.is_none());

        let comp = shell.composer.read(cx);
        assert!(comp.text().is_empty(), "last-chat sidebar delete must clear draft text");
        assert!(!comp.bot_menu_controls.is_active);
        assert!(!comp.hermes_slash_enabled);
        assert!(!comp.bot_keyboard.has_markup());
        assert!(comp.replying_to.is_none());
        assert!(comp.editor.attachments.is_empty());

        let insp = shell.inspector.read(cx);
        assert!(insp.profile.bio.is_empty());
        assert!(!insp.is_open);
    });
}

// [T5-13] R3-2: Chat-scoped toasts & async completion gating across InspectorAction, ConfirmClearChatHistory, and BotAssistCardAction
#[test]
fn t5_13_chat_scoped_toasts_and_async_gating_prevent_cross_chat_leak() {
    run_in_headless_subprocess(
        "t5_13_chat_scoped_toasts_and_async_gating_prevent_cross_chat_leak",
    );
}

async fn t5_13_body(app: AsyncApp) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.contains("/members") {
                    std::thread::sleep(Duration::from_millis(60));
                    let body = if req.starts_with("GET ") {
                        r#"{"members":[]}"#
                    } else {
                        r#"{"ok":true}"#
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let _hold = stream;
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Case A: InspectorAction::StartVoiceCall queued toast on Group 102 must NOT promote into Chat 101
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.show_chat_toast(102, "Toast 1 on 102", 3000, cx);
            shell.handle_inspector_action(InspectorAction::StartVoiceCall("Engineering Group".into()), cx);
            assert_eq!(
                shell.window_runtime.controller.toast_queue.front().and_then(|t| t.chat_id),
                Some(102),
                "InspectorAction toast must be scoped to active chat 102"
            );

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "InspectorAction queued toast from 102 must NOT leak onto 101"
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Case B: ConfirmClearChatHistory queued toast on Chat 101 must NOT promote into Chat 102
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.show_chat_toast(CID_FABUSHI, "Toast 1 on 101", 3000, cx);
            shell.handle_modal_outcome(
                ModalActionOutcome::ConfirmClearChatHistory {
                    also_for_peer: false,
                },
                cx,
            );
            assert_eq!(
                shell.window_runtime.controller.toast_queue.front().and_then(|t| t.chat_id),
                Some(CID_FABUSHI),
                "ConfirmClearChatHistory toast must be scoped to 101"
            );

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "ConfirmClearChatHistory queued toast from 101 must NOT leak onto 102"
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Case C: BotAssistCardAction (SubmitCaptchaClicks empty points & SubmitLogin missing card) scoped to 101
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.show_chat_toast(CID_FABUSHI, "Active 101", 3000, cx);
            shell.handle_bot_assist_card_action(
                BotAssistCardAction::SubmitCaptchaClicks {
                    card_id: "card_missing_101".into(),
                },
                cx,
            );
            assert_eq!(
                shell.window_runtime.controller.toast_queue.front().and_then(|t| t.chat_id),
                Some(CID_FABUSHI)
            );
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Case D: Async OpenAddMember completion after switching away from 102 -> 101 must be gated
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.inspector.update(cx, |insp, _| {
                insp.pending_add_member_user_id = Some(999_001);
            });
            shell.handle_inspector_action(InspectorAction::OpenAddMember, cx);
            // Switch immediately to 101 while add_chat_members is in flight
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(320))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "async OpenAddMember toast for 102 must NOT appear after switching to 101"
            );
        })
        .unwrap();
}

// [T5-14] R3-3: Synchronous is_compose_isolated exit on open_or_select_chat, Cmd+E, NavRail switch, and + -> prestart bot
#[test]
fn t5_14_sync_compose_isolation_exit_on_non_escape_and_prestart() {
    run_in_headless_subprocess("t5_14_sync_compose_isolation_exit_on_non_escape_and_prestart");
}

async fn t5_14_body(app: AsyncApp) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Setup chat 101 with draft + persistent BotKeyboard, enter + compose mode
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| {
                comp.set_text("hello bot");
                comp.set_bot_reply_keyboard(
                    vec![vec![BotKeyboardButton::new(
                        "Persist",
                        BotKeyboardButtonKind::Default,
                    )]],
                    kb_flags::RESIZE | kb_flags::PERSISTENT,
                    None,
                );
            });
            shell.flush_pending_draft_for_chat(CID_FABUSHI, cx);
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.toggle_plus_menu();
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    // 1) open_or_select_chat synchronously exits compose isolation
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.composer.read(cx).is_compose_isolated);
            shell.open_or_select_chat(102, Some("Engineering Group".into()), false, cx);
            assert!(
                !shell.composer.read(cx).is_compose_isolated,
                "open_or_select_chat must clear is_compose_isolated synchronously"
            );
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(25))
        .await;

    // Re-select 101 with draft + persistent BotKeyboard, then enter + compose mode
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| {
                comp.set_text("hello bot");
                comp.set_bot_reply_keyboard(
                    vec![vec![BotKeyboardButton::new(
                        "Persist",
                        BotKeyboardButtonKind::Default,
                    )]],
                    kb_flags::RESIZE | kb_flags::PERSISTENT,
                    None,
                );
            });
            shell.flush_pending_draft_for_chat(CID_FABUSHI, cx);
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.toggle_plus_menu();
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    // 2) Cmd+E synchronously exits compose isolation & restores draft/keyboard
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.composer.read(cx).is_compose_isolated);
            assert!(shell.handle_shortcut_key("e", false, cx));
            let comp = shell.composer.read(cx);
            assert!(
                !comp.is_compose_isolated,
                "Cmd+E must clear is_compose_isolated synchronously"
            );
            assert_eq!(comp.text(), "hello bot");
            assert!(comp.bot_keyboard.has_markup());
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.is_account_popover_open = false;
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(25))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.toggle_plus_menu();
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    // 3) NavRail destination switch synchronously exits compose isolation
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.composer.read(cx).is_compose_isolated);
            shell.activate_nav_destination(NavDestination::Calls, cx);
            assert!(
                !shell.composer.read(cx).is_compose_isolated,
                "NavRail switch must clear is_compose_isolated synchronously"
            );
            shell.activate_nav_destination(NavDestination::Chats, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(25))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.toggle_plus_menu();
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    // 4) + compose -> open_bot_message_chat_or_prestart synchronously clears isolation & send_disabled
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.composer.read(cx).is_compose_isolated);
            shell.open_bot_message_chat_or_prestart(
                888,
                "prestart_bot",
                "Prestart Bot",
                "desc",
                None,
                false,
                cx,
            );
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            let comp = shell.composer.read(cx);
            assert!(
                !comp.is_compose_isolated,
                "open_bot_message_chat_or_prestart from + compose must clear is_compose_isolated synchronously"
            );
            assert!(!comp.compose_send_disabled);
            assert_eq!(shell.stage.read(cx).active_chat_id, None);
        })
        .unwrap();
}

// [T5-15] R4-1: Cmd+N, open_modal_dialog(NewGroup), MainMenuCommand::NewGroup, and CreateGroupSubmitted
// preserve compose_isolation_snapshot (replying_to, attachments, BotKeyboard, and un-flushed draft) across Esc cancel
#[test]
fn t5_15_cmd_n_new_group_and_create_group_submitted_preserve_snapshot() {
    run_in_headless_subprocess(
        "t5_15_cmd_n_new_group_and_create_group_submitted_preserve_snapshot",
    );
}

async fn t5_15_body(app: AsyncApp) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    #[derive(Clone, Copy)]
    enum EntryPath {
        CmdN,
        OpenModalNewGroup,
        MainMenuNewGroup,
        CreateGroupSubmitted,
    }

    let paths = [
        (EntryPath::CmdN, "unflushed cmd_n draft"),
        (EntryPath::OpenModalNewGroup, "unflushed new_group draft"),
        (EntryPath::MainMenuNewGroup, "unflushed menu_group draft"),
        (EntryPath::CreateGroupSubmitted, "unflushed submit_group draft"),
    ];

    for (idx, (path, draft_text)) in paths.into_iter().enumerate() {
        let reply_id = 600 + idx as u64;
        shell
            .update(&mut app.clone(), |shell, cx| {
                shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
                shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
                // Intentionally leave draft un-flushed before triggering compose/group entry!
                shell.composer.update(cx, |comp, _| {
                    comp.set_text(draft_text);
                    comp.begin_reply(reply_id, "Alice".into(), "quoted bot msg".into());
                    comp.editor.attachments.clear();
                    comp.editor
                        .attachments
                        .push(PathBuf::from(format!("/tmp/t5_15_{idx}.png")));
                    comp.set_bot_reply_keyboard(
                        vec![vec![BotKeyboardButton::new(
                            "PersistBtn",
                            BotKeyboardButtonKind::Default,
                        )]],
                        kb_flags::RESIZE | kb_flags::PERSISTENT,
                        None,
                    );
                });

                match path {
                    EntryPath::CmdN => {
                        assert!(shell.handle_shortcut_key("n", false, cx));
                    }
                    EntryPath::OpenModalNewGroup => {
                        shell.open_modal_dialog("New Group", ModalDialogKind::NewGroup, 420.0, 380.0, cx);
                    }
                    EntryPath::MainMenuNewGroup => {
                        shell.handle_main_menu_command(MainMenuCommand::NewGroup, cx);
                    }
                    EntryPath::CreateGroupSubmitted => {
                        shell.handle_modal_outcome(
                            ModalActionOutcome::CreateGroupSubmitted {
                                title: "T5 Group".into(),
                                members_count: 0,
                                enable_forum_topics: false,
                            },
                            cx,
                        );
                    }
                }

                assert!(shell.sidebar.read(cx).is_plus_menu_open);
                let comp = shell.composer.read(cx);
                assert!(comp.is_compose_isolated);
                assert!(comp.text().is_empty());
                assert!(comp.replying_to.is_none());
                assert!(comp.editor.attachments.is_empty());
                assert!(!comp.bot_keyboard.has_markup());
            })
            .unwrap();

        // Pump async observers before cancelling with Esc (mirroring real event loop ticks)
        app.background_executor()
            .timer(Duration::from_millis(35))
            .await;

        shell
            .update(&mut app.clone(), |shell, cx| {
                // Step through progressive Grok Esc cascade (menu_open -> exit compose mode)
                while shell.sidebar.read(cx).is_plus_menu_open {
                    assert!(shell.handle_non_char_key("escape", false, cx));
                }
                assert!(!shell.sidebar.read(cx).is_plus_menu_open);
                let comp = shell.composer.read(cx);
                assert!(!comp.is_compose_isolated);
                assert_eq!(
                    comp.text(),
                    draft_text,
                    "Esc cancel must restore un-flushed draft for entry path #{idx}"
                );
                assert_eq!(
                    comp.replying_to.as_ref().map(|r| r.0),
                    Some(reply_id),
                    "Esc cancel must restore replying_to for entry path #{idx}"
                );
                assert_eq!(
                    comp.editor.attachments.len(),
                    1,
                    "Esc cancel must restore attachments for entry path #{idx}"
                );
                assert!(
                    comp.bot_keyboard.has_markup(),
                    "Esc cancel must restore persistent BotKeyboard for entry path #{idx}"
                );
            })
            .unwrap();

        app.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

// [T5-16] R4-2: Cmd+7 (NavDestination::SavedMessages) and Cmd+, synchronously exit + compose mode and clear is_compose_isolated
#[test]
fn t5_16_saved_messages_and_cmd_comma_exit_compose_and_isolation_sync() {
    run_in_headless_subprocess(
        "t5_16_saved_messages_and_cmd_comma_exit_compose_and_isolation_sync",
    );
}

async fn t5_16_body(app: AsyncApp) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Register SavedMessages dialog (id = UID_VIEWER = 100_001) and enter + compose from Chat 101
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: UID_VIEWER,
                    peer_name: "Saved Messages".to_string(),
                    avatar_initials: "SM".to_string(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: "now".to_string(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: false,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".to_string(),
                });
                sb.saved_messages_id = Some(UID_VIEWER);
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| {
                comp.set_text("hello bot");
                comp.set_bot_reply_keyboard(
                    vec![vec![BotKeyboardButton::new(
                        "Persist",
                        BotKeyboardButtonKind::Default,
                    )]],
                    kb_flags::RESIZE | kb_flags::PERSISTENT,
                    None,
                );
            });
            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.composer.read(cx).is_compose_isolated);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    // 1) Cmd+7 (SavedMessages) exits compose mode & isolation synchronously and preserves Chat 101 draft
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.handle_shortcut_key("7", false, cx));
            assert!(
                !shell.sidebar.read(cx).is_plus_menu_open,
                "Cmd+7 must synchronously exit + compose mode"
            );
            assert!(
                !shell.composer.read(cx).is_compose_isolated,
                "Cmd+7 must synchronously clear is_compose_isolated"
            );
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(UID_VIEWER));
            assert_eq!(
                shell.session_sync.last_draft_text_by_chat.get(&CID_FABUSHI).map(String::as_str),
                Some("hello bot")
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            assert!(!shell.composer.read(cx).is_compose_isolated);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(UID_VIEWER));
        })
        .unwrap();

    // 2) Switch back to Chat 101 with persistent BotKeyboard, enter + compose, then Cmd+, (Settings) synchronously exits compose & isolation
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| {
                comp.set_bot_reply_keyboard(
                    vec![vec![BotKeyboardButton::new(
                        "Persist",
                        BotKeyboardButtonKind::Default,
                    )]],
                    kb_flags::RESIZE | kb_flags::PERSISTENT,
                    None,
                );
            });
            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.composer.read(cx).is_compose_isolated);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.handle_shortcut_key(",", false, cx));
            assert!(
                !shell.sidebar.read(cx).is_plus_menu_open,
                "Cmd+, must synchronously exit + compose mode"
            );
            assert!(
                !shell.composer.read(cx).is_compose_isolated,
                "Cmd+, must synchronously clear is_compose_isolated"
            );
            assert_eq!(shell.active_panel, ActiveWorkspacePanel::Settings);
            assert_eq!(shell.composer.read(cx).text(), "hello bot");
            assert!(shell.composer.read(cx).bot_keyboard.has_markup());
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            assert!(!shell.composer.read(cx).is_compose_isolated);
            assert_eq!(shell.active_panel, ActiveWorkspacePanel::Settings);
        })
        .unwrap();

    // 3) Cmd+7 when saved_messages_id is unresolved still exits + compose & isolation cleanly
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.close_settings(cx);
            shell.sidebar.update(cx, |sb, _| {
                sb.saved_messages_id = None;
                sb.dialogs.retain(|d| d.id != UID_VIEWER);
            });
            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.composer.read(cx).is_compose_isolated);
            assert!(shell.handle_shortcut_key("7", false, cx));
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            assert!(!shell.composer.read(cx).is_compose_isolated);
        })
        .unwrap();
}

// [T5-17] R4-3 & Reviewer R3-2 Minor Finding 1:
// 1) Deleting a bot chat clears bot_profiles_cache & stage per-chat caches so reopening opens prestart instead of resurrecting zombie chat
// 2) persist_bot_computer_profile async completion gates inspector & toast on target_cid
#[test]
fn t5_17_delete_bot_chat_clears_bot_profiles_and_stage_caches_plus_computer_toast_scoping() {
    run_in_headless_subprocess(
        "t5_17_delete_bot_chat_clears_bot_profiles_and_stage_caches_plus_computer_toast_scoping",
    );
}

async fn t5_17_body(app: AsyncApp) {
    use std::io::{Read, Write};

    let mock_profile_json = serde_json::to_string(&BotComputerProfileDto {
        bot_id: UID_FABUSHI,
        bound_node_id: None,
        persona_prompt: "Updated Persona".to_string(),
        preferred_model: "gpt-5".to_string(),
        approval_mode: ComputerRunPolicy::default(),
        allow_cross_device_control: false,
        allow_cloud_self_config: false,
        updated_at_ms: 1_700_000_010_000,
    })
    .unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock computer server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let body = mock_profile_json.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.contains("/computer") {
                    std::thread::sleep(Duration::from_millis(60));
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let _hold = stream;
                    std::thread::sleep(Duration::from_millis(400));
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part 1: persist_bot_computer_profile async completion after switching 101 -> 102 must NOT emit stale toast or mutate 102's inspector
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.inspector.update(cx, |insp, _| {
                insp.subpage = InspectorSubpage::BotConfigEditor;
            });
            shell.handle_inspector_action(
                InspectorAction::SaveBotComputerConfig {
                    bot_id: UID_FABUSHI,
                    persona_prompt: "Updated Persona".into(),
                    preferred_model: "gpt-5".into(),
                    approval_mode: ComputerRunPolicy::default(),
                    allow_cross_device_control: false,
                    allow_cloud_self_config: false,
                },
                cx,
            );
            // Switch immediately to Group 102 while PUT /computer is in flight
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.inspector.update(cx, |insp, _| {
                insp.subpage = InspectorSubpage::ComputersList;
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(320))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "persist_bot_computer_profile toast for 101 must NOT appear after switching to 102"
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
            assert_eq!(
                shell.inspector.read(cx).subpage,
                InspectorSubpage::ComputersList,
                "persist_bot_computer_profile completion for 101 must NOT reset 102's inspector subpage"
            );
        })
        .unwrap();

    // Part 2: Deleting Bot chat 101 clears stage caches & bot_profiles_cache so reopening opens prestart
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.stage.update(cx, |st, _| {
                st.chat_histories.insert(
                    CID_FABUSHI,
                    vec![MessageItemModel {
                        id: 901,
                        sender_id: UID_FABUSHI,
                        sender_name: "Fabushi".into(),
                        avatar_initials: "FB".into(),
                        avatar_color: 0x8F59F9,
                        direction: MessageDirection::Incoming,
                        timestamp: "10:00".into(),
                        delivery_status: DeliveryStatus::Read,
                        reply_to: None,
                        reactions: Vec::new(),
                        content: MessageContent::Text {
                            text: "cached bot msg".into(),
                            is_edited: false,
                        },
                        bot_keyboard: None,
                        is_pinned: false,
                    }],
                );
                st.chat_pinned_messages
                    .insert(CID_FABUSHI, Some("Pinned bot notice".into()));
                st.chat_scroll_positions
                    .insert(CID_FABUSHI, (gpui::Point::default(), true));
            });

            // Delete chat 101 via SidebarContextAction::DeleteChat
            shell.sidebar.update(cx, |sb, _| sb.delete_dialog(CID_FABUSHI));
            shell.handle_sidebar_context_action(SidebarContextAction::DeleteChat(CID_FABUSHI), cx);

            assert!(
                !shell.sidebar.read(cx).dialogs.iter().any(|d| d.id == CID_FABUSHI),
                "deleted bot chat must not remain in sidebar dialogs"
            );
            {
                let st = shell.stage.read(cx);
                assert!(st.chat_histories.get(&CID_FABUSHI).is_none());
                assert!(st.chat_pinned_messages.get(&CID_FABUSHI).is_none());
                assert!(st.chat_pinned_entries.get(&CID_FABUSHI).is_none());
                assert!(st.chat_scroll_positions.get(&CID_FABUSHI).is_none());
            }
            {
                let prof = shell
                    .miniapps
                    .bot_profiles_cache
                    .get("fabushi_bot")
                    .expect("cached bot profile entry");
                assert_eq!(prof.existing_chat_id, None);
                assert!(!prof.has_started_chat);
                assert_eq!(
                    prof.main_app.as_ref().and_then(|a| a.direct_chat_id),
                    None
                );
                assert_eq!(shell.miniapps.fabushi_bots[0].companion_chat_id, None);
            }

            // Re-open the bot from cached profile: must open virtual prestart, NEVER resurrect deleted chat 101
            let prof = shell
                .miniapps
                .bot_profiles_cache
                .get("fabushi_bot")
                .unwrap()
                .clone();
            shell.open_bot_message_chat_or_prestart(
                UID_FABUSHI,
                &prof.bot.username,
                &prof.bot.display_name,
                &prof.bot.description,
                prof.existing_chat_id,
                prof.has_started_chat,
                cx,
            );
            assert!(
                !shell.sidebar.read(cx).dialogs.iter().any(|d| d.id == CID_FABUSHI),
                "open_bot_message_chat_or_prestart must NEVER resurrect deleted bot chat 101"
            );
            assert_eq!(shell.stage.read(cx).active_chat_id, None);
            assert!(shell.stage.read(cx).show_bot_start_bar);
            assert!(shell.miniapps.pending_prestart_bot.is_some());
        })
        .unwrap();
}

// [T5-18] R5-1: apply_local_fabushi_turn_error_to_shell gates chat toast on active_chat_id
// while still persisting the inline error card into chat_histories when switched away.
#[test]
fn t5_18_fabushi_turn_error_toast_gated_on_active_chat() {
    run_in_headless_subprocess("t5_18_fabushi_turn_error_toast_gated_on_active_chat");
}

fn t5_18_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let _rt = client.runtime().enter();
    let shell = setup_authenticated_shell(client, cx);

    shell.update(cx, |shell, cx| {
        // Switch from Bot Chat 101 to Group Chat 102 before the Bot Chat 101 error arrives.
        shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
        shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
        shell.sync_selected_chat_inspector_and_stage(102, cx);
        assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
        assert!(shell.window_runtime.controller.current_toast.is_none());

        shell.apply_local_fabushi_turn_error_to_shell(
            CID_FABUSHI,
            UID_FABUSHI as i64,
            "Fabushi AI",
            "PROVIDER_ERROR: No configured provider",
            cx,
        );

        // Must NOT leak Chat 101's PROVIDER_ERROR toast onto Group Chat 102.
        assert!(
            shell.window_runtime.controller.current_toast.is_none(),
            "Bot Chat 101 PROVIDER_ERROR toast must NOT leak onto active Group Chat 102"
        );
        assert!(shell.window_runtime.controller.toast_queue.is_empty());

        // Must still insert the inline error card into Chat 101's cached history.
        let hist_101 = shell
            .stage
            .read(cx)
            .chat_histories
            .get(&CID_FABUSHI)
            .expect("chat_histories entry for 101");
        assert!(
            hist_101.iter().any(|m| match &m.content {
                MessageContent::Text { text, .. } => text.contains("No configured provider"),
                _ => false,
            }),
            "Chat 101 history must still record the inline PROVIDER_ERROR card"
        );

        // When Chat 101 IS active, apply_local_fabushi_turn_error_to_shell MUST show the chat toast.
        shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
        shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));

        shell.apply_local_fabushi_turn_error_to_shell(
            CID_FABUSHI,
            UID_FABUSHI as i64,
            "Fabushi AI",
            "PROVIDER_ERROR: No configured provider",
            cx,
        );

        let toast = shell
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .expect("toast must be shown when Bot Chat 101 is active");
        assert_eq!(toast.chat_id, Some(CID_FABUSHI));
    });
}

// [T5-19] R5-2: open_bot_chat_by_username("unknown_new_bot") & open_miniapps_center from active
// `+` compose mode synchronously exit compose mode, clear compose_draft/query, and restore isolation.
#[test]
fn t5_19_open_bot_chat_by_username_exits_compose_and_isolation_sync() {
    run_in_headless_subprocess("t5_19_open_bot_chat_by_username_exits_compose_and_isolation_sync");
}

async fn t5_19_body(app: AsyncApp) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock add_contact server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                std::thread::sleep(Duration::from_millis(60));
                let body = r#"{"error":"not_found"}"#;
                let resp = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Case 1: open_bot_chat_by_username("unknown_new_bot") with existing_cid == None
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| comp.set_text("keep_101_draft"));

            // Enter `+` compose mode with dirty compose_draft & compose_recipient_query
            assert!(shell.handle_shortcut_key("n", false, cx));
            shell.sidebar.update(cx, |sb, _| {
                sb.compose_draft.is_group_mode = true;
                sb.compose_draft.custom_group_title = "Dirty Title".into();
                sb.compose_recipient_query = "stale_query".into();
            });
            assert!(shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.composer.read(cx).is_compose_isolated);
            assert!(shell.composer.read(cx).text().is_empty());

            shell.open_bot_chat_by_username("unknown_new_bot", cx);

            // Synchronously (before async add_contact resolves): compose mode, draft, query, and
            // isolation MUST all be cleared and Chat 101's draft restored.
            let sb = shell.sidebar.read(cx);
            assert!(!sb.is_plus_menu_open);
            assert!(!sb.compose_draft.is_group_mode);
            assert!(sb.compose_draft.custom_group_title.is_empty());
            assert!(sb.compose_recipient_query.is_empty());
            let comp = shell.composer.read(cx);
            assert!(
                !comp.is_compose_isolated,
                "is_compose_isolated must be cleared synchronously on open_bot_chat_by_username"
            );
            assert_eq!(comp.text(), "keep_101_draft");
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(!sb.is_plus_menu_open);
            assert!(!sb.compose_draft.is_group_mode);
            assert!(sb.compose_recipient_query.is_empty());
            let comp = shell.composer.read(cx);
            assert!(!comp.is_compose_isolated);
            assert_eq!(comp.text(), "keep_101_draft");
        })
        .unwrap();

    // Case 2: sibling open_miniapps_center also calls exit_compose_mode() synchronously
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.handle_shortcut_key("n", false, cx));
            shell.sidebar.update(cx, |sb, _| {
                sb.compose_draft.is_group_mode = true;
                sb.compose_recipient_query = "miniapp_stale_query".into();
            });
            assert!(shell.composer.read(cx).is_compose_isolated);

            shell.open_miniapps_center(cx);

            let sb = shell.sidebar.read(cx);
            assert!(!sb.is_plus_menu_open);
            assert!(!sb.compose_draft.is_group_mode);
            assert!(sb.compose_recipient_query.is_empty());
            let comp = shell.composer.read(cx);
            assert!(!comp.is_compose_isolated);
            assert_eq!(comp.text(), "keep_101_draft");
        })
        .unwrap();
}

// [T5-20] R6-1: execute_hermes_inline_callback, StageAction::DecideToolApproval, and
// load_historical_messages gate async chat toasts & background history loads on active_chat_id.
#[test]
fn t5_20_hermes_callback_approval_and_history_toast_gated_on_switch() {
    run_in_headless_subprocess("t5_20_hermes_callback_approval_and_history_toast_gated_on_switch");
}

async fn t5_20_body(app: AsyncApp) {
    use gpui_telegram_desktop::components::tool_permission_dock::ToolApprovalDecision;
    use gpui_telegram_desktop::views::conversation_stage::StageAction;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let hist_101_hits = Arc::new(AtomicUsize::new(0));
    let fail_101_hist = Arc::new(AtomicBool::new(false));
    let hist_101_hits_srv = Arc::clone(&hist_101_hits);
    let fail_101_hist_srv = Arc::clone(&fail_101_hist);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let hits_req = Arc::clone(&hist_101_hits_srv);
            let fail_101 = Arc::clone(&fail_101_hist_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let first_line = req_str.lines().next().unwrap_or("");
                std::thread::sleep(Duration::from_millis(60));
                let (status, body) = if first_line.contains("/api/v1/hermes/command") {
                    (
                        "200 OK",
                        r#"{"command":"/status","output":"ok","reply_text":"Status OK from 101"}"#,
                    )
                } else if first_line.contains("/api/v1/hermes/approvals/") {
                    (
                        "200 OK",
                        r#"{"approval_id":"appr_1","tool_name":"shell_exec","status":"approved"}"#,
                    )
                } else if first_line.contains("/api/v1/chats/101/messages") {
                    hits_req.fetch_add(1, Ordering::SeqCst);
                    if fail_101.load(Ordering::SeqCst) {
                        ("500 Internal Server Error", r#"{"error":"history_err_101"}"#)
                    } else {
                        ("200 OK", r#"[]"#)
                    }
                } else if first_line.contains("/messages") {
                    ("200 OK", r#"[]"#)
                } else {
                    ("200 OK", r#"{}"#)
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part A: execute_hermes_inline_callback on 101 -> switch to 102 before async completion.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
        })
        .unwrap();

    // Let initial Chat 101 stage hydration finish before arming the callback probe.
    app.background_executor()
        .timer(Duration::from_millis(320))
        .await;

    let mut gen_after_switch_to_102 = 0u64;
    shell
        .update(&mut app.clone(), |shell, cx| {
            hist_101_hits.store(0, Ordering::SeqCst);
            shell.execute_hermes_inline_callback("hermes:/status".to_string(), cx);

            // Switch to Group Chat 102 before async command returns.
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert!(shell.window_runtime.controller.current_toast.is_none());
            gen_after_switch_to_102 = shell.session_sync.history_request_generation;
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Chat 101 Hermes callback toast leaked onto Chat 102: {:?}",
                shell.window_runtime.controller.current_toast
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
            assert_eq!(
                shell.session_sync.history_request_generation,
                gen_after_switch_to_102,
                "Background Chat 101 callback must NOT bump history_request_generation while Chat 102 is active"
            );
            assert_eq!(
                hist_101_hits.load(Ordering::SeqCst),
                0,
                "Background Chat 101 history load must be skipped when Chat 102 is active"
            );
        })
        .unwrap();

    // Part B: StageAction::DecideToolApproval on 101 -> switch to 102 before async completion.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.stage.update(cx, |st, _| {
                st.pending_stage_action = Some(StageAction::DecideToolApproval {
                    approval_id: "appr_1".to_string(),
                    decision: ToolApprovalDecision::AllowOnce,
                });
            });
            shell.handle_stage_pending_actions(cx);

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Chat 101 DecideToolApproval toast leaked onto Chat 102: {:?}",
                shell.window_runtime.controller.current_toast
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Part C: load_historical_messages(101) error while user switched to 102 -> no error toast leak.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            fail_101_hist.store(true, Ordering::SeqCst);
            shell.load_historical_messages(CID_FABUSHI, cx);

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Chat 101 history-load error toast leaked onto Chat 102: {:?}",
                shell.window_runtime.controller.current_toast
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Positive sanity check: staying on Chat 101 DOES show Chat 101's scoped toast.
    fail_101_hist.store(false, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.execute_hermes_inline_callback("hermes:/status".to_string(), cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected toast when staying on active Chat 101");
            assert_eq!(toast.chat_id, Some(CID_FABUSHI));
            assert_eq!(toast.text, "Status OK from 101");
        })
        .unwrap();
}

// [T5-21] R6-2: observe_sidebar_entity open_menu (open_main_menu_requested = true) exits compose
// isolation synchronously even when triggered in the same turn while prev_compose_or_modal == false.
#[test]
fn t5_21_sidebar_open_menu_same_turn_exits_compose_and_isolation_sync() {
    run_in_headless_subprocess("t5_21_sidebar_open_menu_same_turn_exits_compose_and_isolation_sync");
}

async fn t5_21_body(app: AsyncApp) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| comp.set_text("keep_101_draft"));

            // Trigger compose isolation + same-turn open_main_menu_requested before
            // observe_sidebar_entity has latched prev_compose_or_modal = true.
            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.composer.read(cx).is_compose_isolated);
            assert!(shell.composer.read(cx).text().is_empty());

            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.compose_draft.is_group_mode = true;
                sb.compose_recipient_query = "stale_menu_query".into();
                sb.open_main_menu_requested = true;
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(sb.is_account_popover_open, "account popover should open");
            assert!(!sb.is_plus_menu_open, "plus compose mode must be exited");
            assert!(!sb.compose_draft.is_group_mode);
            assert!(sb.compose_recipient_query.is_empty());

            let comp = shell.composer.read(cx);
            assert!(
                !comp.is_compose_isolated,
                "open_menu in same turn must clear composer.is_compose_isolated"
            );
            assert_eq!(comp.text(), "keep_101_draft");
        })
        .unwrap();
}

// [T5-22] R6-4 & R6-5: build_local_fabushi_error_card & apply_local_fabushi_turn_error_to_shell
// strip `"Internal error: PROVIDER_ERROR:"` and distinguish unconfigured-provider errors from
// configured-provider HTTP 429/500/Transport/Auth failures.
#[test]
fn t5_22_local_fabushi_error_card_strips_internal_prefix_and_classifies_provider_errors() {
    run_in_headless_subprocess(
        "t5_22_local_fabushi_error_card_strips_internal_prefix_and_classifies_provider_errors",
    );
}

fn t5_22_body(cx: &mut App) {
    use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
    use gpui_telegram_agent::hermes::providers::ProviderConfig;
    use gpui_telegram_core::error::TelegramError;
    use gpui_telegram_desktop::views::stage::cards::InlineButtonType;

    // 1. Real TelegramError::Internal("PROVIDER_ERROR: ...") prefix stripping for unconfigured host
    let uncfg_err = TelegramError::Internal(
        "PROVIDER_ERROR: Provider configuration error: 未检测到已配置的 AI 订阅或模型供应商。"
            .to_string(),
    )
    .to_string();
    let (card_uncfg_zh, kb_uncfg_zh, toast_uncfg_zh) =
        AppShell::build_local_fabushi_error_card(&uncfg_err, true, true);
    assert!(card_uncfg_zh.contains("⚠️ **未配置可用的 AI 供应商**"), "got {card_uncfg_zh}");
    assert!(!card_uncfg_zh.contains("Internal error:"), "leaked Internal error prefix: {card_uncfg_zh}");
    assert!(!card_uncfg_zh.contains("PROVIDER_ERROR:"), "leaked PROVIDER_ERROR prefix: {card_uncfg_zh}");
    assert_eq!(
        kb_uncfg_zh.as_ref().unwrap()[0][0].button_type,
        InlineButtonType::Callback("hermes:open_ai_providers".to_string())
    );
    assert_eq!(toast_uncfg_zh, "AI 供应商未就绪，请点击消息按钮配置");

    // 2. Configured-provider transient/upstream/auth/transport PROVIDER_ERRORs (429/500/Transport/Auth)
    for raw in [
        "PROVIDER_ERROR: Provider rate limited (HTTP 429): Rate limit reached for gpt-4o",
        "PROVIDER_ERROR: Provider upstream HTTP 500: Internal server error",
        "PROVIDER_ERROR: Provider transport error: connection reset by peer",
        "PROVIDER_ERROR: Provider authentication error: Invalid API key",
    ] {
        let wrapped = TelegramError::Internal(raw.to_string()).to_string();
        let (card_zh, kb_zh, toast_zh) =
            AppShell::build_local_fabushi_error_card(&wrapped, true, false);
        assert!(card_zh.contains("⚠️ **AI 服务请求失败**"), "got {card_zh}");
        assert!(!card_zh.contains("未配置可用的 AI 供应商"), "falsely claimed unconfigured: {card_zh}");
        assert!(!card_zh.contains("Internal error:"), "leaked Internal error: {card_zh}");
        assert!(!card_zh.contains("PROVIDER_ERROR:"), "leaked PROVIDER_ERROR: {card_zh}");
        assert!(kb_zh.is_none(), "configured provider request failure should not show configure CTA");
        assert!(toast_zh.chars().count() <= 80 && !toast_zh.contains("PROVIDER_ERROR"));

        let (card_en, kb_en, _) =
            AppShell::build_local_fabushi_error_card(&wrapped, false, false);
        assert!(card_en.contains("⚠️ **AI Provider Request Failed**"), "got {card_en}");
        assert!(!card_en.contains("AI Provider Not Ready"), "falsely claimed not ready: {card_en}");
        assert!(!card_en.contains("Internal error:"), "leaked Internal error: {card_en}");
        assert!(!card_en.contains("PROVIDER_ERROR:"), "leaked PROVIDER_ERROR: {card_en}");
        assert!(kb_en.is_none());
    }

    // 3. End-to-end apply_local_fabushi_turn_error_to_shell with attached configured FabushiBotHost
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let _rt = client.runtime().enter();
    let host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("gpt-4o").expect("ephemeral host"),
    );
    let uid_str = UID_VIEWER.to_string();
    host.core_engine()
        .providers_for_user(&uid_str)
        .register_provider(ProviderConfig::openai_compatible(
            "custom_openai",
            "Custom OpenAI",
            "http://127.0.0.1:9/v1",
            "gpt-4o",
            "sk-configured-test",
        ));
    let _ = host
        .core_engine()
        .providers_for_user(&uid_str)
        .set_active("custom_openai", Some("gpt-4o"));
    assert!(
        !host
            .core_engine()
            .resolve_providers(Some(&uid_str))
            .needs_provider_selection(),
        "configured user router must have needs_provider_selection == false"
    );

    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |shell, cx| {
        shell.attach_local_fabushi_host(host);
        shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
        shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);

        let wrapped_429 = TelegramError::Internal(
            "PROVIDER_ERROR: Provider rate limited (HTTP 429): Rate limit reached".to_string(),
        )
        .to_string();
        shell.apply_local_fabushi_turn_error_to_shell(
            CID_FABUSHI,
            UID_FABUSHI as i64,
            "Fabushi AI",
            &wrapped_429,
            cx,
        );

        let card = shell.stage.read(cx).messages.last().expect("error card");
        let MessageContent::Text { ref text, .. } = card.content else {
            panic!("expected text error card");
        };
        assert!(text.contains("⚠️ **AI 服务请求失败**"), "got {text}");
        assert!(!text.contains("未配置可用的 AI 供应商"), "got {text}");
        assert!(!text.contains("Internal error:"), "got {text}");
        assert!(!text.contains("PROVIDER_ERROR:"), "got {text}");
        assert!(card.bot_keyboard.is_none());

        // [R7-3] Configured upstream HTTP 401/400/exhausted errors mentioning "Missing API key"
        // must classify as provider request failures (no CTA), not unconfigured provider.
        for upstream_missing_key in [
            "PROVIDER_ERROR: Provider authentication error: HTTP 401: {\"error\":{\"message\":\"Missing API key in Authorization header\"}}",
            "PROVIDER_ERROR: Provider upstream HTTP 400: Missing API key",
            "PROVIDER_ERROR: All providers exhausted: openai/gpt-4o: Provider authentication error: HTTP 401: Missing API key",
        ] {
            let wrapped = TelegramError::Internal(upstream_missing_key.to_string()).to_string();
            let (card_zh, kb_zh, _) =
                AppShell::build_local_fabushi_error_card(&wrapped, true, false);
            assert!(
                card_zh.contains("⚠️ **AI 服务请求失败**"),
                "upstream missing-key 401/400 must be request error (zh), got {card_zh}"
            );
            assert!(!card_zh.contains("未配置可用的 AI 供应商"), "got {card_zh}");
            assert!(kb_zh.is_none(), "upstream 401/400 must not attach configure CTA");

            let (card_en, kb_en, _) =
                AppShell::build_local_fabushi_error_card(&wrapped, false, false);
            assert!(
                card_en.contains("⚠️ **AI Provider Request Failed**"),
                "upstream missing-key 401/400 must be request error (en), got {card_en}"
            );
            assert!(!card_en.contains("AI Provider Not Ready"), "got {card_en}");
            assert!(kb_en.is_none());

            let prev_len = shell.stage.read(cx).messages.len();
            shell.apply_local_fabushi_turn_error_to_shell(
                CID_FABUSHI,
                UID_FABUSHI as i64,
                "Fabushi AI",
                &wrapped,
                cx,
            );
            let msgs = &shell.stage.read(cx).messages;
            assert_eq!(msgs.len(), prev_len + 1);
            let added = msgs.last().expect("added error card");
            let MessageContent::Text { ref text, .. } = added.content else {
                panic!("expected text error card");
            };
            assert!(text.contains("⚠️ **AI 服务请求失败**"), "got {text}");
            assert!(added.bot_keyboard.is_none());
        }

        // Backward-compatibility checks for R7-3:
        // (a) needs_provider_cta == true even with HTTP 401 -> still produces CTA
        let (card_cta, kb_cta, _) = AppShell::build_local_fabushi_error_card(
            "PROVIDER_ERROR: Provider authentication error: HTTP 401: Missing API key",
            true,
            true,
        );
        assert!(card_cta.contains("⚠️ **未配置可用的 AI 供应商**"), "got {card_cta}");
        assert_eq!(
            kb_cta.as_ref().unwrap()[0][0].button_type,
            InlineButtonType::Callback("hermes:open_ai_providers".to_string())
        );

        // (b) bare "PROVIDER_ERROR: missing API key\nline2" with needs_provider_cta == false
        // and no upstream HTTP/request marker -> still produces unconfigured CTA
        let (card_bare, kb_bare, toast_bare) = AppShell::build_local_fabushi_error_card(
            "PROVIDER_ERROR: missing API key\nline2",
            true,
            false,
        );
        assert!(card_bare.contains("⚠️ **未配置可用的 AI 供应商**"), "got {card_bare}");
        assert!(kb_bare.is_some());
        assert!(!toast_bare.contains('\n') && toast_bare.chars().count() <= 80);
    });
}

// [T5-23] R7-1: toggle_bot_block_status cold chat resolution & origin_chat == None toast scoping
#[test]
fn t5_23_toggle_bot_block_cold_chat_and_profile_box_toast_scoping() {
    run_in_headless_subprocess("t5_23_toggle_bot_block_cold_chat_and_profile_box_toast_scoping");
}

async fn t5_23_body(app: AsyncApp) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/block") {
                    std::thread::sleep(Duration::from_millis(60));
                    let body = r#"{"ok":true}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part A: Cold chat (rich_chats_by_id empty for 101) -> toggle_bot_block_status -> switch to 102
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(!shell.session_sync.rich_chats_by_id.contains_key(&CID_FABUSHI));
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));

            shell.toggle_bot_block_status(UID_FABUSHI, cx);
            assert!(
                shell.stage.read(cx).is_peer_blocked,
                "cold chat 101 must still resolve origin_chat and update is_peer_blocked"
            );

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "cold chat 101 toggle_bot_block_status toast leaked onto chat 102: {:?}",
                shell.window_runtime.controller.current_toast
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Part B: BotProfileBox / origin_chat == None triggered while viewing Group 102 -> switch to 101
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            let prev_group_blocked = shell.stage.read(cx).is_peer_blocked;
            shell.toggle_bot_block_status(UID_FABUSHI, cx);
            assert_eq!(
                shell.stage.read(cx).is_peer_blocked,
                prev_group_blocked,
                "blocking bot from group 102 must NOT mutate group 102's is_peer_blocked"
            );

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "group-102 origin_chat == None toggle_bot_block_status toast leaked onto chat 101: {:?}",
                shell.window_runtime.controller.current_toast
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Part C: Positive checks — staying on cold 101 or staying on 102 scopes toast to active chat
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.toggle_bot_block_status(UID_FABUSHI, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            let t = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected toast when staying on cold chat 101");
            assert_eq!(t.chat_id, Some(CID_FABUSHI));
        })
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.window_runtime.controller.clear_all_toasts();
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.toggle_bot_block_status(UID_FABUSHI, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            let t = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected toast when staying on chat 102");
            assert_eq!(t.chat_id, Some(102));

            // Switching away after toast appeared clears the chat-102-scoped toast
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert!(shell.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();
}

// [T5-24] R7-2: DeleteChat / LeaveChat while Apple + compose mode is open preserves isolation & next_id draft
#[test]
fn t5_24_delete_chat_during_plus_compose_preserves_isolation_and_next_draft() {
    run_in_headless_subprocess(
        "t5_24_delete_chat_during_plus_compose_preserves_isolation_and_next_draft",
    );
}

async fn t5_24_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::chat_sidebar::ComposeRecipientChip;

    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part A: Seed 102 with "secret_102_draft", select 101 with draft/reply/att/kb, open + compose,
    // delete 101 via SidebarContextAction::DeleteChat(101), then Esc-cancel +.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.composer.update(cx, |comp, _| comp.set_text("secret_102_draft"));
            shell.flush_pending_draft_for_chat(102, cx);

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| {
                comp.set_text("deleted_101_draft");
                comp.begin_reply(501, "Alice".into(), "msg 101".into());
                comp.editor.attachments.push(PathBuf::from("/tmp/del_101.png"));
                comp.set_bot_reply_keyboard(
                    vec![vec![BotKeyboardButton::new("K", BotKeyboardButtonKind::Default)]],
                    kb_flags::RESIZE | kb_flags::PERSISTENT,
                    None,
                );
            });
            shell.flush_pending_draft_for_chat(CID_FABUSHI, cx);

            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.composer.read(cx).is_compose_isolated);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.delete_dialog(CID_FABUSHI));
            shell.handle_sidebar_context_action(SidebarContextAction::DeleteChat(CID_FABUSHI), cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert!(shell.sidebar.read(cx).is_plus_menu_open);
            {
                let comp = shell.composer.read(cx);
                assert!(comp.is_compose_isolated, "must remain compose isolated");
                assert!(
                    comp.text().is_empty(),
                    "next_id draft must NOT leak into + compose composer, got {:?}",
                    comp.text()
                );
                assert!(comp.autocomplete.all_mention_candidates.is_empty());
                assert!(comp.send_as_options.is_empty());
            }

            // Cancel + compose via Esc cascade
            while shell.sidebar.read(cx).is_plus_menu_open {
                assert!(shell.handle_non_char_key("escape", false, cx));
            }
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            let comp = shell.composer.read(cx);
            assert!(!comp.is_compose_isolated);
            assert_eq!(comp.text(), "secret_102_draft");
            assert!(comp.replying_to.is_none());
            assert!(comp.editor.attachments.is_empty());
            assert!(!comp.bot_keyboard.has_markup());
        })
        .unwrap();

    // Part B: ConfirmLeaveOrDeleteChat while + compose has selected recipients and typed text
    shell
        .update(&mut app.clone(), |shell, cx| {
            let c103 = ChatSummary {
                chat_id: 103,
                title: "Group C".into(),
                chat_type: 1,
                last_message: None,
                unread_count: 0,
                is_group: Some(true),
                is_bot: Some(false),
                avatar_color: None,
                updated_at: Some(1_700_000_002_000),
            };
            let stage_e = shell.stage.clone();
            let insp_e = shell.inspector.clone();
            shell.sidebar.update(cx, |sb, sb_cx| {
                let mut existing: Vec<ChatSummary> = sb
                    .dialogs
                    .iter()
                    .map(|d| ChatSummary {
                        chat_id: d.id,
                        title: d.peer_name.clone(),
                        chat_type: 1,
                        last_message: Some(d.last_message.clone()),
                        unread_count: d.unread_count,
                        is_group: Some(true),
                        is_bot: Some(false),
                        avatar_color: Some(d.avatar_color),
                        updated_at: Some(1_700_000_004_000),
                    })
                    .collect();
                existing.push(c103);
                stage_e.update(sb_cx, |st, st_cx| {
                    insp_e.update(st_cx, |insp, _| {
                        apply_authoritative_chat_list(sb, st, insp, &existing);
                    });
                });
                sb.select_dialog(103);
            });
            shell.sync_selected_chat_inspector_and_stage(103, cx);
            shell.composer.update(cx, |comp, _| comp.set_text("secret_103_draft"));
            shell.flush_pending_draft_for_chat(103, cx);

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);

            assert!(shell.handle_shortcut_key("n", false, cx));
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.compose_draft.selected_recipients.push(ComposeRecipientChip {
                    user_id: 9001,
                    chat_id: None,
                    username: Some("alice".into()),
                    display_name: "Alice".into(),
                    initials: "AL".into(),
                    is_bot: false,
                    bot_id: None,
                    avatar_color: 0x0A84FF,
                    avatar_shape: None,
                });
                sb_cx.notify();
            });
            shell.composer.update(cx, |comp, _| comp.set_text("compose_to_new_peer"));
            shell.handle_modal_outcome(ModalActionOutcome::ConfirmLeaveOrDeleteChat, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(103));
            let comp = shell.composer.read(cx);
            assert!(comp.is_compose_isolated);
            assert_eq!(
                comp.text(),
                "compose_to_new_peer",
                "typed + compose text must NOT be clobbered by next_id draft"
            );
            assert_eq!(
                shell.session_sync.last_draft_text_by_chat.get(&103).map(String::as_str),
                Some("secret_103_draft")
            );
        })
        .unwrap();

    // Part C: Sidebar click exit regression check — clicking 103 from + compose must load 103's draft
    shell
        .update(&mut app.clone(), |shell, cx| {
            while shell.sidebar.read(cx).is_plus_menu_open {
                assert!(shell.handle_non_char_key("escape", false, cx));
            }
            let c104 = ChatSummary {
                chat_id: 104,
                title: "Group D".into(),
                chat_type: 1,
                last_message: None,
                unread_count: 0,
                is_group: Some(true),
                is_bot: Some(false),
                avatar_color: None,
                updated_at: Some(1_700_000_003_000),
            };
            let stage_e = shell.stage.clone();
            let insp_e = shell.inspector.clone();
            shell.sidebar.update(cx, |sb, sb_cx| {
                let mut existing: Vec<ChatSummary> = sb
                    .dialogs
                    .iter()
                    .map(|d| ChatSummary {
                        chat_id: d.id,
                        title: d.peer_name.clone(),
                        chat_type: 1,
                        last_message: Some(d.last_message.clone()),
                        unread_count: d.unread_count,
                        is_group: Some(true),
                        is_bot: Some(false),
                        avatar_color: Some(d.avatar_color),
                        updated_at: Some(1_700_000_002_000),
                    })
                    .collect();
                existing.push(c104);
                stage_e.update(sb_cx, |st, st_cx| {
                    insp_e.update(st_cx, |insp, _| {
                        apply_authoritative_chat_list(sb, st, insp, &existing);
                    });
                });
                sb.select_dialog(104);
            });
            shell.sync_selected_chat_inspector_and_stage(104, cx);
            assert!(shell.handle_shortcut_key("n", false, cx));
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, sb_cx| {
                sb.select_dialog_from_user(103);
                sb_cx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(103));
            let comp = shell.composer.read(cx);
            assert!(!comp.is_compose_isolated);
            assert_eq!(
                comp.text(),
                "secret_103_draft",
                "switching out of + compose via sidebar click must load 103's saved draft"
            );
        })
        .unwrap();
}

// [T5-25] Sibling compose-isolation exit in open_remote_computers_hub & Escape popover tightening
#[test]
fn t5_25_remote_computers_hub_and_escape_popover_exit_compose_isolation_sync() {
    run_in_headless_subprocess(
        "t5_25_remote_computers_hub_and_escape_popover_exit_compose_isolation_sync",
    );
}

async fn t5_25_body(app: AsyncApp) {
    let client = new_isolated_client("http://127.0.0.1:1".to_string());
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // 1. Same-turn Cmd+N -> MainMenuCommand::RemoteComputer (open_remote_computers_hub)
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.composer.update(cx, |comp, _| comp.set_text("keep_101_draft"));

            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.composer.read(cx).is_compose_isolated);
            assert!(shell.composer.read(cx).text().is_empty());

            shell.handle_main_menu_command(MainMenuCommand::RemoteComputer, cx);
            assert!(
                !shell.sidebar.read(cx).is_plus_menu_open,
                "open_remote_computers_hub must exit + compose mode"
            );
            assert!(
                !shell.composer.read(cx).is_compose_isolated,
                "open_remote_computers_hub must synchronously clear composer.is_compose_isolated"
            );
            assert_eq!(shell.composer.read(cx).text(), "keep_101_draft");
            assert_eq!(shell.active_panel, ActiveWorkspacePanel::Settings);
            assert!(shell.settings_router.read(cx).standalone_computers_mode);

            // Escape unwinds standalone Remote Computers hub back to Chats
            assert!(shell.handle_non_char_key_with_alt("escape", false, false, cx));
            assert_eq!(shell.active_panel, ActiveWorkspacePanel::Chats);
            assert_eq!(shell.nav_rail.read(cx).active_destination, NavDestination::Chats);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(35))
        .await;

    // 2. Escape when account popover is open with is_compose_isolated == true (even when is_plus_menu_open == false)
    // exits isolation synchronously
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert_eq!(shell.composer.read(cx).text(), "keep_101_draft");
            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.composer.read(cx).is_compose_isolated);
            shell.sidebar.update(cx, |sb, _| {
                sb.is_plus_menu_open = false;
                sb.is_account_popover_open = true;
            });
            assert!(shell.handle_non_char_key_with_alt("escape", false, false, cx));
            assert!(!shell.sidebar.read(cx).is_account_popover_open);
            assert!(
                !shell.composer.read(cx).is_compose_isolated,
                "Escape on account popover must synchronously clear is_compose_isolated"
            );
            assert_eq!(shell.composer.read(cx).text(), "keep_101_draft");
        })
        .unwrap();

    // 3. Escape when is_plus_menu_open == true && !is_compose_isolated still runs sync_compose_state_to_composer cleanly
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| {
                sb.is_plus_menu_open = true;
            });
            shell.composer.update(cx, |c, _| {
                c.set_compose_send_disabled(true);
            });
            assert!(!shell.composer.read(cx).is_compose_isolated);
            assert!(shell.composer.read(cx).compose_send_disabled);
            assert!(shell.handle_non_char_key_with_alt("escape", false, false, cx));
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            assert!(!shell.composer.read(cx).compose_send_disabled);
        })
        .unwrap();
}

// [T5-26] R8-1 & R8-2:
// - persist_bot_computer_profile when target_cid == None drops inspector subpage/profile mutation
//   and toast if user switched to another chat, and uses show_non_chat_toast when staying at None.
// - open_sender_dm_by_user_id Err path drops toast if user switched chats, and uses show_chat_toast
//   if user remained in the originating chat.
#[test]
fn t5_26_persist_bot_computer_none_chat_and_open_sender_dm_err_toast_scoping() {
    run_in_headless_subprocess(
        "t5_26_persist_bot_computer_none_chat_and_open_sender_dm_err_toast_scoping",
    );
}

async fn t5_26_body(app: AsyncApp) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let put_hits = Arc::new(AtomicUsize::new(0));
    let create_hits = Arc::new(AtomicUsize::new(0));
    let put_hits_srv = Arc::clone(&put_hits);
    let create_hits_srv = Arc::clone(&create_hits);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let put_hits_srv = Arc::clone(&put_hits_srv);
            let create_hits_srv = Arc::clone(&create_hits_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/api/v1/bots/9001/computer") {
                    put_hits_srv.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(80));
                    let body = serde_json::to_string(&BotComputerProfileDto {
                        bot_id: 9001,
                        bound_node_id: None,
                        persona_prompt: "LEAKED_PERSONA_9001".to_string(),
                        preferred_model: "gpt-4o".to_string(),
                        approval_mode: ComputerRunPolicy::default(),
                        allow_cross_device_control: true,
                        allow_cloud_self_config: false,
                        updated_at_ms: 1_700_000_010_000,
                    })
                    .unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/api/v1/chats/create") {
                    create_hits_srv.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(80));
                    let body = r#"{"error":"simulated create_chat failure"}"#;
                    let resp = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part A1 (R8-1): SaveBotComputerConfig when target_cid == None, then switch to Chat 102
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.stage.read(cx).active_chat_id.is_none());
            assert!(shell.sidebar.read(cx).active_chat_id.is_none());
            shell.handle_inspector_action(
                InspectorAction::SaveBotComputerConfig {
                    bot_id: 9001,
                    persona_prompt: "LEAKED_PERSONA_9001".to_string(),
                    preferred_model: "gpt-4o".to_string(),
                    approval_mode: ComputerRunPolicy::default(),
                    allow_cross_device_control: true,
                    allow_cloud_self_config: false,
                },
                cx,
            );

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.inspector.update(cx, |insp, icx| {
                insp.sync_bot_computer_profile(None);
                insp.subpage = InspectorSubpage::BotConfigEditor;
                icx.notify();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(put_hits.load(Ordering::SeqCst), 1);
            let insp = shell.inspector.read(cx);
            assert_eq!(insp.subpage, InspectorSubpage::BotConfigEditor);
            assert!(insp.bot_computer_profile.is_none());
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Part A2 (R8-1): Staying at active_chat == None emits non-chat toast (not auto-scoped onto 102)
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.stage.update(cx, |st, _| st.clear_selection());
            shell.sidebar.update(cx, |sb, _| sb.active_chat_id = None);
            shell.handle_inspector_action(
                InspectorAction::SaveBotComputerConfig {
                    bot_id: 9001,
                    persona_prompt: "VALID_NONE_CHAT".to_string(),
                    preferred_model: "gpt-4o".to_string(),
                    approval_mode: ComputerRunPolicy::default(),
                    allow_cross_device_control: true,
                    allow_cloud_self_config: false,
                },
                cx,
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected non-chat toast when staying at None");
            assert_eq!(toast.chat_id, None);
            assert!(!toast.allow_chat_auto_scope);

            // Selecting Chat 102 afterwards must not stamp the non-chat toast with Chat 102
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.schedule_current_toast_dismiss(cx);
            let toast_after = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("non-chat toast still visible until dismissed");
            assert_eq!(toast_after.chat_id, None);
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    // Part B1 (R8-2): open_sender_dm_by_user_id on Chat 101, switch to Chat 102 before HTTP 500 -> no toast leak
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.open_sender_dm_by_user_id(888_001, "Alice".to_string(), cx);

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert_eq!(create_hits.load(Ordering::SeqCst), 1);
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Part B2 (R8-2): Staying on Chat 101 emits chat-scoped toast for Chat 101
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.open_sender_dm_by_user_id(888_002, "Bob".to_string(), cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert_eq!(create_hits.load(Ordering::SeqCst), 2);
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected chat-scoped error toast when staying on Chat 101");
            assert_eq!(toast.chat_id, Some(CID_FABUSHI));
        })
        .unwrap();
}

// [T5-27] R8-3 & R8-6:
// - build_local_fabushi_error_card classifies `"Request rejected: Missing API key in Authorization header"`
//   (needs_provider_cta = false) as provider request error (no CTA), while preserving bare `"missing API key"`.
// - ToggleAttachMenuBot(None) & SubmitCaptchaClicks(None) emit non-chat toasts (`allow_chat_auto_scope == false`).
// - launch_miniapp_with_url with delayed 404 + switch to Chat 102 drops error toast.
// - Deleting last chat while + compose is open + attaching files + pressing Escape clears all attachments & text.
#[test]
fn t5_27_provider_missing_key_request_rejection_and_r8_6_non_chat_and_escape_cleanup() {
    run_in_headless_subprocess(
        "t5_27_provider_missing_key_request_rejection_and_r8_6_non_chat_and_escape_cleanup",
    );
}

async fn t5_27_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::stage::cards::InlineButtonType;
    use std::io::{Read, Write};

    // Part A (R8-3): Provider request rejection mentioning "Missing API key in Authorization header"
    let rejected_err =
        "Internal error: PROVIDER_ERROR: Request rejected: Missing API key in Authorization header";
    let (card_zh, kb_zh, _) = AppShell::build_local_fabushi_error_card(rejected_err, true, false);
    assert!(card_zh.contains("AI 服务请求失败"), "got {card_zh}");
    assert!(kb_zh.is_none(), "request rejection must not attach configure CTA");

    let (card_en, kb_en, _) = AppShell::build_local_fabushi_error_card(rejected_err, false, false);
    assert!(card_en.contains("AI Provider Request Failed"), "got {card_en}");
    assert!(kb_en.is_none(), "request rejection must not attach configure CTA");

    // Backward compatibility: bare missing API key & needs_provider_cta == true both retain CTA
    let (_, kb_bare, _) =
        AppShell::build_local_fabushi_error_card("PROVIDER_ERROR: missing API key\nline2", true, false);
    assert!(kb_bare.is_some());
    let (_, kb_cta, _) = AppShell::build_local_fabushi_error_card(rejected_err, true, true);
    assert_eq!(
        kb_cta.as_ref().unwrap()[0][0].button_type,
        InlineButtonType::Callback("hermes:open_ai_providers".to_string())
    );

    // Part B (R8-6): Async miniapp launch 404 + non-chat toasts + last-chat deletion Escape attachment clear
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/launch") {
                    std::thread::sleep(Duration::from_millis(80));
                    let body = r#"{"error":"miniapp not found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // B1: ToggleAttachMenuBot with active_chat == None and SubmitCaptchaClicks with cid_u64 == None
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.stage.read(cx).active_chat_id.is_none());
            shell.handle_inspector_action(InspectorAction::ToggleAttachMenuBot(999_999), cx);
            let t1 = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("ToggleAttachMenuBot fallback toast");
            assert_eq!(t1.chat_id, None);
            assert!(!t1.allow_chat_auto_scope);
            shell.window_runtime.controller.clear_all_toasts();

            shell.handle_bot_assist_card_action(
                BotAssistCardAction::SubmitCaptchaClicks {
                    card_id: "missing-captcha-card".to_string(),
                },
                cx,
            );
            let t2 = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("SubmitCaptchaClicks fallback toast");
            assert_eq!(t2.chat_id, None);
            assert!(!t2.allow_chat_auto_scope);
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    // B2: launch_miniapp_with_url (source_chat_id = Some(101)) -> switch to 102 before 404 settles
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.launch_miniapp_with_url(
                "fabushi_bot",
                "fabushi",
                Some(CID_FABUSHI),
                MiniAppLaunchMode::DirectLink,
                None,
                None,
                cx,
            );

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Mini App launch failure toast leaked onto switched Chat 102: {:?}",
                shell.window_runtime.controller.current_toast
            );
        })
        .unwrap();

    // B3: Delete last remaining chat while + compose is open, attach files, press Escape -> clears all attachments & text
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.handle_modal_outcome(ModalActionOutcome::ConfirmLeaveOrDeleteChat, cx);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
            assert!(shell.handle_shortcut_key("n", false, cx));
            assert!(shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.composer.read(cx).is_compose_isolated);

            // Delete the last remaining chat (CID_FABUSHI) while + compose is active
            shell.handle_modal_outcome(ModalActionOutcome::ConfirmLeaveOrDeleteChat, cx);
            assert!(shell.sidebar.read(cx).dialogs.is_empty());
            assert!(shell.stage.read(cx).active_chat_id.is_none());

            // Attach files & set text during + compose
            shell.composer.update(cx, |comp, ccx| {
                comp.set_text("dangling compose text");
                comp.editor.attachments.push(PathBuf::from("/tmp/orphan_compose.png"));
                comp.submitted_attachment = Some("/tmp/orphan_compose.png".to_string());
                comp.pending_attached_files = Some(vec![PathBuf::from("/tmp/orphan_compose.png")]);
                ccx.notify();
            });

            for _ in 0..4 {
                if !shell.sidebar.read(cx).is_plus_menu_open {
                    break;
                }
                assert!(shell.handle_non_char_key_with_alt("escape", false, false, cx));
            }
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            let comp = shell.composer.read(cx);
            assert!(!comp.is_compose_isolated);
            assert!(comp.text().is_empty());
            assert!(comp.editor.attachments.is_empty());
            assert!(comp.submitted_attachment.is_none());
            assert!(comp.pending_attached_files.is_none());
        })
        .unwrap();
}

// [T5-28] R8-4: disconnect_all_remote_control uses computer_fence across logout/account switch
// and emits non-chat toast (`chat_id == None && !allow_chat_auto_scope`).
#[test]
fn t5_28_disconnect_all_remote_control_auth_fence_and_non_chat_toast() {
    run_in_headless_subprocess("t5_28_disconnect_all_remote_control_auth_fence_and_non_chat_toast");
}

async fn t5_28_body(app: AsyncApp) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let disconnect_hits = Arc::new(AtomicUsize::new(0));
    let disconnect_hits_srv = Arc::clone(&disconnect_hits);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let disconnect_hits_srv = Arc::clone(&disconnect_hits_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/api/v1/remote/sessions/disconnect-all") {
                    disconnect_hits_srv.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(100));
                    let body = r#"{"ok":true}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part 1: User A triggers disconnect_all_remote_control, then logs out & User B logs in at 25ms
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.disconnect_all_remote_control(cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || disconnect_hits.load(Ordering::SeqCst) >= 1).await,
        "User A disconnect-all request did not arrive at mock server before logout"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: 102,
                    peer_name: "Engineering Group".into(),
                    avatar_initials: "EG".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: false,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(102);
            });
            shell.sync_selected_chat_inspector_and_stage(102, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(150))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert_eq!(disconnect_hits.load(Ordering::SeqCst), 1);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "User A's disconnect-all toast leaked into User B session: {:?}",
                shell.window_runtime.controller.current_toast
            );
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // Part 2: User B calls disconnect_all_remote_control with active_chat_id == None -> non-chat toast
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.stage.update(cx, |st, _| st.clear_selection());
            shell.sidebar.update(cx, |sb, _| sb.active_chat_id = None);
            shell.disconnect_all_remote_control(cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected non-chat disconnect-all toast for User B");
            assert_eq!(toast.chat_id, None);
            assert!(!toast.allow_chat_auto_scope);

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.schedule_current_toast_dismiss(cx);
            let toast_after = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("non-chat toast remains unpinned");
            assert_eq!(toast_after.chat_id, None);
        })
        .unwrap();
}

// [T5-30] GAP-R9-1: 260ms delayed RefreshScreen after SendGuiClick must fence on both
// computer_fence (cross-account logout/login) and exact Entity<ComputerWorkbench> identity.
#[test]
fn t5_30_gap_r9_1_delayed_refresh_screen_cross_account_and_overlay_switch() {
    run_in_headless_subprocess(
        "t5_30_gap_r9_1_delayed_refresh_screen_cross_account_and_overlay_switch",
    );
}

async fn t5_30_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::computer_workbench::WorkbenchCommand;
    use gpui_telegram_proto::computer_contracts::RemoteComputerCommandResult;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    let recorded_execs: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_execs_srv = Arc::clone(&recorded_execs);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let recorded_execs_srv = Arc::clone(&recorded_execs_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let auth_hdr = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .to_string();
                let body_part = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();

                if first_line.contains("/api/v1/computers/exec") {
                    recorded_execs_srv
                        .lock()
                        .unwrap()
                        .push((auth_hdr, body_part));
                    let ok_json = serde_json::to_string(&RemoteComputerCommandResult {
                        command_id: "cmd-1".into(),
                        node_id: "cloud:node".into(),
                        ok: true,
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: "test".into(),
                        duration_ms: 1,
                    })
                    .unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        ok_json.len(),
                        ok_json
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| {
            let s = cx.new(|cx| AppShell::new_with_client(client, cx));
            s.update(cx, |shell, cx| {
                shell.apply_authenticated_session_with_vault(
                    AuthResponse {
                        user_id: 1001,
                        username: "user_a".into(),
                        session_id: "sess-a".into(),
                        token: "tok-user-a".into(),
                        first_name: None,
                        created_at: None,
                        expires_at: None,
                    },
                    None,
                    cx,
                );
                shell.open_remote_workbench_for_node("cloud:1001:user-a".into(), cx);
                let ov_a = shell
                    .overlays
                    .computer_overlay
                    .clone()
                    .expect("overlay A opened");
                shell.handle_workbench_command(
                    "cloud:1001:user-a".into(),
                    WorkbenchCommand::SendGuiClick { x: 10, y: 10 },
                    ov_a,
                    cx,
                );
            });
            s
        })
        .unwrap();

    // Wait 80ms (< 260ms RefreshScreen timer), then logout & login User B with new overlay
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 2002,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-user-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.open_remote_workbench_for_node("cloud:2002:user-b".into(), cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(350))
        .await;

    {
        let log = recorded_execs.lock().unwrap();
        assert!(
            !log.iter()
                .any(|(auth, body)| auth.contains("tok-user-b") && body.contains("cloud:1001:user-a")),
            "GAP-R9-1 cross-account RefreshScreen fired User A node under User B token: {log:?}"
        );
    }

    // Part 2: Same-user overlay switch within 260ms must not fire RefreshScreen on stale node
    recorded_execs.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |shell, cx| {
            let ov_b = shell
                .overlays
                .computer_overlay
                .clone()
                .expect("overlay B opened");
            shell.handle_workbench_command(
                "cloud:2002:user-b".into(),
                WorkbenchCommand::SendGuiClick { x: 12, y: 12 },
                ov_b,
                cx,
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.open_remote_workbench_for_node("cloud:2002:other".into(), cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(10))
        .await;
    recorded_execs.lock().unwrap().clear();

    app.background_executor()
        .timer(Duration::from_millis(350))
        .await;

    {
        let log = recorded_execs.lock().unwrap();
        assert!(
            !log.iter()
                .any(|(_, body)| body.contains("cloud:2002:user-b") && body.contains("capture_screen")),
            "GAP-R9-1 stale overlay RefreshScreen fired after switching workbench overlay: {log:?}"
        );
    }
}

// [T5-31] TOGGLE_ALLOW_REMOTE & SetAssistAllow roll back optimistic allow_assist and emit
// non-chat error toast on HTTP 500.
#[test]
fn t5_31_toggle_allow_remote_and_set_assist_allow_rollback_and_toast() {
    run_in_headless_subprocess(
        "t5_31_toggle_allow_remote_and_set_assist_allow_rollback_and_toast",
    );
}

async fn t5_31_body(app: AsyncApp) {
    use gpui_telegram_desktop::platform::mac::tray_mac::tray_action_ids;
    use gpui_telegram_desktop::views::settings::sections::SettingsComputersEvent;
    use gpui_telegram_proto::remote_control_contracts::RemoteAssistConfigDto;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let allow_hits = Arc::new(AtomicUsize::new(0));
    let allow_hits_srv = Arc::clone(&allow_hits);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let allow_hits_srv = Arc::clone(&allow_hits_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/api/v1/remote/assist/allow") {
                    allow_hits_srv.fetch_add(1, Ordering::SeqCst);
                    let body = r#"{"error":"simulated assist allow 500"}"#;
                    let resp = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") || first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part 1: TOGGLE_ALLOW_REMOTE tray action rolls back on HTTP 500 and emits non-chat toast
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.computers.assist_config = Some(RemoteAssistConfigDto {
                assist_id: "482910375".into(),
                allow_assist: true,
                ..Default::default()
            });
            shell.sync_computers_to_views(cx);
            shell.handle_tray_action(tray_action_ids::TOGGLE_ALLOW_REMOTE, cx);
            assert!(
                !shell
                    .computers
                    .assist_config
                    .as_ref()
                    .unwrap()
                    .allow_assist,
                "optimistic toggle should flip allow_assist to false"
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert_eq!(allow_hits.load(Ordering::SeqCst), 1);
            assert!(
                shell
                    .computers
                    .assist_config
                    .as_ref()
                    .unwrap()
                    .allow_assist,
                "allow_assist must roll back to true on HTTP 500"
            );
            assert!(
                shell.remote_host.tray_mac.allow_remote_control,
                "tray_mac.allow_remote_control must roll back to true"
            );
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected non-chat error toast on TOGGLE_ALLOW_REMOTE 500");
            assert_eq!(toast.chat_id, None);
            assert!(!toast.allow_chat_auto_scope);
            shell.window_runtime.controller.clear_all_toasts();

            // Part 2: SettingsComputersEvent::SetAssistAllow(false) syncs Settings rollback + non-chat toast
            let sc = shell
                .settings_router
                .update(cx, |sr, scx| sr.ensure_computers(scx));
            sc.update(cx, |view, _| {
                if let Some(ref mut cfg) = view.assist_config {
                    cfg.allow_assist = false;
                }
            });
            shell.handle_computer_settings_event(&SettingsComputersEvent::SetAssistAllow(false), cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(allow_hits.load(Ordering::SeqCst), 2);
            let sc = shell
                .settings_router
                .read(cx)
                .settings_computers
                .as_ref()
                .expect("settings_computers initialized");
            assert!(
                sc.read(cx)
                    .assist_config
                    .as_ref()
                    .unwrap()
                    .allow_assist,
                "settings_computers.assist_config must roll back to true via sync_computers_to_views"
            );
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected non-chat error toast on SetAssistAllow 500");
            assert_eq!(toast.chat_id, None);
            assert!(!toast.allow_chat_auto_scope);
        })
        .unwrap();
}

// [T5-32] GAP-R9-2: upload_and_send_paths runs tokio::fs::read + upload_media + send_message_durable
// on client.runtime() (even with NO Tokio EnterGuard on the GPUI thread) and fences subsequent files
// across logout / account switch.
#[test]
fn t5_32_gap_r9_2_upload_and_send_paths_without_enter_guard_and_session_fence() {
    run_in_headless_subprocess(
        "t5_32_gap_r9_2_upload_and_send_paths_without_enter_guard_and_session_fence",
    );
}

async fn t5_32_body(app: AsyncApp) {
    use gpui_telegram_proto::contracts::media::MediaUploadCompleteResponse;
    use gpui_telegram_proto::contracts::messages::SendMessageHttpResponse;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    let uploads: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sends: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let uploads_srv = Arc::clone(&uploads);
    let sends_srv = Arc::clone(&sends);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let uploads_srv = Arc::clone(&uploads_srv);
            let sends_srv = Arc::clone(&sends_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                let auth_hdr = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .to_string();

                if first_line.contains("/api/v1/media/upload") {
                    if req.contains("file_1_delay") {
                        std::thread::sleep(Duration::from_millis(120));
                    }
                    uploads_srv.lock().unwrap().push((auth_hdr, req.clone()));
                    let body = serde_json::to_string(&MediaUploadCompleteResponse {
                        file_id: "file_mock_1".into(),
                        sha256: "deadbeef".into(),
                        size_bytes: 12,
                        mime_type: "application/octet-stream".into(),
                        filename: Some("attachment.txt".into()),
                        width: None,
                        height: None,
                        duration_seconds: None,
                        waveform_b64: None,
                        waveform_bars: None,
                        thumbnail_data_url: None,
                    })
                    .unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/api/v1/messages/send") {
                    let body_part = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                    sends_srv.lock().unwrap().push((auth_hdr, body_part));
                    let body = serde_json::to_string(&SendMessageHttpResponse {
                        message_id: 9001,
                        id: 9001,
                        pts: 10,
                        pts_count: 1,
                        duplicate: false,
                        date_ms: 1_700_000_010_000,
                        date: "2026-11-23T00:00:00Z".into(),
                        sender_id: UID_VIEWER,
                        chat_id: CID_FABUSHI as i64,
                        text: "attachment".into(),
                        client_nonce: None,
                        reply_to: None,
                        status: "sent".into(),
                    })
                    .unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") || first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let temp_file_a = tmp_dir.path().join("temp_file_a.txt");
    let file_1_delay = tmp_dir.path().join("file_1_delay.txt");
    let file_2_secret = tmp_dir.path().join("file_2_secret.txt");
    std::fs::write(&temp_file_a, b"hello temp_file_a").unwrap();
    std::fs::write(&file_1_delay, b"file_1_delay payload").unwrap();
    std::fs::write(&file_2_secret, b"file_2_secret payload").unwrap();

    // CRITICAL: Explicitly OMIT `let _rt = client.runtime().enter();` to prove zero Tokio EnterGuard panic!
    let client = new_isolated_client(format!("http://{addr}"));
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part 1: Single file upload & send without EnterGuard
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.upload_and_send_paths(vec![temp_file_a.clone()], cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    {
        let u = uploads.lock().unwrap();
        let s = sends.lock().unwrap();
        assert!(
            u.iter().any(|(_, b)| b.contains("temp_file_a.txt")),
            "expected upload for temp_file_a.txt without EnterGuard: {u:?}"
        );
        assert!(
            s.iter().any(|(_, b)| b.contains("temp_file_a.txt")),
            "expected send for temp_file_a.txt without EnterGuard: {s:?}"
        );
    }

    // Part 2: Multi-file upload fenced across logout + User B login
    uploads.lock().unwrap().clear();
    sends.lock().unwrap().clear();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.upload_and_send_paths(vec![file_1_delay.clone(), file_2_secret.clone()], cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(30))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-user-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(250))
        .await;

    {
        let u = uploads.lock().unwrap();
        let s = sends.lock().unwrap();
        assert!(
            !u.iter().any(|(_, b)| b.contains("file_2_secret.txt")),
            "second file must not upload after logout/user switch: {u:?}"
        );
        assert!(
            !s.iter().any(|(_, b)| b.contains("file_2_secret.txt")),
            "second file must not send after logout/user switch: {s:?}"
        );
        assert!(
            !u.iter().any(|(auth, _)| auth.contains("tok-user-b"))
                && !s.iter().any(|(auth, _)| auth.contains("tok-user-b")),
            "User A attachment upload/send must never run under User B token"
        );
    }
}

// [T5-33] Cancelled `+` compose DM/Group `create_chat` failure (and direct `exit_compose_mode` + `Ok`)
// must not reopen `+` compose or wipe active chat / draft.
#[test]
fn t5_33_cancelled_plus_compose_create_chat_guards() {
    run_in_headless_subprocess("t5_33_cancelled_plus_compose_create_chat_guards");
}

async fn t5_33_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::chat_sidebar::ComposeRecipientChip;
    use gpui_telegram_proto::contracts::chats::ChatDetailDto;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let create_hits = Arc::new(AtomicUsize::new(0));
    let create_hits_srv = Arc::clone(&create_hits);
    let create_done = Arc::new(AtomicUsize::new(0));
    let create_done_srv = Arc::clone(&create_done);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let create_hits_srv = Arc::clone(&create_hits_srv);
            let create_done_srv = Arc::clone(&create_done_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/api/v1/chats/create") {
                    create_hits_srv.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(80));
                    if req.contains("OK_200") {
                        let dto = ChatDetailDto {
                            chat_id: 777,
                            title: "CreatedOk".into(),
                            kind: "private".into(),
                            chat_type: 0,
                            creator_id: UID_VIEWER,
                            username: Some("alice_new".into()),
                            about: None,
                            is_group: false,
                            is_channel: false,
                            is_bot: false,
                            is_forum: false,
                            is_archived: false,
                            is_pinned: false,
                            is_muted: false,
                            mute_until_ms: None,
                            pts: 1,
                            last_message_id: 0,
                            last_message_date_ms: 0,
                            unread_count: 0,
                            read_inbox_max_id: 0,
                            read_outbox_max_id: 0,
                            member_count: 2,
                            pinned_message_id: None,
                            draft: None,
                        };
                        let body = serde_json::to_string(&dto).unwrap();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                        let _ = stream.flush();
                    } else {
                        let body = r#"{"error":"simulated create_chat 500"}"#;
                        let resp = format!(
                            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                        let _ = stream.flush();
                    }
                    create_done_srv.fetch_add(1, Ordering::SeqCst);
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    let chip1 = ComposeRecipientChip {
        user_id: 0,
        username: Some("alice_new".into()),
        display_name: "Alice New".into(),
        initials: "AN".into(),
        avatar_color: 0x4A90E2,
        avatar_shape: None,
        is_bot: false,
        bot_id: None,
        chat_id: None,
    };
    let chip2 = ComposeRecipientChip {
        user_id: 0,
        username: Some("bob_new".into()),
        display_name: "Bob New".into(),
        initials: "BN".into(),
        avatar_color: 0x50E3C2,
        avatar_shape: None,
        is_bot: false,
        bot_id: None,
        chat_id: None,
    };

    // Sub-case A (DM Escape + HTTP 500)
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert!(shell.handle_shortcut_key("n", false, cx));
            shell.sidebar.update(cx, |sb, _| {
                sb.compose_draft.selected_recipients.push(chip1.clone());
            });
            shell.send_rich_text_options("FAIL_500 dm".into(), false, None, cx);
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_some());

            for _ in 0..4 {
                if !shell.sidebar.read(cx).is_plus_menu_open {
                    break;
                }
                assert!(shell.handle_non_char_key_with_alt("escape", false, false, cx));
            }
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_none());

            shell.sidebar.update(cx, |sb, _| sb.select_dialog_from_user(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.composer.update(cx, |c, _| c.set_text("draft_in_102"));
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || create_done.load(Ordering::SeqCst) >= 1).await,
        "t5_33 Sub-case A create_chat server response timeout"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(!sb.is_plus_menu_open);
            assert!(sb.compose_draft.creation_error.is_none());
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert_eq!(shell.composer.read(cx).text(), "draft_in_102");
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "unexpected toast in t5_33 Sub-case A: {:?}",
                shell.window_runtime.controller.current_toast
            );
        })
        .unwrap();

    // Sub-case B (Group Escape + HTTP 500)
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.handle_shortcut_key("n", false, cx));
            shell.sidebar.update(cx, |sb, _| {
                sb.compose_draft.selected_recipients.push(chip1.clone());
                sb.compose_draft.selected_recipients.push(chip2.clone());
            });
            shell.send_rich_text_options("FAIL_500 grp".into(), false, None, cx);
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_some());

            for _ in 0..4 {
                if !shell.sidebar.read(cx).is_plus_menu_open {
                    break;
                }
                assert!(shell.handle_non_char_key_with_alt("escape", false, false, cx));
            }
            assert!(!shell.sidebar.read(cx).is_plus_menu_open);
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_none());

            shell.sidebar.update(cx, |sb, _| sb.select_dialog_from_user(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.composer.update(cx, |c, _| c.set_text("draft_in_102"));
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || create_done.load(Ordering::SeqCst) >= 2).await,
        "t5_33 Sub-case B create_chat server response timeout"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(!sb.is_plus_menu_open);
            assert!(sb.compose_draft.creation_error.is_none());
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert_eq!(shell.composer.read(cx).text(), "draft_in_102");
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "unexpected toast in t5_33 Sub-case B: {:?}",
                shell.window_runtime.controller.current_toast
            );
        })
        .unwrap();

    // Sub-case C (Direct exit_compose_mode() + delayed Ok HTTP 200)
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.handle_shortcut_key("n", false, cx));
            shell.sidebar.update(cx, |sb, _| {
                sb.compose_draft.selected_recipients.push(chip1.clone());
            });
            shell.send_rich_text_options("OK_200 dm".into(), false, None, cx);
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_some());
            shell.sidebar.update(cx, |sb, _| sb.exit_compose_mode());
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_some());
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.composer.update(cx, |c, _| c.set_text("draft_in_102"));
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            create_done.load(Ordering::SeqCst) >= 3
                && shell
                    .read_with(&app, |s, cx| {
                        s.sidebar.read(cx).compose_draft.pending_creation.is_none()
                    })
                    .unwrap_or(false)
        })
        .await,
        "t5_33 Sub-case C pending_creation settle timeout"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(!sb.is_plus_menu_open);
            assert!(sb.compose_draft.pending_creation.is_none());
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(102));
            assert_eq!(shell.composer.read(cx).text(), "draft_in_102");
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "unexpected toast in t5_33 Sub-case C: {:?}",
                shell.window_runtime.controller.current_toast
            );
        })
        .unwrap();

    // Sub-case D (Stay in + compose + HTTP 500)
    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(shell.handle_shortcut_key("n", false, cx));
            shell.sidebar.update(cx, |sb, _| {
                sb.compose_draft.selected_recipients.push(chip1.clone());
            });
            shell.send_rich_text_options("FAIL_500 stay".into(), false, None, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            create_hits.load(Ordering::SeqCst) == 4
                && shell
                    .read_with(&app, |s, cx| {
                        s.sidebar.read(cx).compose_draft.creation_error.is_some()
                            && s.window_runtime.controller.current_toast.is_some()
                    })
                    .unwrap_or(false)
        })
        .await,
        "t5_33 Sub-case D in-compose error toast timeout"
    );

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(create_hits.load(Ordering::SeqCst), 4);
            let sb = shell.sidebar.read(cx);
            assert!(sb.is_plus_menu_open);
            assert!(sb.compose_draft.creation_error.is_some());
            assert_eq!(shell.composer.read(cx).text(), "FAIL_500 stay");
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected in-compose error toast");
            assert_eq!(toast.chat_id, None);
            assert!(!toast.allow_chat_auto_scope);
        })
        .unwrap();
}

// [T5-34] open_bot_profile_popup, dl_req, toggle_block_peer async fences & toast scoping +
// drain_create_bot_box_actions toast emitted after open_or_select_chat.
#[test]
fn t5_34_bot_profile_popup_stage_dl_block_fences_and_create_bot_toast() {
    run_in_headless_subprocess(
        "t5_34_bot_profile_popup_stage_dl_block_fences_and_create_bot_toast",
    );
}

async fn t5_34_body(app: AsyncApp) {
    use gpui_telegram_client::net::hermes_client::HermesRichChatDto;
    use gpui_telegram_proto::miniapp_contracts::{BotRegistryEntry, BotTokenResponse};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let bot_created = Arc::new(AtomicBool::new(false));
    let bot_created_srv = Arc::clone(&bot_created);
    let bot_post_done = Arc::new(AtomicBool::new(false));
    let bot_post_done_srv = Arc::clone(&bot_post_done);
    let err_responses_sent = Arc::new(AtomicUsize::new(0));
    let err_responses_sent_srv = Arc::clone(&err_responses_sent);
    let dl_responses_sent = Arc::new(AtomicUsize::new(0));
    let dl_responses_sent_srv = Arc::clone(&dl_responses_sent);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let bot_created_srv = Arc::clone(&bot_created_srv);
            let bot_post_done_srv = Arc::clone(&bot_post_done_srv);
            let err_responses_sent_srv = Arc::clone(&err_responses_sent_srv);
            let dl_responses_sent_srv = Arc::clone(&dl_responses_sent_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/api/v1/bots/profile/") {
                    std::thread::sleep(Duration::from_millis(80));
                    let body = r#"{"error":"bot not found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    err_responses_sent_srv.fetch_add(1, Ordering::SeqCst);
                } else if first_line.contains("/api/v1/media/") {
                    std::thread::sleep(Duration::from_millis(80));
                    let body = "PDFDATA";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    dl_responses_sent_srv.fetch_add(1, Ordering::SeqCst);
                } else if first_line.contains("/block") {
                    std::thread::sleep(Duration::from_millis(80));
                    let body = r#"{"error":"block failed"}"#;
                    let resp = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    err_responses_sent_srv.fetch_add(1, Ordering::SeqCst);
                } else if first_line.starts_with("POST /api/v1/bots ") {
                    bot_created_srv.store(true, Ordering::SeqCst);
                    let bot_resp = BotTokenResponse {
                        bot: BotRegistryEntry {
                            bot_user_id: 777_555,
                            owner_user_id: 202,
                            username: "my_test_bot".into(),
                            display_name: "MyTestBot".into(),
                            description: "desc".into(),
                            about: "about".into(),
                            ai_persona: "persona".into(),
                            is_fabushi_bot: true,
                            ..Default::default()
                        },
                        bot_token: "bot_tok".into(),
                        companion_chat_id: Some(555),
                    };
                    let body = serde_json::to_string(&bot_resp).unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    bot_post_done_srv.store(true, Ordering::SeqCst);
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = if bot_created_srv.load(Ordering::SeqCst) {
                        r#"[{"chat_id":555,"title":"MyTestBot","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000006000},{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#
                    } else {
                        r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // 1. open_bot_profile_popup: switch to Chat 102 before 404 returns -> no toast
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.open_bot_profile_popup("unknown_bot", cx);
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || err_responses_sent.load(Ordering::SeqCst) >= 1).await,
        "t5_34 bot profile 404 response timeout"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "unexpected toast in t5_34 open_bot_profile_popup: {:?}",
                shell.window_runtime.controller.current_toast
            );
            // Stay on Chat 101 -> emits chat-scoped toast for Chat 101
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.open_bot_profile_popup("unknown_bot", cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            err_responses_sent.load(Ordering::SeqCst) >= 2
                && shell
                    .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                    .unwrap_or(false)
        })
        .await,
        "t5_34 expected chat-scoped bot profile toast timeout"
    );

    shell
        .update(&mut app.clone(), |shell, _cx| {
            let t = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected chat-scoped bot profile error toast");
            assert_eq!(t.chat_id, Some(CID_FABUSHI));
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    // 2. dl_req: switch to Chat 102 before 200 returns -> no toast
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.stage.update(cx, |st, _| {
                st.pending_download_url = Some(("file_99".into(), "doc.pdf".into()));
            });
            shell.handle_stage_pending_actions(cx);
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || dl_responses_sent.load(Ordering::SeqCst) >= 1).await,
        "t5_34 dl_req 1 response timeout"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "unexpected toast in t5_34 dl_req: {:?}",
                shell.window_runtime.controller.current_toast
            );
            // dl_req -> logout at 20ms -> no toast
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.stage.update(cx, |st, _| {
                st.pending_download_url = Some(("file_99".into(), "doc.pdf".into()));
            });
            shell.handle_stage_pending_actions(cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || dl_responses_sent.load(Ordering::SeqCst) >= 2).await,
        "t5_34 dl_req 2 response timeout"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(shell.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();

    // 3. toggle_block_peer: logout + User B login before HTTP 500 returns -> User B Chat 101 untouched
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Peer A".into(),
                    avatar_initials: "PA".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.session_sync.chat_peer_user_ids.insert(CID_FABUSHI, 9001);
            shell
                .session_sync
                .rich_chats_by_id
                .insert(CID_FABUSHI, HermesRichChatDto::default());
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.stage.update(cx, |st, _| {
                st.is_peer_blocked = false;
                st.pending_toggle_block_peer = true;
            });
            shell.handle_stage_pending_actions(cx);
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "User B Peer".into(),
                    avatar_initials: "UB".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell
                .session_sync
                .rich_chats_by_id
                .insert(CID_FABUSHI, HermesRichChatDto::default());
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || err_responses_sent.load(Ordering::SeqCst) >= 3).await,
        "t5_34 toggle_block_peer 500 response timeout"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert!(
                !shell.session_sync.rich_chats_by_id[&CID_FABUSHI].is_blocked,
                "User A's failed block_user must not mutate User B's rich_chats_by_id"
            );
            assert!(!shell.stage.read(cx).is_peer_blocked);
            assert!(shell.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();

    // 4. drain_create_bot_box_actions: toast emitted AFTER open_or_select_chat(555) survives stage observer
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.open_nl_creator_modal(cx);
            let box_ent = shell.miniapps.create_bot_box.clone().expect("create_bot_box");
            box_ent.update(cx, |b, _| {
                b.pending_submit_credentials = Some((
                    "MyTestBot".into(),
                    "my_test_bot".into(),
                    String::new(),
                    false,
                ));
            });
            shell.drain_create_bot_box_actions(&box_ent, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            bot_post_done.load(Ordering::SeqCst)
                && shell
                    .read_with(&app, |s, cx| {
                        s.stage.read(cx).active_chat_id == Some(555)
                            && s.window_runtime.controller.current_toast.is_some()
                    })
                    .unwrap_or(false)
        })
        .await,
        "t5_34 create_bot toast timeout"
    );

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(555));
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("create bot toast must survive switch from Chat 101 to Chat 555");
            assert_eq!(toast.chat_id, Some(555));
        })
        .unwrap();
}

// [T5-35] logout() & user switch clear tasks_workspace / settings_ai_providers / settings_main
// Magpie state, and /vault-unlock intermediate Err closures are fenced against logout & user switch.
#[test]
fn t5_35_logout_user_switch_magpie_cleanup_and_vault_unlock_err_fence() {
    run_in_headless_subprocess(
        "t5_35_logout_user_switch_magpie_cleanup_and_vault_unlock_err_fence",
    );
}

async fn t5_35_body(app: AsyncApp) {
    use gpui_telegram_client::net::hermes_client::{HermesProviderConfigDto, MagpieStateDto};
    use gpui_telegram_desktop::views::settings::sections::settings_ai_providers::SubscriptionAccountDto;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let envelope_hits = Arc::new(AtomicUsize::new(0));
    let hold_tok_a_envelope = Arc::new(AtomicBool::new(false));
    let hold_tok_b_envelope = Arc::new(AtomicBool::new(false));
    let tok_a_envelope_arrived = Arc::new(AtomicUsize::new(0));
    let tok_a_envelope_done = Arc::new(AtomicUsize::new(0));
    let tok_b_envelope_arrived = Arc::new(AtomicUsize::new(0));
    let tok_b_envelope_done = Arc::new(AtomicUsize::new(0));

    let envelope_hits_srv = Arc::clone(&envelope_hits);
    let hold_tok_a_srv = Arc::clone(&hold_tok_a_envelope);
    let hold_tok_b_srv = Arc::clone(&hold_tok_b_envelope);
    let tok_a_arr_srv = Arc::clone(&tok_a_envelope_arrived);
    let tok_a_done_srv = Arc::clone(&tok_a_envelope_done);
    let tok_b_arr_srv = Arc::clone(&tok_b_envelope_arrived);
    let tok_b_done_srv = Arc::clone(&tok_b_envelope_done);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let envelope_hits_srv = Arc::clone(&envelope_hits_srv);
            let hold_tok_a_srv = Arc::clone(&hold_tok_a_srv);
            let hold_tok_b_srv = Arc::clone(&hold_tok_b_srv);
            let tok_a_arr_srv = Arc::clone(&tok_a_arr_srv);
            let tok_a_done_srv = Arc::clone(&tok_a_done_srv);
            let tok_b_arr_srv = Arc::clone(&tok_b_arr_srv);
            let tok_b_done_srv = Arc::clone(&tok_b_done_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let is_tok_a = req.contains("Bearer tok-a");
                let is_tok_b = req.contains("Bearer tok-b");
                if first_line.contains("/api/v1/user-vault/envelope") {
                    envelope_hits_srv.fetch_add(1, Ordering::SeqCst);
                    if is_tok_a {
                        tok_a_arr_srv.fetch_add(1, Ordering::SeqCst);
                        while hold_tok_a_srv.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    } else if is_tok_b {
                        tok_b_arr_srv.fetch_add(1, Ordering::SeqCst);
                        while hold_tok_b_srv.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    let body = r#"{"error":"simulated vault envelope failure"}"#;
                    let resp = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    if is_tok_a {
                        tok_a_done_srv.fetch_add(1, Ordering::SeqCst);
                    } else if is_tok_b {
                        tok_b_done_srv.fetch_add(1, Ordering::SeqCst);
                    }
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let body = r#"[]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let body = r#"{}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    let populate_magpie = |shell: &mut AppShell, cx: &mut gpui::Context<AppShell>| {
        let state = MagpieStateDto {
            active_provider: "openai".into(),
            active_model: "gpt-4o".into(),
            has_configured_provider: true,
            ..Default::default()
        };
        let prov = HermesProviderConfigDto {
            provider_id: "openai".into(),
            name: "OpenAI".into(),
            api_mode: "chat_completions".into(),
            default_model: "gpt-4o".into(),
            enabled: true,
            has_api_key: true,
            is_active: true,
            ..Default::default()
        };
        let sub = SubscriptionAccountDto {
            id: "codex-oauth".into(),
            account_id: Some("acct-a".into()),
            kind: "codex".into(),
            label: "Codex".into(),
            plan: Some("pro".into()),
            models: vec!["gpt-5".into()],
            active: true,
            ..Default::default()
        };
        shell.tasks_workspace.update(cx, |w, _| {
            w.sync_magpie_state(state.clone());
            w.sync_providers(vec![prov]);
        });
        let ap = shell.settings_router.update(cx, |r, rcx| r.ensure_ai_providers(rcx));
        ap.update(cx, |sec, _| {
            sec.sync_state(state.clone());
            sec.sync_subscriptions(vec![sub]);
        });
        let sm = shell.settings_router.update(cx, |r, rcx| r.ensure_main(rcx));
        sm.update(cx, |m, _| {
            m.sync_ai_providers_status(Some("OpenAI · gpt-4o".into()), false);
        });
        shell.magpie_local.latest_state = Some(state);
        shell.inspector.update(cx, |insp, _| {
            insp.ai_models.active_provider_id = "openai".into();
            insp.ai_models.active_model = "gpt-4o".into();
            insp.ai_models.active_provider_models = vec!["gpt-4o".into()];
        });
    };

    // Part 1A: logout() clears tasks_workspace, settings_ai_providers, settings_main, magpie_local, inspector
    shell
        .update(&mut app.clone(), |shell, cx| {
            populate_magpie(shell, cx);
            shell.logout(cx);

            assert!(shell.tasks_workspace.read(cx).magpie_state.is_none());
            assert!(shell.tasks_workspace.read(cx).providers.is_empty());
            let ap = shell
                .settings_router
                .read(cx)
                .settings_ai_providers
                .as_ref()
                .unwrap();
            assert!(ap.read(cx).state.is_none());
            assert!(ap.read(cx).subscription_accounts.is_empty());
            let sm = shell.settings_router.read(cx).settings_main.as_ref().unwrap();
            assert!(sm.read(cx).ai_providers_summary.is_none());
            assert!(shell.magpie_local.latest_state.is_none());
            assert!(shell.inspector.read(cx).ai_models.active_model.is_empty());
            assert!(shell.inspector.read(cx).ai_models.active_provider_models.is_empty());

            // Part 1B: Direct user switch clears all Magpie state
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            populate_magpie(shell, cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            assert!(shell.tasks_workspace.read(cx).magpie_state.is_none());
            assert!(shell.tasks_workspace.read(cx).providers.is_empty());
            let ap = shell
                .settings_router
                .read(cx)
                .settings_ai_providers
                .as_ref()
                .unwrap();
            assert!(ap.read(cx).state.is_none());
            assert!(ap.read(cx).subscription_accounts.is_empty());
            let sm = shell.settings_router.read(cx).settings_main.as_ref().unwrap();
            assert!(sm.read(cx).ai_providers_summary.is_none());
            assert!(shell.magpie_local.latest_state.is_none());
            assert!(shell.inspector.read(cx).ai_models.active_model.is_empty());
            assert!(shell.inspector.read(cx).ai_models.active_provider_models.is_empty());
        })
        .unwrap();

    // Part 2A: /vault-unlock delayed HTTP 500 after logout()
    hold_tok_a_envelope.store(false, Ordering::SeqCst);
    tok_a_envelope_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Fabushi".into(),
                    avatar_initials: "FB".into(),
                    avatar_color: 0x8F59F9,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    // Let earlier apply_authenticated_session_with_vault bootstrap /envelope calls settle deterministically
    assert!(
        pump_until(&app, 3000, || tok_a_envelope_done.load(Ordering::SeqCst) >= 1).await,
        "Part 2A bootstrap /envelope did not settle"
    );

    hold_tok_a_envelope.store(true, Ordering::SeqCst);
    tok_a_envelope_arrived.store(0, Ordering::SeqCst);
    tok_a_envelope_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.window_runtime.controller.clear_all_toasts();
            envelope_hits.store(0, Ordering::SeqCst);
            shell.send_rich_text_options("/vault-unlock mysecret".into(), false, None, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_a_envelope_arrived.load(Ordering::SeqCst) >= 1).await,
        "Part 2A /vault-unlock /envelope request did not arrive"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            assert!(shell.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();

    hold_tok_a_envelope.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || tok_a_envelope_done.load(Ordering::SeqCst) >= 1).await,
        "Part 2A /vault-unlock /envelope response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(50))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(envelope_hits.load(Ordering::SeqCst), 1);
            assert!(!shell.is_authenticated);
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
            assert!(shell.bot_browser.vault_custody_status_note.is_none());
            assert!(shell.stage.read(cx).messages.is_empty());
            assert!(!shell.stage.read(cx).chat_histories.contains_key(&CID_FABUSHI));
        })
        .unwrap();

    // Part 2B: /vault-unlock delayed HTTP 500 after logout + User B login
    hold_tok_a_envelope.store(false, Ordering::SeqCst);
    tok_a_envelope_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Fabushi".into(),
                    avatar_initials: "FB".into(),
                    avatar_color: 0x8F59F9,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    // Let earlier apply_authenticated_session_with_vault bootstrap /envelope calls settle deterministically
    assert!(
        pump_until(&app, 3000, || tok_a_envelope_done.load(Ordering::SeqCst) >= 1).await,
        "Part 2B bootstrap /envelope did not settle"
    );

    hold_tok_a_envelope.store(true, Ordering::SeqCst);
    tok_a_envelope_arrived.store(0, Ordering::SeqCst);
    tok_a_envelope_done.store(0, Ordering::SeqCst);
    tok_b_envelope_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.window_runtime.controller.clear_all_toasts();
            envelope_hits.store(0, Ordering::SeqCst);
            shell.send_rich_text_options("/vault-unlock mysecret".into(), false, None, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_a_envelope_arrived.load(Ordering::SeqCst) >= 1).await,
        "Part 2B /vault-unlock /envelope request did not arrive"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "User B Chat".into(),
                    avatar_initials: "UB".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.dialogs.push(DialogItemModel {
                    id: 102,
                    peer_name: "User B Group".into(),
                    avatar_initials: "BG".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    hold_tok_a_envelope.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || {
            tok_a_envelope_done.load(Ordering::SeqCst) >= 1
                && tok_b_envelope_done.load(Ordering::SeqCst) >= 1
        })
        .await,
        "Part 2B /envelope responses did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(50))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert!(envelope_hits.load(Ordering::SeqCst) >= 1);
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
            assert!(shell.bot_browser.vault_custody_status_note.is_none());
            let st = shell.stage.read(cx);
            assert!(st.messages.is_empty());
            assert!(
                st.chat_histories
                    .get(&CID_FABUSHI)
                    .is_none_or(|msgs| msgs.is_empty()),
                "User A's vault notice must not leak into User B's Chat 101"
            );
        })
        .unwrap();

    // Part 2C: Same user switches Chat 101 -> Chat 102 before /vault-unlock 500 returns -> no toast leak onto 102
    envelope_hits.store(0, Ordering::SeqCst);
    hold_tok_b_envelope.store(true, Ordering::SeqCst);
    tok_b_envelope_arrived.store(0, Ordering::SeqCst);
    tok_b_envelope_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.send_rich_text_options("/vault-unlock mysecret".into(), false, None, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_b_envelope_arrived.load(Ordering::SeqCst) >= 1).await,
        "Part 2C /vault-unlock /envelope request did not arrive"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    hold_tok_b_envelope.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || {
            tok_b_envelope_done.load(Ordering::SeqCst) >= 1
                && shell
                    .read_with(&app, |s, cx| {
                        s.stage
                            .read(cx)
                            .chat_histories
                            .get(&CID_FABUSHI)
                            .is_some_and(|m| !m.is_empty())
                    })
                    .unwrap_or(false)
        })
        .await,
        "Part 2C origin chat history vault notice did not arrive"
    );
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(envelope_hits.load(Ordering::SeqCst), 1);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Chat 101 /vault-unlock error toast must not leak onto switched Chat 102"
            );
            let st = shell.stage.read(cx);
            assert!(
                st.chat_histories
                    .get(&CID_FABUSHI)
                    .is_some_and(|msgs| !msgs.is_empty()),
                "origin Chat 101 should record the local vault error notice"
            );
            assert!(
                st.chat_histories
                    .get(&102)
                    .is_none_or(|msgs| msgs.is_empty()),
                "switched Chat 102 must remain clean"
            );
        })
        .unwrap();
}

// [T5-36] GAP-R10-1: Reopening `+` compose after `focus_search()` / `exit_compose_mode()` /
// `set_collapsed(true)` must bump `compose_epoch` and drop stale `create_chat` completions
// for both DM send, Group creation+send, and `commit_single_compose_recipient_to_dm`.
#[test]
fn t5_36_compose_reopen_after_exit_and_commit_dm_epoch_guards() {
    run_in_headless_subprocess("t5_36_compose_reopen_after_exit_and_commit_dm_epoch_guards");
}

async fn t5_36_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::chat_sidebar::ComposeRecipientChip;
    use gpui_telegram_proto::contracts::chats::ChatDetailDto;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let send_hits = Arc::new(AtomicUsize::new(0));
    let send_hits_srv = Arc::clone(&send_hits);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let send_hits_srv = Arc::clone(&send_hits_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/api/v1/chats/create") {
                    std::thread::sleep(Duration::from_millis(120));
                    let dto = ChatDetailDto {
                        chat_id: 777,
                        title: "CreatedChat".into(),
                        kind: "private".into(),
                        chat_type: 0,
                        creator_id: UID_VIEWER,
                        username: Some("alice_new".into()),
                        about: None,
                        is_group: false,
                        is_channel: false,
                        is_bot: false,
                        is_forum: false,
                        is_archived: false,
                        is_pinned: false,
                        is_muted: false,
                        mute_until_ms: None,
                        pts: 1,
                        last_message_id: 0,
                        last_message_date_ms: 0,
                        unread_count: 0,
                        read_inbox_max_id: 0,
                        read_outbox_max_id: 0,
                        member_count: 2,
                        pinned_message_id: None,
                        draft: None,
                    };
                    let body = serde_json::to_string(&dto).unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/api/v1/messages/send") {
                    send_hits_srv.fetch_add(1, Ordering::SeqCst);
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    let chip1 = ComposeRecipientChip {
        user_id: 0,
        username: Some("alice_new".into()),
        display_name: "Alice New".into(),
        initials: "AN".into(),
        avatar_color: 0x4A90E2,
        avatar_shape: None,
        is_bot: false,
        bot_id: None,
        chat_id: None,
    };
    let chip2 = ComposeRecipientChip {
        user_id: 0,
        username: Some("bob_new".into()),
        display_name: "Bob New".into(),
        initials: "BN".into(),
        avatar_color: 0x50E3C2,
        avatar_shape: None,
        is_bot: false,
        bot_id: None,
        chat_id: None,
    };

    // Sub-case A: DM focus_search() + toggle_plus_menu() reopen
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert!(shell.handle_shortcut_key("n", false, cx));
            shell.sidebar.update(cx, |sb, _| {
                sb.compose_draft.selected_recipients.push(chip1.clone());
            });
            shell.send_rich_text_options("secret to alice".into(), false, None, cx);
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_some());

            shell.sidebar.update(cx, |sb, _| {
                sb.focus_search();
                sb.toggle_plus_menu();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(sb.is_plus_menu_open, "reopened + compose must remain open");
            assert!(sb.compose_draft.pending_creation.is_none());
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
            assert_eq!(send_hits.load(Ordering::SeqCst), 0);
            assert!(
                shell.client.get_messages(ChatId(777)).unwrap_or_default().is_empty(),
                "stale DM send must not stage into Chat 777"
            );
        })
        .unwrap();

    // Sub-case B: Group direct exit_compose_mode() + open_compose_group_mode() reopen
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| {
                sb.cancel_compose_mode();
                sb.toggle_plus_menu();
                sb.compose_draft.selected_recipients.push(chip1.clone());
                sb.compose_draft.selected_recipients.push(chip2.clone());
            });
            shell.send_rich_text_options("secret group msg".into(), false, None, cx);
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_some());

            shell.sidebar.update(cx, |sb, _| {
                sb.exit_compose_mode();
                sb.open_compose_group_mode();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(sb.is_plus_menu_open, "reopened group compose must remain open");
            assert!(sb.compose_draft.pending_creation.is_none());
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
            assert_eq!(send_hits.load(Ordering::SeqCst), 0);
            assert!(
                shell.client.get_messages(ChatId(777)).unwrap_or_default().is_empty(),
                "stale group send must not stage into Chat 777"
            );
        })
        .unwrap();

    // Sub-case C: commit_single_compose_recipient_to_dm via Enter + set_collapsed(true) + toggle_plus_menu() reopen
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| {
                sb.cancel_compose_mode();
                sb.toggle_plus_menu();
                sb.compose_draft.selected_recipients.push(chip1.clone());
                sb.compose_recipient_query.clear();
            });
            shell.composer.update(cx, |c, _| c.is_input_focused = false);
            assert!(shell.handle_non_char_key_with_alt("enter", false, false, cx));
            assert!(shell.sidebar.read(cx).compose_draft.pending_creation.is_some());

            shell.sidebar.update(cx, |sb, _| {
                sb.set_collapsed(true);
                sb.toggle_plus_menu();
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let sb = shell.sidebar.read(cx);
            assert!(sb.is_plus_menu_open, "reopened + compose after collapse must remain open");
            assert!(sb.compose_draft.pending_creation.is_none());
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
        })
        .unwrap();
}

// [T5-37] GAP-R10-2: upload_and_send_paths mid-upload logout + User B switch must abort
// before send_message_durable so User A's file never leaks into User B's local_store or outbox.
#[test]
fn t5_37_upload_and_send_paths_mid_upload_logout_and_user_switch_no_leak() {
    run_in_headless_subprocess(
        "t5_37_upload_and_send_paths_mid_upload_logout_and_user_switch_no_leak",
    );
}

async fn t5_37_body(app: AsyncApp) {
    use gpui_telegram_proto::contracts::media::MediaUploadCompleteResponse;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let uploads: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sends: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let hold_upload = Arc::new(AtomicBool::new(true));
    let upload_done = Arc::new(AtomicUsize::new(0));
    let uploads_srv = Arc::clone(&uploads);
    let sends_srv = Arc::clone(&sends);
    let hold_upload_srv = Arc::clone(&hold_upload);
    let upload_done_srv = Arc::clone(&upload_done);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock upload server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let uploads_srv = Arc::clone(&uploads_srv);
            let sends_srv = Arc::clone(&sends_srv);
            let hold_upload_srv = Arc::clone(&hold_upload_srv);
            let upload_done_srv = Arc::clone(&upload_done_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 32768];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                let auth_hdr = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .to_string();

                if first_line.contains("/api/v1/media/upload") {
                    uploads_srv.lock().unwrap().push(auth_hdr);
                    while hold_upload_srv.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let body = serde_json::to_string(&MediaUploadCompleteResponse {
                        file_id: "file_user_a_secret".into(),
                        filename: Some("secret_user_a.txt".into()),
                        size_bytes: 17,
                        sha256: "00".repeat(32),
                        mime_type: "application/octet-stream".into(),
                        width: None,
                        height: None,
                        duration_seconds: None,
                        waveform_b64: None,
                        waveform_bars: None,
                        thumbnail_data_url: None,
                    })
                    .unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    upload_done_srv.fetch_add(1, Ordering::SeqCst);
                } else if first_line.contains("/api/v1/messages/send") {
                    sends_srv.lock().unwrap().push(auth_hdr);
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"Chat 101","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000005000},{"chat_id":102,"title":"Chat 102","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let secret_path = tmp_dir.path().join("secret_user_a.txt");
    std::fs::write(&secret_path, b"user_a_secret_doc").unwrap();

    // Explicitly omit `client.runtime().enter()` to verify GAP-R9-2 Tokio safety.
    let client = new_isolated_client(format!("http://{addr}"));
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-user-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Chat 101".into(),
                    avatar_initials: "C1".into(),
                    avatar_color: 0x4A90E2,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.upload_and_send_paths(vec![secret_path.clone()], cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || !uploads.lock().unwrap().is_empty()).await,
        "User A upload did not arrive at mock server before logout"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-user-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "User B Chat 101".into(),
                    avatar_initials: "UB".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    hold_upload.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || upload_done.load(Ordering::SeqCst) >= 1).await,
        "held User A upload response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    {
        let u = uploads.lock().unwrap();
        let s = sends.lock().unwrap();
        assert_eq!(u.len(), 1, "expected exactly 1 upload from User A: {u:?}");
        assert!(u[0].contains("tok-user-a"), "upload must belong to User A: {u:?}");
        assert!(s.is_empty(), "expected 0 sends after mid-upload user switch: {s:?}");
    }
    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell
                    .client
                    .get_messages(ChatId(CID_FABUSHI as i64))
                    .unwrap_or_default()
                    .is_empty(),
                "User A's uploaded file must not poison User B's local_store"
            );
            assert_eq!(
                shell.client.outbox_worker().store().pending_count(),
                0,
                "User A's uploaded file must not enqueue in User B's outbox"
            );
        })
        .unwrap();
}

// [T5-38] GAP-R10-3: set_ttl_secs must not emit optimistic toast before RPC settles or on HTTP 500,
// and must not leak chat-scoped toasts while in Settings or `+` compose.
#[test]
fn t5_38_set_ttl_secs_no_optimistic_toast_on_500_and_no_settings_or_compose_leak() {
    run_in_headless_subprocess(
        "t5_38_set_ttl_secs_no_optimistic_toast_on_500_and_no_settings_or_compose_leak",
    );
}

async fn t5_38_body(app: AsyncApp) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let return_500 = Arc::new(AtomicBool::new(true));
    let hold_ttl = Arc::new(AtomicBool::new(true));
    let ttl_hits = Arc::new(AtomicUsize::new(0));
    let ttl_done = Arc::new(AtomicUsize::new(0));
    let return_500_srv = Arc::clone(&return_500);
    let hold_ttl_srv = Arc::clone(&hold_ttl);
    let ttl_hits_srv = Arc::clone(&ttl_hits);
    let ttl_done_srv = Arc::clone(&ttl_done);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock ttl server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let return_500_srv = Arc::clone(&return_500_srv);
            let hold_ttl_srv = Arc::clone(&hold_ttl_srv);
            let ttl_hits_srv = Arc::clone(&ttl_hits_srv);
            let ttl_done_srv = Arc::clone(&ttl_done_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.contains("/api/v1/chats/101/ttl") {
                    ttl_hits_srv.fetch_add(1, Ordering::SeqCst);
                    while hold_ttl_srv.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if return_500_srv.load(Ordering::SeqCst) {
                        let body = r#"{"error":"simulated ttl 500"}"#;
                        let resp = format!(
                            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else {
                        let body = r#"{"ttl_seconds":86400}"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    }
                    let _ = stream.flush();
                    ttl_done_srv.fetch_add(1, Ordering::SeqCst);
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"Chat 101","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000005000},{"chat_id":102,"title":"Chat 102","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Sub-case A: active_panel == Settings + HTTP 500
    hold_ttl.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.active_panel = ActiveWorkspacePanel::Settings;
            shell.window_runtime.controller.clear_all_toasts();
            shell.stage.update(cx, |st, _| st.pending_set_ttl_seconds = Some(86_400));
            shell.handle_stage_pending_actions(cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "must not emit optimistic toast in Settings"
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || ttl_hits.load(Ordering::SeqCst) >= 1).await,
        "TTL request 1 did not reach mock server"
    );
    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "must not emit optimistic toast while TTL request 1 is held"
            );
        })
        .unwrap();
    hold_ttl.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 4000, || ttl_done.load(Ordering::SeqCst) >= 1).await,
        "TTL request 1 did not finish"
    );
    pump_until_settled(&app, 20, || ttl_done.load(Ordering::SeqCst)).await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(ttl_hits.load(Ordering::SeqCst), 1);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "error toast must not leak into Settings"
            );
            assert_eq!(shell.stage.read(cx).message_ttl_seconds, 0);
        })
        .unwrap();

    // Sub-case B: is_plus_menu_open == true + HTTP 500
    hold_ttl.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.active_panel = ActiveWorkspacePanel::Chats;
            shell.sidebar.update(cx, |sb, _| sb.toggle_plus_menu());
            shell.window_runtime.controller.clear_all_toasts();
            shell.stage.update(cx, |st, _| st.pending_set_ttl_seconds = Some(86_400));
            shell.handle_stage_pending_actions(cx);
            assert!(shell.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || ttl_hits.load(Ordering::SeqCst) >= 2).await,
        "TTL request 2 did not reach mock server"
    );
    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "must not emit optimistic toast while TTL request 2 is held"
            );
        })
        .unwrap();
    hold_ttl.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 4000, || ttl_done.load(Ordering::SeqCst) >= 2).await,
        "TTL request 2 did not finish"
    );
    pump_until_settled(&app, 20, || ttl_done.load(Ordering::SeqCst)).await;

    shell
        .read_with(&app, |shell, cx| {
            assert_eq!(ttl_hits.load(Ordering::SeqCst), 2);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "error toast must not leak into + compose"
            );
            assert_eq!(shell.stage.read(cx).message_ttl_seconds, 0);
        })
        .unwrap();

    // Sub-case C: Active Chat 101, !is_plus_menu_open + HTTP 500 vs HTTP 200
    hold_ttl.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.cancel_compose_mode());
            shell.window_runtime.controller.clear_all_toasts();
            shell.stage.update(cx, |st, _| st.pending_set_ttl_seconds = Some(86_400));
            shell.handle_stage_pending_actions(cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "must not emit optimistic toast before RPC settles"
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || ttl_hits.load(Ordering::SeqCst) >= 3).await,
        "TTL request 3 did not reach mock server"
    );
    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "must not emit optimistic toast while TTL 500 is held"
            );
        })
        .unwrap();
    hold_ttl.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 4000, || {
            ttl_done.load(Ordering::SeqCst) >= 3
                && shell
                    .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                    .unwrap_or(false)
        })
        .await,
        "expected error toast on HTTP 500 in active chat"
    );

    shell
        .read_with(&app, |shell, _| {
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected error toast on HTTP 500 in active chat");
            assert_eq!(toast.chat_id, Some(CID_FABUSHI));
            assert!(
                toast.text.contains("保存失败") || toast.text.contains("Failed"),
                "unexpected 500 toast: {}",
                toast.text
            );
        })
        .unwrap();

    return_500.store(false, Ordering::SeqCst);
    hold_ttl.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.window_runtime.controller.clear_all_toasts();
            shell.stage.update(cx, |st, _| st.pending_set_ttl_seconds = Some(86_400));
            shell.handle_stage_pending_actions(cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "must not emit optimistic toast before HTTP 200 settles"
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || ttl_hits.load(Ordering::SeqCst) >= 4).await,
        "TTL request 4 did not reach mock server"
    );
    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "must not emit optimistic toast while TTL 200 is held"
            );
        })
        .unwrap();
    hold_ttl.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 4000, || {
            ttl_done.load(Ordering::SeqCst) >= 4
                && shell
                    .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                    .unwrap_or(false)
        })
        .await,
        "expected success toast after HTTP 200"
    );

    shell
        .read_with(&app, |shell, _| {
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected success toast after HTTP 200");
            assert_eq!(toast.chat_id, Some(CID_FABUSHI));
            assert!(
                toast.text.contains("1 天") || toast.text.contains("1 day"),
                "unexpected 200 toast: {}",
                toast.text
            );
        })
        .unwrap();
}

// [T5-39] GAP-R10-4 + shell_modals.rs async fences, ForwardToChat Tokio safety, and RunChatExport toast scoping.
#[test]
fn t5_39_shell_modals_proxy_and_sibling_async_fences_and_toast_scoping() {
    run_in_headless_subprocess("t5_39_shell_modals_proxy_and_sibling_async_fences_and_toast_scoping");
}

async fn t5_39_body(app: AsyncApp) {
    use gpui_telegram_client::session_persistence::ProxyEndpointRecord;
    use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let delete_hits_tok_b = Arc::new(AtomicUsize::new(0));
    let delete_hits_tok_b_srv = Arc::clone(&delete_hits_tok_b);

    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock modal api");
    let api_addr = api_listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            let delete_hits_tok_b_srv = Arc::clone(&delete_hits_tok_b_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                let is_tok_b = req.contains("Bearer tok-b");

                if first_line.starts_with("DELETE ") && first_line.contains("/messages/") {
                    if is_tok_b {
                        delete_hits_tok_b_srv.fetch_add(1, Ordering::SeqCst);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/api/v1/chats/create") {
                    std::thread::sleep(Duration::from_millis(80));
                    let body = r#"{"chat_id":777,"title":"ModalCreated","kind":"group","chat_type":1,"creator_id":100001,"is_group":true,"is_channel":false,"is_bot":false,"is_forum":false,"is_archived":false,"is_pinned":false,"is_muted":false,"pts":1,"last_message_id":0,"last_message_date_ms":0,"unread_count":0,"read_inbox_max_id":0,"read_outbox_max_id":0,"member_count":1}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/export") {
                    std::thread::sleep(Duration::from_millis(80));
                    let body = "/tmp/export_101.json";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages/send") {
                    std::thread::sleep(Duration::from_millis(60));
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"Chat 101","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000005000},{"chat_id":102,"title":"Chat 102","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    // Mock slow HTTP proxy sleeping 140ms on CONNECT
    let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock proxy");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = proxy_listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                std::thread::sleep(Duration::from_millis(140));
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
            });
        }
    });

    // Omit client.runtime().enter() so ForwardToChat Tokio spawn safety is also verified.
    let client = new_isolated_client(format!("http://{api_addr}"));
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // 1. AddProxy slow TCP -> logout()
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.handle_modal_outcome(
                ModalActionOutcome::AddProxy(ProxyEndpointRecord {
                    id: 91,
                    kind: "HTTP".into(),
                    host: "127.0.0.1".into(),
                    port: proxy_port,
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 0,
                    enabled: true,
                }),
                cx,
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(25))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(200))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(!shell.is_authenticated);
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // 2. AddProxy slow TCP -> logout() + User B login
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.handle_modal_outcome(
                ModalActionOutcome::AddProxy(ProxyEndpointRecord {
                    id: 92,
                    kind: "HTTP".into(),
                    host: "127.0.0.1".into(),
                    port: proxy_port,
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 0,
                    enabled: true,
                }),
                cx,
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(25))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(200))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.window_runtime.controller.toast_queue.is_empty());
        })
        .unwrap();

    // 3. SelectProxy fallback (selected_rec == None, proxy_id = 99) -> logout() + User B login
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            let mut bridge = gpui_telegram_client::session_persistence::SessionPersistenceBridge::new(
                shell.client.http_client.base_url.clone(),
            );
            bridge.state.proxies.push(ProxyEndpointRecord {
                id: 99,
                kind: "HTTP".into(),
                host: "127.0.0.1".into(),
                port: proxy_port,
                secret_or_user: String::new(),
                username: None,
                password: None,
                ping_ms: 0,
                enabled: false,
            });
            bridge.save_to_disk();
            shell.modal_container.update(cx, |m, _| m.dialog.proxies.clear());
            shell.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 99 }, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(25))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(200))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            if let Some(ref sa) = shell.settings_router.read(cx).settings_advanced {
                let adv = sa.read(cx);
                assert_ne!(
                    adv.proxy_port, proxy_port,
                    "stale User A SelectProxy must not overwrite proxy_port after logout/switch"
                );
                assert!(
                    adv.proxy_host.is_empty(),
                    "expected empty proxy_host, got {:?} {}:{}",
                    adv.proxy_type,
                    adv.proxy_host,
                    adv.proxy_port
                );
            }
            assert!(shell.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();

    // 4. Sibling shell_modals.rs handlers across logout() + User B switch & RunChatExport scoping
    shell
        .update(&mut app.clone(), |shell, cx| {
            let _ = shell.client.apply_proxy(None);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Chat 101".into(),
                    avatar_initials: "C1".into(),
                    avatar_color: 0x4A90E2,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);

            shell.handle_modal_outcome(
                ModalActionOutcome::CreateGroupSubmitted {
                    title: "StaleGroup".into(),
                    members_count: 1,
                    enable_forum_topics: true,
                },
                cx,
            );
            shell.handle_modal_outcome(
                ModalActionOutcome::CreateChannelSubmitted {
                    title: "StaleChannel".into(),
                    username_slug: "stale_chan".into(),
                    is_public: false,
                },
                cx,
            );
            shell.handle_modal_outcome(
                ModalActionOutcome::ForwardToChat {
                    target_chat_id: 102,
                    message_ids: vec![1, 2, 3],
                    extra_caption: "secret fwd".into(),
                    show_sender: true,
                },
                cx,
            );
            shell.handle_modal_outcome(
                ModalActionOutcome::ConfirmDeleteMessages {
                    message_ids: vec![11, 12, 13],
                    revoke_for_everyone: true,
                },
                cx,
            );
            shell.handle_modal_outcome(
                ModalActionOutcome::RunChatExport { format_json: true },
                cx,
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "User B Chat 101".into(),
                    avatar_initials: "UB".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(200))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert_ne!(
                shell.stage.read(cx).active_chat_id,
                Some(777),
                "stale CreateGroup/CreateChannel must not hijack User B's active_chat_id"
            );
            assert!(
                shell.client.get_messages(ChatId(102)).unwrap_or_default().is_empty(),
                "stale ForwardToChat must not stage into User B's Chat 102"
            );
            assert_eq!(
                delete_hits_tok_b.load(Ordering::SeqCst),
                0,
                "ConfirmDeleteMessages loop must abort on logout/switch"
            );
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "User A RunChatExport toast must not leak onto User B's Chat 101"
            );
        })
        .unwrap();

    // Also verify RunChatExport does not leak Chat-101 toast into Settings
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.window_runtime.controller.clear_all_toasts();
            shell.handle_modal_outcome(
                ModalActionOutcome::RunChatExport { format_json: true },
                cx,
            );
            shell.navigate_settings_to(SettingsSectionType::Main, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "RunChatExport toast must not leak into Settings: {:?}",
                shell.window_runtime.controller.current_toast
            );
        })
        .unwrap();
}

// [T5-40] GAP-R10-5: Settings toasts stay non-chat after close_settings(), ApplyGlobalTtlToExistingChats
// aborts loop on logout()/switch, and CreateFolder/DeleteFolder fence against logout/switch.
#[test]
fn t5_40_shell_observers_settings_toast_scoping_ttl_loop_and_folders_fence() {
    run_in_headless_subprocess(
        "t5_40_shell_observers_settings_toast_scoping_ttl_loop_and_folders_fence",
    );
}

async fn t5_40_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::settings::sections::settings_privacy_security::SettingsVaultAction;
    use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
    use gpui_telegram_desktop::views::settings::SettingsRouterEvent;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let ttl_tok_a = Arc::new(AtomicUsize::new(0));
    let ttl_tok_b = Arc::new(AtomicUsize::new(0));
    let ttl_arrived = Arc::new(AtomicUsize::new(0));
    let hold_ttl = Arc::new(AtomicBool::new(false));
    let ttl_done = Arc::new(AtomicUsize::new(0));
    let create_folder_arrived = Arc::new(AtomicBool::new(false));
    let release_create_folder = Arc::new(AtomicBool::new(false));
    let create_folder_done = Arc::new(AtomicBool::new(false));
    let folder_gets: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let ttl_tok_a_srv = Arc::clone(&ttl_tok_a);
    let ttl_tok_b_srv = Arc::clone(&ttl_tok_b);
    let ttl_arrived_srv = Arc::clone(&ttl_arrived);
    let hold_ttl_srv = Arc::clone(&hold_ttl);
    let ttl_done_srv = Arc::clone(&ttl_done);
    let create_folder_arrived_srv = Arc::clone(&create_folder_arrived);
    let release_create_folder_srv = Arc::clone(&release_create_folder);
    let create_folder_done_srv = Arc::clone(&create_folder_done);
    let folder_gets_srv = Arc::clone(&folder_gets);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock settings server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let ttl_tok_a_srv = Arc::clone(&ttl_tok_a_srv);
            let ttl_tok_b_srv = Arc::clone(&ttl_tok_b_srv);
            let ttl_arrived_srv = Arc::clone(&ttl_arrived_srv);
            let hold_ttl_srv = Arc::clone(&hold_ttl_srv);
            let ttl_done_srv = Arc::clone(&ttl_done_srv);
            let create_folder_arrived_srv = Arc::clone(&create_folder_arrived_srv);
            let release_create_folder_srv = Arc::clone(&release_create_folder_srv);
            let create_folder_done_srv = Arc::clone(&create_folder_done_srv);
            let folder_gets_srv = Arc::clone(&folder_gets_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                let auth_hdr = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .to_string();

                if first_line.contains("/api/v1/auth/2fa/set") {
                    std::thread::sleep(Duration::from_millis(80));
                    let body = r#"{"error":"simulated 2fa 500"}"#;
                    let resp = format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/api/v1/auth/sessions/")
                    || first_line.contains("/api/v1/users/9001/block")
                {
                    std::thread::sleep(Duration::from_millis(60));
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/api/v1/wallet/topup") {
                    std::thread::sleep(Duration::from_millis(60));
                    let body = r#"{"stars_balance":100,"ton_nano":0}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/ttl") {
                    if auth_hdr.contains("tok-a") {
                        ttl_tok_a_srv.fetch_add(1, Ordering::SeqCst);
                    } else if auth_hdr.contains("tok-b") {
                        ttl_tok_b_srv.fetch_add(1, Ordering::SeqCst);
                    }
                    ttl_arrived_srv.fetch_add(1, Ordering::SeqCst);
                    while hold_ttl_srv.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let body = r#"{"ttl_seconds":86400}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    ttl_done_srv.fetch_add(1, Ordering::SeqCst);
                } else if first_line.starts_with("POST /api/v1/folders") {
                    create_folder_arrived_srv.store(true, Ordering::SeqCst);
                    while !release_create_folder_srv.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let body = r#"{"id":9,"title":"Work","include_contacts":true,"include_non_contacts":true,"include_groups":true,"include_channels":true,"include_bots":true}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    create_folder_done_srv.store(true, Ordering::SeqCst);
                } else if first_line.starts_with("GET /api/v1/folders") {
                    folder_gets_srv.lock().unwrap().push(auth_hdr);
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"Chat 101","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000005000},{"chat_id":102,"title":"Chat 102","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // 1. TwoFactorMutation HTTP 500 -> close_settings() to Chat 101 -> stays non-chat & survives chat switch
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Chat 101".into(),
                    avatar_initials: "C1".into(),
                    avatar_color: 0x4A90E2,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.dialogs.push(DialogItemModel {
                    id: 102,
                    peer_name: "Chat 102".into(),
                    avatar_initials: "C2".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.window_runtime.controller.clear_all_toasts();

            shell.navigate_settings_to(SettingsSectionType::PrivacySecurity, cx);
            shell.handle_settings_router_event(
                &SettingsRouterEvent::TwoFactorMutation(Some("pw".into())),
                cx,
            );
            shell.close_settings(cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                .unwrap_or(false)
        })
        .await,
        "2FA failure toast timeout"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            let toast = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("2FA failure toast must appear");
            assert_eq!(toast.chat_id, None, "Settings toast must have chat_id == None");
            assert!(
                !toast.allow_chat_auto_scope,
                "Settings toast must have allow_chat_auto_scope == false"
            );

            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_some(),
                "global Settings toast must NOT be cancelled by chat switch 101 -> 102"
            );
        })
        .unwrap();

    // 2. Sibling Settings toasts stay non-chat after close_settings()
    for kind in 0..5u8 {
        shell
            .update(&mut app.clone(), |shell, cx| {
                shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
                shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
                shell.window_runtime.controller.clear_all_toasts();
                shell.navigate_settings_to(SettingsSectionType::PrivacySecurity, cx);
                match kind {
                    0 => shell.handle_settings_router_event(
                        &SettingsRouterEvent::RevokeSession("sess-old".into()),
                        cx,
                    ),
                    1 => shell.handle_settings_router_event(
                        &SettingsRouterEvent::RevokeOtherSessions,
                        cx,
                    ),
                    2 => shell.handle_settings_router_event(
                        &SettingsRouterEvent::UnblockPeer {
                            user_id: 9001,
                            remaining: 0,
                        },
                        cx,
                    ),
                    3 => shell.handle_settings_router_event(
                        &SettingsRouterEvent::TopupStars(50),
                        cx,
                    ),
                    _ => shell.handle_settings_vault_action(
                        &SettingsVaultAction::CopyEmergencyKit,
                        cx,
                    ),
                }
                shell.close_settings(cx);
            })
            .unwrap();

        assert!(
            pump_until(&app, 3000, || {
                shell
                    .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                    .unwrap_or(false)
            })
            .await,
            "Settings toast timeout for kind={kind}"
        );

        shell
            .read_with(&app, |shell, _| {
                let toast = shell
                    .window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .expect("Settings toast expected");
                assert_eq!(toast.chat_id, None, "kind={kind} leaked chat_id");
                assert!(
                    !toast.allow_chat_auto_scope,
                    "kind={kind} had allow_chat_auto_scope=true"
                );
            })
            .unwrap();
    }

    // 3. ApplyGlobalTtlToExistingChats loop abort on logout() + User B switch
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| {
                for extra_id in [103u64, 104u64] {
                    sb.dialogs.push(DialogItemModel {
                        id: extra_id,
                        peer_name: format!("Peer {extra_id}"),
                        avatar_initials: "P".into(),
                        avatar_color: 0x4A90E2,
                        last_message: String::new(),
                        timestamp: String::new(),
                        unread_count: 0,
                        is_muted: false,
                        is_pinned: false,
                        is_online: true,
                        typing_indicator: None,
                        delivery_status: DialogDeliveryStatus::Read,
                        folder: "All".into(),
                    });
                }
            });
            ttl_tok_a.store(0, Ordering::SeqCst);
            ttl_tok_b.store(0, Ordering::SeqCst);
            ttl_arrived.store(0, Ordering::SeqCst);
            ttl_done.store(0, Ordering::SeqCst);
            hold_ttl.store(true, Ordering::SeqCst);
            shell.handle_settings_router_event(
                &SettingsRouterEvent::ApplyGlobalTtlToExistingChats(86_400),
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || ttl_arrived.load(Ordering::SeqCst) >= 1).await,
        "ApplyGlobalTtlToExistingChats first /ttl request did not arrive"
    );

    folder_gets.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    hold_ttl.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || ttl_done.load(Ordering::SeqCst) >= 1).await,
        "ApplyGlobalTtlToExistingChats first /ttl response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    assert!(
        ttl_tok_a.load(Ordering::SeqCst) <= 1,
        "ApplyGlobalTtlToExistingChats should abort after 1st iteration on logout: {}",
        ttl_tok_a.load(Ordering::SeqCst)
    );
    assert_eq!(
        ttl_tok_b.load(Ordering::SeqCst),
        0,
        "ApplyGlobalTtlToExistingChats must never send with User B's token"
    );

    // 4. CreateFolder auth fence
    create_folder_arrived.store(false, Ordering::SeqCst);
    release_create_folder.store(false, Ordering::SeqCst);
    create_folder_done.store(false, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.handle_settings_router_event(
                &SettingsRouterEvent::CreateFolder {
                    id: 9,
                    title: "Work".into(),
                },
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || create_folder_arrived.load(Ordering::SeqCst)).await,
        "POST /api/v1/folders did not arrive"
    );

    folder_gets.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            folder_gets
                .lock()
                .unwrap()
                .iter()
                .filter(|h| h.contains("tok-b"))
                .count()
                >= 2
        })
        .await,
        "User B login folder refreshes did not complete"
    );
    assert!(
        !create_folder_done.load(Ordering::SeqCst),
        "POST /api/v1/folders finished before User B login barrier"
    );
    folder_gets.lock().unwrap().clear();

    release_create_folder.store(true, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || create_folder_done.load(Ordering::SeqCst)).await,
        "POST /api/v1/folders did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    assert!(
        folder_gets.lock().unwrap().is_empty(),
        "stale User A CreateFolder completion must not trigger refresh_folders under User B: {:?}",
        folder_gets.lock().unwrap()
    );
}

// [T5-41] r10_2 GAP-R10-1: All shell_hermes_studio_sync async closures (spawn_honcho_request,
// ReloadFrozenSnapshot, ToggleToolset, PollTerminalProcesses, KillTerminalProcess, SaveHeartbeat,
// SearchSessions, ExportSession, and spawn_and_refresh!) fence against logout() + User C login.
#[test]
fn t5_41_hermes_studio_sync_fences_all_tasks_workspace_closures_on_logout_and_switch() {
    run_in_headless_subprocess(
        "t5_41_hermes_studio_sync_fences_all_tasks_workspace_closures_on_logout_and_switch",
    );
}

async fn t5_41_body(app: AsyncApp) {
    use gpui_telegram_client::net::hermes_client::HermesHeartbeatConfigDto;
    use gpui_telegram_desktop::views::global_tasks_workspace::TasksWorkspaceAction;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let tasks_list_tok_c_hits = Arc::new(AtomicUsize::new(0));
    let hold_tok_a = Arc::new(AtomicBool::new(false));
    let tok_a_create_task_arrived = Arc::new(AtomicBool::new(false));
    let tok_a_create_task_done = Arc::new(AtomicBool::new(false));
    let tok_a_held_count = Arc::new(AtomicUsize::new(0));
    let tok_a_done_count = Arc::new(AtomicUsize::new(0));

    let tasks_list_tok_c_hits_srv = Arc::clone(&tasks_list_tok_c_hits);
    let hold_tok_a_srv = Arc::clone(&hold_tok_a);
    let tok_a_create_arr_srv = Arc::clone(&tok_a_create_task_arrived);
    let tok_a_create_done_srv = Arc::clone(&tok_a_create_task_done);
    let tok_a_held_srv = Arc::clone(&tok_a_held_count);
    let tok_a_done_srv = Arc::clone(&tok_a_done_count);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock hermes server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let tasks_list_tok_c_hits_srv = Arc::clone(&tasks_list_tok_c_hits_srv);
            let hold_tok_a_srv = Arc::clone(&hold_tok_a_srv);
            let tok_a_create_arr_srv = Arc::clone(&tok_a_create_arr_srv);
            let tok_a_create_done_srv = Arc::clone(&tok_a_create_done_srv);
            let tok_a_held_srv = Arc::clone(&tok_a_held_srv);
            let tok_a_done_srv = Arc::clone(&tok_a_done_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let is_tok_c = req.contains("Bearer tok-user-c");
                let is_tok_a = req.contains("Bearer tok-user-a");

                let is_held_tok_a = is_tok_a
                    && (first_line.contains("/api/v1/hermes/") || first_line.contains("/api/v1/tasks"));
                let is_tok_a_create_task = is_tok_a && first_line.starts_with("POST /api/v1/tasks");
                if is_held_tok_a {
                    tok_a_held_srv.fetch_add(1, Ordering::SeqCst);
                    if is_tok_a_create_task {
                        tok_a_create_arr_srv.store(true, Ordering::SeqCst);
                    }
                    while hold_tok_a_srv.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }

                let body = if first_line.contains("/api/v1/hermes/honcho") && is_tok_a {
                    r#"{"enabled":true,"profile_summary":"USER_A_SECRET_HONCHO_SUMMARY","peer_count":1}"#
                } else if first_line.contains("/api/v1/hermes/memory") && is_tok_a {
                    r#"{"memory_markdown":"USER_A_SECRET_MEMORY","user_markdown":"USER_A_SECRET_USER_MD","char_limit":2200}"#
                } else if first_line.contains("/api/v1/hermes/sessions/search") && is_tok_a {
                    r#"{"results":[{"session_id":"sess-a-1","snippet":"USER_A_SECRET_SNIPPET"}]}"#
                } else if first_line.contains("/api/v1/hermes/sessions/")
                    && first_line.contains("/export")
                    && is_tok_a
                {
                    r#"{"session_id":"sess-a-1","format":"markdown","content":"USER_A_SECRET_EXPORT"}"#
                } else if first_line.contains("/api/v1/hermes/toolsets") && is_tok_a {
                    r#"{"toolsets":[{"name":"user_a_secret_toolset","description":"secret","enabled":false,"tools":[],"risk_level":"high"}]}"#
                } else if first_line.contains("/api/v1/hermes/terminal/processes") && is_tok_a {
                    r#"{"processes":[{"process_id":"proc-a-1","command":"cat /user_a_secret","status":"running"}]}"#
                } else if first_line.contains("/api/v1/hermes/heartbeat") && is_tok_a {
                    r#"{"enabled":true,"interval_secs":77,"prompt":"USER_A_SECRET_HEARTBEAT"}"#
                } else if first_line.starts_with("GET /api/v1/tasks") {
                    if is_tok_c {
                        tasks_list_tok_c_hits_srv.fetch_add(1, Ordering::SeqCst);
                    }
                    r#"[]"#
                } else if first_line.contains("/messages")
                    || first_line.contains("/chats")
                    || first_line.contains("/api/v1/mcp/")
                    || first_line.contains("/api/v1/hermes/skills")
                    || first_line.contains("/api/v1/hermes/cron")
                    || first_line.contains("/api/v1/hermes/sessions")
                    || first_line.contains("/api/v1/hermes/providers")
                    || first_line.contains("/api/v1/hermes/kanban")
                    || first_line.contains("/api/v1/hermes/approvals")
                    || first_line.contains("/api/v1/hermes/commands")
                    || first_line.contains("/api/v1/hermes/toolsets")
                    || first_line.contains("/api/v1/hermes/terminal/processes")
                {
                    r#"[]"#
                } else {
                    r#"{}"#
                };

                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                if is_held_tok_a {
                    if is_tok_a_create_task {
                        tok_a_create_done_srv.store(true, Ordering::SeqCst);
                    }
                    tok_a_done_srv.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-user-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            hold_tok_a.store(true, Ordering::SeqCst);
            tok_a_create_task_arrived.store(false, Ordering::SeqCst);
            tok_a_create_task_done.store(false, Ordering::SeqCst);
            tok_a_held_count.store(0, Ordering::SeqCst);
            tok_a_done_count.store(0, Ordering::SeqCst);
            shell.handle_tasks_workspace_action(
                TasksWorkspaceAction::QueryHoncho("who am i".into()),
                cx,
            );
            shell.handle_tasks_workspace_action(TasksWorkspaceAction::ReloadFrozenSnapshot, cx);
            shell.handle_tasks_workspace_action(
                TasksWorkspaceAction::SearchSessions("secret".into()),
                cx,
            );
            shell.handle_tasks_workspace_action(
                TasksWorkspaceAction::ExportSession {
                    session_id: "sess-a-1".into(),
                    format: "markdown".into(),
                },
                cx,
            );
            shell.handle_tasks_workspace_action(
                TasksWorkspaceAction::ToggleToolset {
                    name: "core".into(),
                    enabled: false,
                },
                cx,
            );
            shell.handle_tasks_workspace_action(TasksWorkspaceAction::PollTerminalProcesses, cx);
            shell.handle_tasks_workspace_action(
                TasksWorkspaceAction::KillTerminalProcess("proc-a-1".into()),
                cx,
            );
            shell.handle_tasks_workspace_action(
                TasksWorkspaceAction::SaveHeartbeat(HermesHeartbeatConfigDto {
                    enabled: true,
                    interval_secs: 77,
                    prompt: "USER_A_SECRET_HEARTBEAT".into(),
                    ..Default::default()
                }),
                cx,
            );
            shell.handle_tasks_workspace_action(
                TasksWorkspaceAction::CreateTask("user_a_task".into()),
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_a_create_task_arrived.load(Ordering::SeqCst)).await,
        "User A POST /api/v1/tasks did not arrive"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 303,
                    username: "user_c".into(),
                    session_id: "sess-c".into(),
                    token: "tok-user-c".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            tasks_list_tok_c_hits.store(0, Ordering::SeqCst);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tasks_list_tok_c_hits.load(Ordering::SeqCst) >= 1).await,
        "User C initial refresh_tasks_and_mcp GET /api/v1/tasks did not arrive"
    );
    let tok_c_hits_before_user_a_settles = tasks_list_tok_c_hits.load(Ordering::SeqCst);

    hold_tok_a.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || {
            tok_a_create_task_done.load(Ordering::SeqCst)
                && tok_a_done_count.load(Ordering::SeqCst) >= tok_a_held_count.load(Ordering::SeqCst)
        })
        .await,
        "User A in-flight hermes/tasks responses did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            let ws = shell.tasks_workspace.read(cx);
            assert!(
                ws.honcho_state.profile_summary.is_empty(),
                "User A Honcho leaked into User C: {:?}",
                ws.honcho_state
            );
            assert!(
                ws.memory_state.memory_markdown.is_empty() && ws.memory_edit_draft.is_empty(),
                "User A memory leaked into User C: {:?}",
                ws.memory_state.memory_markdown
            );
            assert!(
                ws.session_search_hits.is_empty(),
                "User A session search hits leaked into User C: {:?}",
                ws.session_search_hits
            );
            assert!(
                ws.session_export_preview.is_none(),
                "User A session export leaked into User C: {:?}",
                ws.session_export_preview
            );
            assert!(
                !ws.toolsets.iter().any(|t| t.name == "user_a_secret_toolset"),
                "User A toolsets leaked into User C: {:?}",
                ws.toolsets
            );
            assert!(
                ws.terminal_processes.is_empty(),
                "User A terminal processes leaked into User C: {:?}",
                ws.terminal_processes
            );
            assert_ne!(
                ws.heartbeat.prompt, "USER_A_SECRET_HEARTBEAT",
                "User A heartbeat leaked into User C: {:?}",
                ws.heartbeat
            );
            assert_eq!(
                tasks_list_tok_c_hits.load(Ordering::SeqCst),
                tok_c_hits_before_user_a_settles,
                "User A's CreateTask spawn_and_refresh! must NOT trigger an extra refresh_tasks_and_mcp under User C"
            );
        })
        .unwrap();
}

// [T5-42] drain_create_bot_box_actions modal-dismiss/reopen race & TaskWakeupTriggered /
// BotAssistCardAction auth + toast + Fabushi-target guards.
#[test]
fn t5_42_create_bot_box_close_race_and_task_wakeup_scoping_and_auth_guards() {
    run_in_headless_subprocess(
        "t5_42_create_bot_box_close_race_and_task_wakeup_scoping_and_auth_guards",
    );
}

async fn t5_42_body(app: AsyncApp) {
    use bot_browser::{BotBrowserEvent, TaskWakeup};
    use gpui_telegram_proto::miniapp_contracts::BotTokenResponse;
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock bot server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                if first_line.starts_with("POST /api/v1/bots ") {
                    std::thread::sleep(Duration::from_millis(100));
                    let bot_resp = BotTokenResponse {
                        bot: BotRegistryEntry {
                            bot_user_id: 777_888,
                            owner_user_id: UID_VIEWER as i64,
                            username: "delayed_bot".into(),
                            display_name: "DelayedBot".into(),
                            is_fabushi_bot: true,
                            ..Default::default()
                        },
                        bot_token: "bot_tok".into(),
                        companion_chat_id: Some(888),
                    };
                    let body = serde_json::to_string(&bot_resp).unwrap();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/messages") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else if first_line.contains("/chats") {
                    let body = r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.open_nl_creator_modal(cx);
            let box_ent = shell.miniapps.create_bot_box.clone().expect("box");
            box_ent.update(cx, |b, _| {
                b.pending_submit_credentials = Some((
                    "DelayedBot".into(),
                    "delayed_bot".into(),
                    String::new(),
                    false,
                ));
            });
            shell.drain_create_bot_box_actions(&box_ent, cx);

            box_ent.update(cx, |b, _| b.pending_close = true);
            shell.drain_create_bot_box_actions(&box_ent, cx);
            assert!(shell.miniapps.create_bot_box.is_none());

            shell.open_nl_creator_modal(cx);
            assert!(shell.miniapps.create_bot_box.is_some());
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(
                shell.miniapps.create_bot_box.is_some(),
                "stale create_bot completion must NOT clobber newly reopened CreateBotBox"
            );
            assert_eq!(
                shell.stage.read(cx).active_chat_id,
                Some(CID_FABUSHI),
                "dismissed CreateBotBox completion must NOT hijack active_chat_id to 888"
            );
            let new_box = shell.miniapps.create_bot_box.clone().unwrap();
            new_box.update(cx, |b, _| b.pending_close = true);
            shell.drain_create_bot_box_actions(&new_box, cx);
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.active_panel = ActiveWorkspacePanel::Settings;
            shell.apply_bot_browser_event(
                BotBrowserEvent::TaskWakeupTriggered(TaskWakeup {
                    task_id: "t1".into(),
                    session_id: "s1".into(),
                    chat_id: Some(CID_FABUSHI as i64),
                    target_url: "https://example.com".into(),
                    origin: "example.com".into(),
                    credential_id: "c1".into(),
                    triggered_at_ms: 1,
                }),
                cx,
            );
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "TaskWakeupTriggered leaked Chat-101 toast into Settings"
            );

            shell.active_panel = ActiveWorkspacePanel::Chats;
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(102));
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.apply_bot_browser_event(
                BotBrowserEvent::TaskWakeupTriggered(TaskWakeup {
                    task_id: "t2".into(),
                    session_id: "s2".into(),
                    chat_id: None,
                    target_url: "https://example.com".into(),
                    origin: "example.com".into(),
                    credential_id: "c2".into(),
                    triggered_at_ms: 2,
                }),
                cx,
            );
            assert!(
                shell
                    .sidebar
                    .read(cx)
                    .dialogs
                    .iter()
                    .find(|d| d.id == 102)
                    .is_some_and(|d| d.typing_indicator.is_none()),
                "TaskWakeupTriggered with chat_id=None must NOT spawn Fabushi turn on Group Chat 102"
            );

            shell.logout(cx);
            shell.window_runtime.controller.clear_all_toasts();
            shell.apply_bot_browser_event(
                BotBrowserEvent::TaskWakeupTriggered(TaskWakeup {
                    task_id: "t3".into(),
                    session_id: "s3".into(),
                    chat_id: Some(CID_FABUSHI as i64),
                    target_url: "https://example.com".into(),
                    origin: "example.com".into(),
                    credential_id: "c3".into(),
                    triggered_at_ms: 3,
                }),
                cx,
            );
            assert!(shell.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();
}

// [T5-43] execute_local_vault_unlock Ok(None) & wrong-passphrase Err branches,
// deleted-chat notice suppression, bootstrap_local_subscriptions_on_startup logout fence,
// and ShellMagpieLocalState::reset_on_logout() full field reset.
#[test]
fn t5_43_vault_unlock_all_branches_bootstrap_keepalive_and_magpie_local_reset() {
    run_in_headless_subprocess(
        "t5_43_vault_unlock_all_branches_bootstrap_keepalive_and_magpie_local_reset",
    );
}

async fn t5_43_body(app: AsyncApp) {
    use gpui_telegram_client::net::hermes_client::MagpieStateDto;
    use gpui_telegram_desktop::views::settings::sections::settings_ai_providers::SubscriptionAccountDto;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};

    let return_valid_envelope = Arc::new(AtomicBool::new(false));
    let return_valid_envelope_srv = Arc::clone(&return_valid_envelope);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock vault server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let return_valid_envelope_srv = Arc::clone(&return_valid_envelope_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let body = if first_line.contains("/api/v1/user-vault/envelope") {
                    std::thread::sleep(Duration::from_millis(80));
                    if return_valid_envelope_srv.load(Ordering::SeqCst) {
                        r#"{"envelope":{"key_version":1,"kdf_alg":"argon2id","kdf_salt_b64":"AAAAAAAAAAAAAAAAAAAAAA==","kdf_m_cost_kib":1024,"kdf_t_cost":1,"kdf_p_cost":1,"wrapped_root_key_b64":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}}"#
                            .to_string()
                    } else {
                        r#"{"envelope":null}"#.to_string()
                    }
                } else if first_line.contains("/api/v1/hermes/magpie/state") {
                    std::thread::sleep(Duration::from_millis(80));
                    r#"{"active_provider":"openai","active_model":"gpt-4o","has_configured_provider":true}"#
                        .to_string()
                } else if first_line.contains("/api/v1/hermes/magpie/subscriptions") {
                    r#"[]"#.to_string()
                } else if first_line.contains("/messages") {
                    r#"[]"#.to_string()
                } else if first_line.contains("/chats") {
                    r#"[{"chat_id":101,"title":"Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Group 102","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#.to_string()
                } else {
                    r#"{}"#.to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.magpie_local.latest_state = Some(MagpieStateDto {
                active_provider: "openai".into(),
                ..Default::default()
            });
            shell.magpie_local.server_subscriptions = vec![SubscriptionAccountDto {
                id: "sub1".into(),
                account_id: None,
                kind: "codex".into(),
                label: "Codex".into(),
                plan: None,
                models: vec!["gpt-5".into()],
                active: true,
                ..Default::default()
            }];
            shell.magpie_local.bootstrapped_auth_generation = Some(42);
            let prev_keepalive = shell.magpie_local.legacy_keepalive_generation;
            shell.magpie_local.active_legacy_subscription_id = Some("sub1".into());
            shell.magpie_local.active_legacy_model = Some("gpt-5".into());

            shell.logout(cx);

            assert!(shell.magpie_local.discovered_items.is_empty());
            assert!(shell.magpie_local.latest_state.is_none());
            assert!(shell.magpie_local.server_subscriptions.is_empty());
            assert_eq!(shell.magpie_local.bootstrapped_auth_generation, None);
            assert_ne!(shell.magpie_local.legacy_keepalive_generation, prev_keepalive);
            assert!(shell.magpie_local.active_legacy_subscription_id.is_none());
            assert!(shell.magpie_local.active_legacy_model.is_none());
        })
        .unwrap();

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Fabushi".into(),
                    avatar_initials: "FB".into(),
                    avatar_color: 0x8F59F9,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.window_runtime.controller.clear_all_toasts();
            shell.magpie_local.bootstrapped_auth_generation = None;
            shell.bootstrap_local_subscriptions_on_startup(cx);
            shell.send_rich_text_options("/vault-unlock anypw".into(), false, None, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(15))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(320))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(!shell.is_authenticated);
            assert!(shell.window_runtime.controller.current_toast.is_none());
            assert!(shell.magpie_local.latest_state.is_none());
        })
        .unwrap();

    return_valid_envelope.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: CID_FABUSHI,
                    peer_name: "Fabushi".into(),
                    avatar_initials: "FB".into(),
                    avatar_color: 0x8F59F9,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.dialogs.push(DialogItemModel {
                    id: 102,
                    peer_name: "Group 102".into(),
                    avatar_initials: "G2".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: false,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "Groups".into(),
                });
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(320))
        .await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.window_runtime.controller.clear_all_toasts();
            shell.send_rich_text_options("/vault-unlock wrong_passphrase".into(), false, None, cx);
            shell.sidebar.update(cx, |sb, _| {
                sb.delete_dialog(CID_FABUSHI);
                sb.select_dialog(102);
            });
            shell.sync_selected_chat_inspector_and_stage(102, cx);
            shell.stage.update(cx, |st, _| {
                st.chat_histories.remove(&CID_FABUSHI);
            });
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(160))
        .await;

    shell
        .read_with(&app, |shell, cx| {
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Chat 101 unlock_res Err toast leaked onto Chat 102: {:?}",
                shell.window_runtime.controller.current_toast
            );
            assert!(
                !shell.stage.read(cx).chat_histories.contains_key(&CID_FABUSHI),
                "deleted Chat 101 must not have chat_histories resurrected by vault notice"
            );
        })
        .unwrap();
}

// [T5-44] GAP-R11-1: TelegramClient::new in-memory OutboxStore, FileTransferStore, and E2eeStore
// are cleared on logout() and direct user switch, preventing cross-user outbox impersonation.
#[test]
fn t5_44_gap_r11_1_in_memory_outbox_transfer_e2ee_cleared_on_logout_and_switch() {
    run_in_headless_subprocess(
        "t5_44_gap_r11_1_in_memory_outbox_transfer_e2ee_cleared_on_logout_and_switch",
    );
}

async fn t5_44_body(app: AsyncApp) {
    use gpui_telegram_client::e2ee::DoubleRatchetSession;
    use gpui_telegram_client::media_transfer::{FileTransferRecord, TransferStatus};
    use gpui_telegram_client::net::RichSendMessageRequest;
    use parking_lot::Mutex;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};

    let send_should_succeed = Arc::new(AtomicBool::new(false));
    let send_should_succeed_srv = Arc::clone(&send_should_succeed);
    let recorded_sends: Arc<Mutex<Vec<(Option<String>, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let recorded_sends_srv = Arc::clone(&recorded_sends);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock outbox server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let send_should_succeed_srv = Arc::clone(&send_should_succeed_srv);
            let recorded_sends_srv = Arc::clone(&recorded_sends_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                let auth = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()).unwrap_or_default());
                let body_part = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();

                let (status_line, body) = if first_line.contains("/api/v1/messages/send") {
                    recorded_sends_srv.lock().push((auth, body_part));
                    if send_should_succeed_srv.load(Ordering::SeqCst) {
                        (
                            "HTTP/1.1 200 OK",
                            r#"{"message_id":9001,"id":9001,"pts":10,"pts_count":1,"duplicate":false}"#
                                .to_string(),
                        )
                    } else {
                        (
                            "HTTP/1.1 500 Internal Server Error",
                            r#"{"error":"transient_500"}"#.to_string(),
                        )
                    }
                } else if first_line.contains("/messages") {
                    ("HTTP/1.1 200 OK", r#"[]"#.to_string())
                } else if first_line.contains("/api/v1/chats") {
                    (
                        "HTTP/1.1 200 OK",
                        r#"[{"chat_id":102,"title":"Shared Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000005000}]"#
                            .to_string(),
                    )
                } else {
                    ("HTTP/1.1 200 OK", r#"{}"#.to_string())
                };
                let resp = format!(
                    "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            });
        }
    });

    let mut cfg = ClientConfig::default();
    cfg.server_url = format!("http://{addr}");
    let client = Arc::new(TelegramClient::new(cfg));
    assert!(
        client.local_store.cache_dir().is_none(),
        "TelegramClient::new must exercise the in-memory store path"
    );

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .unwrap();

    // Part 1: Authenticate as User A, queue outbox + transfer + E2EE session, then logout() and switch to User B.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 1001,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    let send_req_a = RichSendMessageRequest {
        chat_id: 102,
        text: Some("user-a-forwarded-secret".into()),
        content: None,
        entities: Vec::new(),
        reply_to: None,
        quote_text: None,
        topic_id: None,
        client_nonce: Some(77001),
        forward_from_chat_id: None,
        forward_from_msg_id: None,
        silent: false,
        scheduled_at_ms: None,
        inline_keyboard: None,
        send_as_peer_id: None,
        skip_server_bot_reply: false,
    };
    let (tx1, rx1) = futures::channel::oneshot::channel();
    {
        let c = client.clone();
        client.runtime().spawn(async move {
            let res = c.send_message_durable(send_req_a).await;
            let _ = tx1.send(res);
        });
    }
    let res1 = rx1.await.expect("send_message_durable oneshot");
    assert!(res1.is_err(), "expected HTTP 500 to queue message in outbox");

    client
        .chunked_transfer_manager()
        .store()
        .insert(&FileTransferRecord {
            transfer_id: "xfer-user-a".into(),
            upload_id: Some("up-a".into()),
            direction: "upload".into(),
            file_path: "/tmp/secret-a.bin".into(),
            filename: "secret-a.bin".into(),
            mime_type: "application/octet-stream".into(),
            total_size: 1024,
            part_size: 512,
            total_parts: 2,
            file_sha256: "abc".into(),
            chunk_hashes: vec!["h1".into(), "h2".into()],
            uploaded_parts: vec![true, false],
            status: TransferStatus::InProgress,
            server_file_id: None,
            last_error: None,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        })
        .unwrap();
    let _ = client.e2ee_manager().store().get_or_create_identity().unwrap();
    let dr_sess = DoubleRatchetSession::init_alice([7u8; 32], [9u8; 32], b"ad-a".to_vec()).unwrap();
    client
        .e2ee_manager()
        .store()
        .save_session(999, "dev-a", &dr_sess)
        .unwrap();

    assert_eq!(client.outbox_worker().store().pending_count(), 1);
    assert!(client.chunked_transfer_manager().store().get("xfer-user-a").unwrap().is_some());
    assert!(client.e2ee_manager().store().get_session(999, "dev-a").unwrap().is_some());

    // Logout must clear all in-memory stores immediately.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
        })
        .unwrap();

    assert_eq!(client.outbox_worker().store().pending_count(), 0);
    assert!(client.chunked_transfer_manager().store().get("xfer-user-a").unwrap().is_none());
    assert!(client.e2ee_manager().store().get_session(999, "dev-a").unwrap().is_none());

    // Switch to User B and allow 200 OK; flush must send 0 User A messages under tok-b.
    recorded_sends.lock().clear();
    send_should_succeed.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 2002,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    let (tx_flush1, rx_flush1) = futures::channel::oneshot::channel();
    {
        let c = client.clone();
        client.runtime().spawn(async move {
            let res = c.outbox_worker().notify_reconnected_and_flush().await;
            let _ = tx_flush1.send(res);
        });
    }
    let flushed1 = rx_flush1.await.unwrap().unwrap();
    assert!(flushed1.is_empty(), "expected 0 flushed records after logout, got {flushed1:?}");
    {
        let sends = recorded_sends.lock();
        assert!(
            sends
                .iter()
                .all(|(auth, body)| auth.as_deref() != Some("Bearer tok-b")
                    && !body.contains("user-a-forwarded-secret")),
            "User B must never transmit User A's queued message: {sends:?}"
        );
    }

    // Part 2: Direct is_user_switch (User B -> User C without explicit logout()).
    send_should_succeed.store(false, Ordering::SeqCst);
    recorded_sends.lock().clear();
    let send_req_b = RichSendMessageRequest {
        chat_id: 102,
        text: Some("user-b-direct-switch-secret".into()),
        content: None,
        entities: Vec::new(),
        reply_to: None,
        quote_text: None,
        topic_id: None,
        client_nonce: Some(77002),
        forward_from_chat_id: None,
        forward_from_msg_id: None,
        silent: false,
        scheduled_at_ms: None,
        inline_keyboard: None,
        send_as_peer_id: None,
        skip_server_bot_reply: false,
    };
    let (tx2, rx2) = futures::channel::oneshot::channel();
    {
        let c = client.clone();
        client.runtime().spawn(async move {
            let res = c.send_message_durable(send_req_b).await;
            let _ = tx2.send(res);
        });
    }
    assert!(rx2.await.unwrap().is_err());

    client
        .chunked_transfer_manager()
        .store()
        .insert(&FileTransferRecord {
            transfer_id: "xfer-user-a-2".into(),
            upload_id: Some("up-b".into()),
            direction: "upload".into(),
            file_path: "/tmp/secret-b.bin".into(),
            filename: "secret-b.bin".into(),
            mime_type: "application/octet-stream".into(),
            total_size: 2048,
            part_size: 512,
            total_parts: 4,
            file_sha256: "def".into(),
            chunk_hashes: vec!["h1".into()],
            uploaded_parts: vec![false],
            status: TransferStatus::Pending,
            server_file_id: None,
            last_error: None,
            created_at_ms: 2000,
            updated_at_ms: 2000,
        })
        .unwrap();
    let dr_sess_2 =
        DoubleRatchetSession::init_alice([8u8; 32], [10u8; 32], b"ad-b".to_vec()).unwrap();
    client
        .e2ee_manager()
        .store()
        .save_session(888, "dev-a2", &dr_sess_2)
        .unwrap();

    assert_eq!(client.outbox_worker().store().pending_count(), 1);
    assert!(client.chunked_transfer_manager().store().get("xfer-user-a-2").unwrap().is_some());
    assert!(client.e2ee_manager().store().get_session(888, "dev-a2").unwrap().is_some());

    // Direct switch to User C without calling logout().
    recorded_sends.lock().clear();
    send_should_succeed.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 3003,
                    username: "user_c".into(),
                    session_id: "sess-c".into(),
                    token: "tok-c".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    assert_eq!(client.outbox_worker().store().pending_count(), 0);
    assert!(client.chunked_transfer_manager().store().get("xfer-user-a-2").unwrap().is_none());
    assert!(client.e2ee_manager().store().get_session(888, "dev-a2").unwrap().is_none());

    let (tx_flush2, rx_flush2) = futures::channel::oneshot::channel();
    {
        let c = client.clone();
        client.runtime().spawn(async move {
            let res = c.outbox_worker().notify_reconnected_and_flush().await;
            let _ = tx_flush2.send(res);
        });
    }
    let flushed2 = rx_flush2.await.unwrap().unwrap();
    assert!(flushed2.is_empty());
    let sends = recorded_sends.lock();
    assert!(
        sends
            .iter()
            .all(|(auth, body)| auth.as_deref() != Some("Bearer tok-c")
                && !body.contains("user-b-direct-switch-secret")),
        "User C must never transmit User B's queued message on direct switch: {sends:?}"
    );
}

// [T5-45] GAP-R11-2, GAP-R11-3, GAP-R11-4:
// - refresh_chats never mutates rich_chats_by_id / chat_peer_user_ids before SessionRequestFence validation
// - AppShell::show_chat_toast & apply_local_fabushi_turn_error_to_shell gate on Chats panel + !is_plus_menu_open
// - SidebarContextAction::DeleteChat and ClearHistory fence refresh_chats across user switch
#[test]
fn t5_45_gap_r11_2_3_4_refresh_chats_fence_toast_guard_and_delete_chat_fence() {
    run_in_headless_subprocess(
        "t5_45_gap_r11_2_3_4_refresh_chats_fence_toast_guard_and_delete_chat_fence",
    );
}

async fn t5_45_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::settings::SettingsSectionType;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let delay_tok_a_chats = Arc::new(AtomicBool::new(true));
    let delay_tok_a_chats_srv = Arc::clone(&delay_tok_a_chats);
    let tok_b_get_chats_count = Arc::new(AtomicUsize::new(0));
    let tok_b_get_chats_count_srv = Arc::clone(&tok_b_get_chats_count);
    let tok_a_chats_arrived = Arc::new(AtomicUsize::new(0));
    let tok_a_chats_arrived_srv = Arc::clone(&tok_a_chats_arrived);
    let tok_a_chats_done = Arc::new(AtomicUsize::new(0));
    let tok_a_chats_done_srv = Arc::clone(&tok_a_chats_done);
    let track_tok_a_post_switch = Arc::new(AtomicBool::new(false));
    let track_tok_a_post_switch_srv = Arc::clone(&track_tok_a_post_switch);
    let tok_a_post_switch_chats_count = Arc::new(AtomicUsize::new(0));
    let tok_a_post_switch_chats_count_srv = Arc::clone(&tok_a_post_switch_chats_count);
    let leave_arrived = Arc::new(AtomicUsize::new(0));
    let leave_arrived_srv = Arc::clone(&leave_arrived);
    let leave_done = Arc::new(AtomicUsize::new(0));
    let leave_done_srv = Arc::clone(&leave_done);
    let clear_arrived = Arc::new(AtomicUsize::new(0));
    let clear_arrived_srv = Arc::clone(&clear_arrived);
    let clear_done = Arc::new(AtomicUsize::new(0));
    let clear_done_srv = Arc::clone(&clear_done);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock r11 server");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let delay_tok_a_chats_srv = Arc::clone(&delay_tok_a_chats_srv);
            let tok_b_get_chats_count_srv = Arc::clone(&tok_b_get_chats_count_srv);
            let tok_a_chats_arrived_srv = Arc::clone(&tok_a_chats_arrived_srv);
            let tok_a_chats_done_srv = Arc::clone(&tok_a_chats_done_srv);
            let track_tok_a_post_switch_srv = Arc::clone(&track_tok_a_post_switch_srv);
            let tok_a_post_switch_chats_count_srv = Arc::clone(&tok_a_post_switch_chats_count_srv);
            let leave_arrived_srv = Arc::clone(&leave_arrived_srv);
            let leave_done_srv = Arc::clone(&leave_done_srv);
            let clear_arrived_srv = Arc::clone(&clear_arrived_srv);
            let clear_done_srv = Arc::clone(&clear_done_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("");
                let path = parts.next().unwrap_or("").split('?').next().unwrap_or("");
                let auth = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()).unwrap_or_default())
                    .unwrap_or_default();

                let mut mark_done: Option<&Arc<AtomicUsize>> = None;
                let body = if path == "/api/v1/chats/101/leave" {
                    leave_arrived_srv.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(120));
                    mark_done = Some(&leave_done_srv);
                    r#"{"ok":true}"#.to_string()
                } else if path == "/api/v1/chats/101/clear" {
                    clear_arrived_srv.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(120));
                    mark_done = Some(&clear_done_srv);
                    r#"{"ok":true}"#.to_string()
                } else if path.contains("/messages") || path.starts_with("/api/v1/messages/history")
                {
                    r#"[]"#.to_string()
                } else if method == "GET" && path == "/api/v1/chats" {
                    if auth == "Bearer tok-a" {
                        tok_a_chats_arrived_srv.fetch_add(1, Ordering::SeqCst);
                        if track_tok_a_post_switch_srv.load(Ordering::SeqCst) {
                            tok_a_post_switch_chats_count_srv.fetch_add(1, Ordering::SeqCst);
                        }
                        if delay_tok_a_chats_srv.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(150));
                        }
                        mark_done = Some(&tok_a_chats_done_srv);
                        r#"[{"chat_id":555,"title":"UserA Secret DM","peer_user_id":999888,"chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000005000}]"#
                            .to_string()
                    } else {
                        if auth == "Bearer tok-b" {
                            tok_b_get_chats_count_srv.fetch_add(1, Ordering::SeqCst);
                        }
                        r#"[{"chat_id":555,"title":"UserB Clean DM","peer_user_id":777666,"chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000005000}]"#
                            .to_string()
                    }
                } else if path == "/api/v1/chats/rich" {
                    r#"[]"#.to_string()
                } else if path.starts_with("/api/v1/chats/") {
                    if path.ends_with("/members")
                        || path.ends_with("/invites")
                        || path.ends_with("/pins")
                    {
                        r#"[]"#.to_string()
                    } else {
                        r#"{}"#.to_string()
                    }
                } else {
                    r#"{}"#.to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                if let Some(counter) = mark_done {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });

    let client = new_isolated_client(format!("http://{addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Sub-case A (GAP-R11-2): delayed User A refresh_chats must not poison logged-out shell or User B.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 1001,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_a_chats_arrived.load(Ordering::SeqCst) >= 1).await,
        "Sub-case A1 User A /api/v1/chats did not arrive"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_a_chats_done.load(Ordering::SeqCst) >= 1).await,
        "Sub-case A1 User A /api/v1/chats did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    shell
        .read_with(&app, |shell, _| {
            assert!(
                shell.session_sync.rich_chats_by_id.get(&555).is_none(),
                "logged-out shell poisoned by delayed User A rich_chats_by_id"
            );
            assert!(
                shell.session_sync.chat_peer_user_ids.get(&555).is_none(),
                "logged-out shell poisoned by delayed User A chat_peer_user_ids"
            );
        })
        .unwrap();

    // Direct switch User A -> User B while User A refresh_chats is in-flight.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 1001,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_a_chats_arrived.load(Ordering::SeqCst) >= 2).await,
        "Sub-case A2 User A /api/v1/chats did not arrive"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 2002,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            tok_a_chats_done.load(Ordering::SeqCst) >= 2
                && shell
                    .read_with(&app, |s, _| s.session_sync.rich_chats_by_id.contains_key(&555))
                    .unwrap_or(false)
        })
        .await,
        "Sub-case A2 User B rich_chats_by_id did not populate after User A response completed"
    );

    shell
        .read_with(&app, |shell, _| {
            let rich = shell
                .session_sync
                .rich_chats_by_id
                .get(&555)
                .expect("User B rich chat 555 must be populated");
            assert_eq!(rich.title, "UserB Clean DM");
            assert_eq!(shell.session_sync.chat_peer_user_ids.get(&555), Some(&777666));
        })
        .unwrap();

    // Sub-case B (GAP-R11-3): show_chat_toast & apply_local_fabushi_turn_error_to_shell gated on Chats + !is_plus_menu_open.
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| {
                if !sb.dialogs.iter().any(|d| d.id == CID_FABUSHI) {
                    sb.dialogs.push(DialogItemModel {
                        id: CID_FABUSHI,
                        peer_name: "Fabushi".into(),
                        avatar_initials: "FB".into(),
                        avatar_color: 0x8F59F9,
                        last_message: String::new(),
                        timestamp: String::new(),
                        unread_count: 0,
                        is_muted: false,
                        is_pinned: false,
                        is_online: true,
                        typing_indicator: None,
                        delivery_status: DialogDeliveryStatus::Read,
                        folder: "All".into(),
                    });
                }
                sb.select_dialog(CID_FABUSHI);
            });
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            assert_eq!(shell.stage.read(cx).active_chat_id, Some(CID_FABUSHI));
            shell.window_runtime.controller.clear_all_toasts();

            // 1. Settings panel active -> no toast
            shell.navigate_settings_to(SettingsSectionType::Main, cx);
            assert_eq!(shell.active_panel, ActiveWorkspacePanel::Settings);
            shell.apply_local_fabushi_turn_error_to_shell(
                CID_FABUSHI,
                UID_FABUSHI as i64,
                "Fabushi",
                "PROVIDER_ERROR: missing key",
                cx,
            );
            shell.show_chat_toast(CID_FABUSHI, "direct chat toast in settings", 3200, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "chat toast leaked into Settings"
            );

            // 2. Chats panel + plus compose open -> no toast
            shell.active_panel = ActiveWorkspacePanel::Chats;
            shell.sidebar.update(cx, |sb, _| sb.is_plus_menu_open = true);
            shell.apply_local_fabushi_turn_error_to_shell(
                CID_FABUSHI,
                UID_FABUSHI as i64,
                "Fabushi",
                "PROVIDER_ERROR: missing key",
                cx,
            );
            shell.show_chat_toast(CID_FABUSHI, "direct chat toast in + compose", 3200, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "chat toast leaked into + compose"
            );

            // 3. Chats panel + plus compose closed -> toast shows
            shell.sidebar.update(cx, |sb, _| sb.is_plus_menu_open = false);
            shell.apply_local_fabushi_turn_error_to_shell(
                CID_FABUSHI,
                UID_FABUSHI as i64,
                "Fabushi",
                "PROVIDER_ERROR: missing key",
                cx,
            );
            let t = shell
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected toast when active chat is visible");
            assert_eq!(t.chat_id, Some(CID_FABUSHI));
            shell.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    // Sub-case C (GAP-R11-4): DeleteChat & ClearHistory fence refresh_chats across user switch.
    delay_tok_a_chats.store(false, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 1001,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || tok_a_chats_done.load(Ordering::SeqCst) >= 3).await,
        "Sub-case C User A initial /api/v1/chats did not complete"
    );
    pump_until_settled(&app, 50, || tok_a_chats_arrived.load(Ordering::SeqCst)).await;

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.handle_sidebar_context_action(SidebarContextAction::DeleteChat(101), cx);
            shell.handle_sidebar_context_action(SidebarContextAction::ClearHistory(101), cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            leave_arrived.load(Ordering::SeqCst) >= 1 && clear_arrived.load(Ordering::SeqCst) >= 1
        })
        .await,
        "DeleteChat/ClearHistory did not reach server before user switch"
    );

    tok_b_get_chats_count.store(0, Ordering::SeqCst);
    tok_a_post_switch_chats_count.store(0, Ordering::SeqCst);
    track_tok_a_post_switch.store(true, Ordering::SeqCst);

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 2002,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || {
            leave_done.load(Ordering::SeqCst) >= 1
                && clear_done.load(Ordering::SeqCst) >= 1
                && tok_b_get_chats_count.load(Ordering::SeqCst) >= 2
        })
        .await,
        "leave/clear or User B /api/v1/chats did not complete"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    assert_eq!(
        tok_b_get_chats_count.load(Ordering::SeqCst),
        2,
        "User A DeleteChat/ClearHistory must not trigger extra refresh_chats under User B (expected 2 = get_chats + list_chats_rich)"
    );
    assert_eq!(
        tok_a_post_switch_chats_count.load(Ordering::SeqCst),
        0,
        "User A DeleteChat/ClearHistory must not trigger tok-a refresh_chats after switch"
    );
}

// [T5-46] r11_2 GAP-R11-1: Coding Host per-user db_path/worktrees_root isolation,
// logout/user-switch resets, and CodingHostCommand ownership fences.
#[test]
fn t5_46_coding_host_per_user_isolation_and_logout_fences() {
    run_in_headless_subprocess("t5_46_coding_host_per_user_isolation_and_logout_fences");
}

async fn t5_46_body(app: AsyncApp) {
    use gpui_telegram_coding::store::{CodingStore, ThreadRecord};
    use gpui_telegram_desktop::views::app_shell::shell_coding_host::{
        should_accept_coding_host_command, user_coding_host_dir,
    };
    use gpui_telegram_proto::{
        CodingAgentBotConfig, CodingControlAction, CodingHostCommand, CodingHostEvent,
        CodingPermissionMode, CodingProviderKind, DetectedCodingProvider,
    };

    let tmp_ws = tempfile::TempDir::new().unwrap();
    let alice_repo = tmp_ws.path().join("alice-repo");
    let bob_repo = tmp_ws.path().join("bob-repo");
    std::fs::create_dir_all(&alice_repo).unwrap();
    std::fs::create_dir_all(&bob_repo).unwrap();

    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .unwrap();

    // User A (1001)
    let rt_a = shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 1001,
                    username: "alice".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            let rt_a = shell
                .ensure_coding_host_runtime(cx)
                .expect("User A coding runtime");
            assert_eq!(rt_a.owner_user_id(), 1001);
            assert_eq!(
                rt_a.db_path(),
                user_coding_host_dir(&shell.preferences_dir, 1001).join("coding_host_state.db")
            );
            assert_eq!(
                rt_a.worktrees_root(),
                user_coding_host_dir(&shell.preferences_dir, 1001).join("coding-worktrees")
            );
            shell.coding_host.detected_providers.push(DetectedCodingProvider {
                kind: CodingProviderKind::ClaudeCode,
                display_name: "Claude Code".into(),
                binary_path: Some("/usr/local/bin/claude".into()),
                version: Some("1.0".into()),
                installed: true,
                recommended_username: "claude_code_bot".into(),
                supports_acp: true,
                login_hint: "claude login".into(),
            });
            shell.coding_host.registered_for_auth_gen = Some(shell.session_sync.auth_generation);
            rt_a
        })
        .unwrap();

    {
        let store = CodingStore::open(rt_a.db_path()).unwrap();
        store
            .upsert_thread(&ThreadRecord {
                owner_user_id: 1001,
                bot_user_id: 500,
                chat_id: 102,
                topic_id: 0,
                thread_id: "thr_u1001_500_102_0".into(),
                provider_session_id: Some("secret_claude_sess_alice".into()),
                workspace_path: alice_repo.to_string_lossy().into_owned(),
                worktree_path: Some("/tmp/wt-alice".into()),
                branch_name: Some("fabushi/alice".into()),
                use_worktree: true,
                active_model: Some("claude-opus-4-6".into()),
                permission_mode: CodingPermissionMode::AutoEdit,
                turn_count: 17,
                last_summary: Some("alice secret work".into()),
                updated_at_ms: 1000,
                bot_provider_kind: Some(CodingProviderKind::ClaudeCode),
                bot_workspace_path: alice_repo.to_string_lossy().into_owned(),
                bot_use_worktree: true,
                bot_active_model: Some("claude-opus-4-6".into()),
                bot_permission_mode: Some(CodingPermissionMode::AutoEdit),
            })
            .unwrap();
    }

    let alice_cfg = CodingAgentBotConfig {
        provider_kind: CodingProviderKind::ClaudeCode,
        workspace_path: alice_repo.to_string_lossy().into_owned(),
        use_worktree: true,
        permission_mode: CodingPermissionMode::AutoEdit,
        ..Default::default()
    };
    let (tx_a, rx_a) = futures::channel::oneshot::channel();
    {
        let rt = rt_a.clone();
        client.runtime().spawn(async move {
            let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel();
            rt.handle_command(
                CodingHostCommand::Control {
                    req_id: "req_alice_status".into(),
                    owner_user_id: 1001,
                    target_device_id: None,
                    bot_user_id: 500,
                    chat_id: 102,
                    topic_id: None,
                    is_forum_topic: false,
                    config: alice_cfg,
                    action: CodingControlAction::GetStatus,
                },
                etx,
            )
            .await;
            let _ = tx_a.send(erx.recv().await);
        });
    }
    match rx_a.await.unwrap().unwrap() {
        CodingHostEvent::ControlResult {
            ok, markdown_reply, ..
        } => {
            assert!(ok);
            assert!(markdown_reply.contains("alice-repo"));
            assert!(markdown_reply.contains("Turns: `17`"));
            assert!(markdown_reply.contains("secret_claude_sess_alice"));
        }
        other => panic!("unexpected event for alice: {other:?}"),
    }

    // Logout must clear runtime, detected_providers, registered_for_auth_gen, and reject commands.
    let probe_cmd_alice = CodingHostCommand::ProbeProviders {
        req_id: "probe_a".into(),
        owner_user_id: 1001,
        target_device_id: None,
    };
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            assert!(shell.coding_host.runtime.is_none());
            assert!(shell.coding_host.detected_providers.is_empty());
            assert!(shell.coding_host.registered_for_auth_gen.is_none());
            assert!(shell.ensure_coding_host_runtime(cx).is_none());
            assert!(!should_accept_coding_host_command(
                shell.is_authenticated,
                shell.session_sync.user_id,
                &shell.client.device_id(),
                &probe_cmd_alice,
            ));
        })
        .unwrap();

    // User B (2002)
    let rt_b = shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 2002,
                    username: "bob".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            let rt_b = shell
                .ensure_coding_host_runtime(cx)
                .expect("User B coding runtime");
            assert_eq!(rt_b.owner_user_id(), 2002);
            assert_ne!(rt_b.db_path(), rt_a.db_path());
            assert_ne!(rt_b.worktrees_root(), rt_a.worktrees_root());
            assert_eq!(
                rt_b.db_path(),
                user_coding_host_dir(&shell.preferences_dir, 2002).join("coding_host_state.db")
            );
            rt_b
        })
        .unwrap();

    let bob_cfg = CodingAgentBotConfig {
        provider_kind: CodingProviderKind::ClaudeCode,
        workspace_path: bob_repo.to_string_lossy().into_owned(),
        use_worktree: false,
        permission_mode: CodingPermissionMode::Ask,
        ..Default::default()
    };
    let (tx_b, rx_b) = futures::channel::oneshot::channel();
    {
        let rt = rt_b.clone();
        client.runtime().spawn(async move {
            let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel();
            rt.handle_command(
                CodingHostCommand::Control {
                    req_id: "req_bob_status".into(),
                    owner_user_id: 2002,
                    target_device_id: None,
                    bot_user_id: 500,
                    chat_id: 102,
                    topic_id: None,
                    is_forum_topic: false,
                    config: bob_cfg.clone(),
                    action: CodingControlAction::GetStatus,
                },
                etx,
            )
            .await;
            let _ = tx_b.send(erx.recv().await);
        });
    }
    match rx_b.await.unwrap().unwrap() {
        CodingHostEvent::ControlResult {
            ok, markdown_reply, ..
        } => {
            assert!(ok);
            assert!(markdown_reply.contains("bob-repo"), "{markdown_reply}");
            assert!(markdown_reply.contains("Turns: `0`"), "{markdown_reply}");
            assert!(markdown_reply.contains("Session: `(none)`"), "{markdown_reply}");
            assert!(markdown_reply.contains("Worktree: `(none)`"), "{markdown_reply}");
            assert!(!markdown_reply.contains("alice-repo"), "{markdown_reply}");
            assert!(!markdown_reply.contains("wt-alice"), "{markdown_reply}");
            assert!(!markdown_reply.contains("Turns: `17`"), "{markdown_reply}");
            assert!(!markdown_reply.contains("secret_claude_sess_alice"), "{markdown_reply}");
        }
        other => panic!("unexpected event for bob: {other:?}"),
    }

    // rt_b rejects owner_user_id 1001 and shell.handle_coding_host_command ignores User A commands while User B is active.
    let (tx_bad, rx_bad) = futures::channel::oneshot::channel();
    {
        let rt = rt_b.clone();
        client.runtime().spawn(async move {
            let (etx, mut erx) = tokio::sync::mpsc::unbounded_channel();
            rt.handle_command(
                CodingHostCommand::Control {
                    req_id: "req_forged_alice".into(),
                    owner_user_id: 1001,
                    target_device_id: None,
                    bot_user_id: 500,
                    chat_id: 102,
                    topic_id: None,
                    is_forum_topic: false,
                    config: CodingAgentBotConfig::default(),
                    action: CodingControlAction::GetStatus,
                },
                etx,
            )
            .await;
            let _ = tx_bad.send(erx.recv().await);
        });
    }
    match rx_bad.await.unwrap().unwrap() {
        CodingHostEvent::ControlResult {
            ok, markdown_reply, ..
        } => {
            assert!(!ok);
            assert!(markdown_reply.contains("Unauthorized"), "{markdown_reply}");
        }
        other => panic!("unexpected forged owner response: {other:?}"),
    }

    shell
        .update(&mut app.clone(), |shell, cx| {
            assert!(!should_accept_coding_host_command(
                shell.is_authenticated,
                shell.session_sync.user_id,
                &shell.client.device_id(),
                &probe_cmd_alice,
            ));
            shell.handle_coding_host_command(probe_cmd_alice.clone(), cx);

            // Direct User B -> User C (3003) switch without explicit logout().
            shell.coding_host.detected_providers.push(DetectedCodingProvider {
                kind: CodingProviderKind::Codex,
                display_name: "Codex".into(),
                binary_path: Some("/usr/local/bin/codex".into()),
                version: Some("1.0".into()),
                installed: true,
                recommended_username: "codex_bot".into(),
                supports_acp: true,
                login_hint: "codex login".into(),
            });
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 3003,
                    username: "carol".into(),
                    session_id: "sess-c".into(),
                    token: "tok-c".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            assert!(
                shell.coding_host.detected_providers.is_empty(),
                "direct user switch must clear coding_host.detected_providers"
            );
            let rt_c = shell
                .ensure_coding_host_runtime(cx)
                .expect("User C coding runtime");
            assert_eq!(rt_c.owner_user_id(), 3003);
            assert_eq!(
                rt_c.db_path(),
                user_coding_host_dir(&shell.preferences_dir, 3003).join("coding_host_state.db")
            );
        })
        .unwrap();
}

// [T5-47] GAP-R12B-1, GAP-R12B-3 & r12b_1 Contacts modal toast suppression:
// 1. spawn_local_fabushi_turn across logout + User B switch AND direct User A -> User B switch
//    never archives User A's confidential reply under User B's token or mutates User B's stage;
//    positive control confirms User B staying logged in does archive under tok-b.
// 2. Same-turn cx.spawn mutations (DeleteChat, ClearHistory, ConfirmLeaveOrDeleteChat,
//    ConfirmClearChatHistory, DeleteFolder) followed immediately by logout+switch or direct switch
//    abort at ClientCallFence preflight with 0 requests sent under tok-b (or tok-a).
// 3. Contacts modal open suppresses chat-scoped toast; closing it restores toast delivery.
#[test]
fn t5_47_gap_r12b_1_3_fabushi_archive_and_dynamic_token_cx_spawn_fences() {
    run_in_headless_subprocess(
        "t5_47_gap_r12b_1_3_fabushi_archive_and_dynamic_token_cx_spawn_fences",
    );
}

async fn t5_47_body(app: AsyncApp) {
    use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
    use gpui_telegram_agent::hermes::providers::ProviderConfig;
    use gpui_telegram_desktop::views::settings::SettingsRouterEvent;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let archive_hits: Arc<std::sync::Mutex<Vec<(String, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let archive_hits_srv = Arc::clone(&archive_hits);
    let mut_hits: Arc<std::sync::Mutex<Vec<(String, String, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut_hits_srv = Arc::clone(&mut_hits);

    let tg_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock tg server");
    let tg_addr = tg_listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = tg_listener.accept() {
            let archive_hits_srv = Arc::clone(&archive_hits_srv);
            let mut_hits_srv = Arc::clone(&mut_hits_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("").to_string();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let auth_hdr = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .to_string();
                let body_str = req
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or("")
                    .to_string();

                if path.contains("/archive-local-bot-reply") {
                    archive_hits_srv.lock().unwrap().push((auth_hdr, body_str));
                    let body = r#"{"message_id":999,"pts":55}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path.contains("/leave")
                    || path.contains("/clear")
                    || path.starts_with("/api/v1/folders/")
                    || path.contains("/ttl")
                    || path.contains("/users/me/profile")
                {
                    mut_hits_srv.lock().unwrap().push((method, path, auth_hdr));
                    let body = r#"{"success":true}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path.contains("/rich") {
                    let body = r#"{"error":"not_found"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path.ends_with("/api/v1/chats") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
                let _ = stream.flush();
            });
        }
    });

    let openai_started = Arc::new(AtomicUsize::new(0));
    let openai_done = Arc::new(AtomicUsize::new(0));
    let hold_openai = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let openai_started_srv = Arc::clone(&openai_started);
    let openai_done_srv = Arc::clone(&openai_done);
    let hold_openai_srv = Arc::clone(&hold_openai);
    let openai_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock openai");
    let openai_addr = openai_listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = openai_listener.accept() {
            let openai_started_srv = Arc::clone(&openai_started_srv);
            let openai_done_srv = Arc::clone(&openai_done_srv);
            let hold_openai_srv = Arc::clone(&hold_openai_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                let _ = stream.read(&mut buf);
                openai_started_srv.fetch_add(1, Ordering::SeqCst);
                while hold_openai_srv.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let body = r#"{"id":"cmpl_1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"USER_A_CONFIDENTIAL_AI_REPLY"},"finish_reason":"stop"}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
                openai_done_srv.fetch_add(1, Ordering::SeqCst);
            });
        }
    });

    let client = new_isolated_client(format!("http://{tg_addr}"));
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    let host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("mock-gpt").expect("ephemeral fabushi host"),
    );
    for uid in [1001u64, 202u64] {
        let uid_str = uid.to_string();
        let router = host.core_engine().providers_for_user(&uid_str);
        router.register_provider(ProviderConfig::openai_compatible(
            "mock_openai",
            "Mock OpenAI",
            format!("http://{openai_addr}/v1"),
            "mock-gpt",
            "sk-mock",
        ));
        let _ = router.set_active("mock_openai", Some("mock-gpt"));
    }

    let make_bot_summary = |owner_id: u64| FabushiBotSummaryDto {
        agent_id: "fabushi_bot".into(),
        bot_user_id: UID_FABUSHI as i64,
        owner_user_id: owner_id as i64,
        username: "fabushi_bot".into(),
        display_name: "Fabushi".into(),
        description: "desc".into(),
        title: "Fabushi".into(),
        avatar_shape: String::new(),
        avatar_color: String::new(),
        preferred_model: "mock-gpt".into(),
        thinking_level: String::new(),
        approval_mode: String::new(),
        toolset_preset: "safe".into(),
        notify_on_agent_updates: true,
        hidden_from_sidebar: false,
        memory_entries_count: 0,
        skills_count: 0,
        execution_state: "idle".into(),
        companion_chat_id: Some(CID_FABUSHI as i64),
        coding_agent: None,
    };

    // ------------------------------------------------------------------
    // Sub-case A: GAP-R12B-1 spawn_local_fabushi_turn cross-user archive leak
    // ------------------------------------------------------------------
    // Variant 1: logout() + User B login while OpenAI turn is in-flight
    hold_openai.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 1001,
                    username: "user_a".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.attach_local_fabushi_host(host.clone());
            shell.miniapps.fabushi_bots = vec![make_bot_summary(1001)];
            shell
                .session_sync
                .chat_peer_user_ids
                .insert(CID_FABUSHI, UID_FABUSHI);
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            let target = shell
                .resolve_fabushi_bot_target(CID_FABUSHI, cx)
                .expect("User A Fabushi target");
            shell.spawn_local_fabushi_turn(CID_FABUSHI, target, None, "secret prompt".into(), cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || openai_started.load(Ordering::SeqCst) >= 1).await,
        "mock OpenAI turn 1 did not start before logout"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.logout(cx);
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.attach_local_fabushi_host(host.clone());
            shell.miniapps.fabushi_bots = vec![make_bot_summary(202)];
            shell
                .session_sync
                .chat_peer_user_ids
                .insert(CID_FABUSHI, UID_FABUSHI);
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    hold_openai.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 4000, || openai_done.load(Ordering::SeqCst) >= 1).await,
        "mock OpenAI turn 1 did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;

    {
        let hits = archive_hits.lock().unwrap();
        assert!(
            !hits.iter().any(|(auth, _)| auth.contains("tok-b")),
            "Variant 1 must never archive User A reply under tok-b: {hits:?}"
        );
        assert!(
            !hits
                .iter()
                .any(|(auth, body)| auth.contains("tok-b") && body.contains("USER_A_CONFIDENTIAL_AI_REPLY")),
            "User A reply leaked into User B archive: {hits:?}"
        );
    }
    shell
        .read_with(&app, |shell, cx| {
            let st = shell.stage.read(cx);
            assert!(
                !st.messages.iter().any(|m| match &m.content {
                    MessageContent::Text { text, .. } => {
                        text.contains("USER_A_CONFIDENTIAL_AI_REPLY")
                    }
                    _ => false,
                }),
                "User A confidential reply must not appear in User B stage"
            );
        })
        .unwrap();

    // Variant 2: direct User A -> User B switch without explicit logout()
    archive_hits.lock().unwrap().clear();
    hold_openai.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 1001,
                    username: "user_a".into(),
                    session_id: "sess-a2".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.attach_local_fabushi_host(host.clone());
            shell.miniapps.fabushi_bots = vec![make_bot_summary(1001)];
            shell
                .session_sync
                .chat_peer_user_ids
                .insert(CID_FABUSHI, UID_FABUSHI);
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            let target = shell
                .resolve_fabushi_bot_target(CID_FABUSHI, cx)
                .expect("User A Fabushi target 2");
            shell.spawn_local_fabushi_turn(CID_FABUSHI, target, None, "secret prompt 2".into(), cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || openai_started.load(Ordering::SeqCst) >= 2).await,
        "mock OpenAI turn 2 did not start before user switch"
    );

    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "user_b".into(),
                    session_id: "sess-b2".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            shell.attach_local_fabushi_host(host.clone());
            shell.miniapps.fabushi_bots = vec![make_bot_summary(202)];
            shell
                .session_sync
                .chat_peer_user_ids
                .insert(CID_FABUSHI, UID_FABUSHI);
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    hold_openai.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 4000, || openai_done.load(Ordering::SeqCst) >= 2).await,
        "mock OpenAI turn 2 did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;

    assert!(
        !archive_hits
            .lock()
            .unwrap()
            .iter()
            .any(|(auth, _)| auth.contains("tok-b")),
        "Direct switch must never archive User A reply under tok-b: {:?}",
        archive_hits.lock().unwrap()
    );

    // Positive control: User B stays logged in -> DOES archive under tok-b
    archive_hits.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |shell, cx| {
            let target = shell
                .resolve_fabushi_bot_target(CID_FABUSHI, cx)
                .expect("User B Fabushi target");
            shell.spawn_local_fabushi_turn(CID_FABUSHI, target, None, "user b turn".into(), cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || {
            archive_hits
                .lock()
                .unwrap()
                .iter()
                .any(|(auth, _)| auth.contains("tok-b"))
        })
        .await,
        "Positive control: User B staying logged in must archive under tok-b"
    );

    // ------------------------------------------------------------------
    // Sub-case B: GAP-R12B-3 Same-turn cx.spawn dynamic require_token()
    //             mutations aborted at preflight_ui
    // ------------------------------------------------------------------
    for direct_only in [false, true] {
        mut_hits.lock().unwrap().clear();
        shell
            .update(&mut app.clone(), |shell, cx| {
                shell.apply_authenticated_session_with_vault(
                    AuthResponse {
                        user_id: 1001,
                        username: "user_a".into(),
                        session_id: "sess-a-mut".into(),
                        token: "tok-a".into(),
                        first_name: None,
                        created_at: None,
                        expires_at: None,
                    },
                    None,
                    cx,
                );
                shell.sidebar.update(cx, |sb, _| {
                    sb.dialogs.push(DialogItemModel {
                        id: 103,
                        peer_name: "Chat 103".into(),
                        avatar_initials: "C3".into(),
                        avatar_color: 0x4A90E2,
                        last_message: String::new(),
                        timestamp: String::new(),
                        unread_count: 0,
                        is_muted: false,
                        is_pinned: false,
                        is_online: true,
                        typing_indicator: None,
                        delivery_status: DialogDeliveryStatus::Read,
                        folder: "All".into(),
                    });
                    sb.dialogs.push(DialogItemModel {
                        id: 104,
                        peer_name: "Chat 104".into(),
                        avatar_initials: "C4".into(),
                        avatar_color: 0x4A90E2,
                        last_message: String::new(),
                        timestamp: String::new(),
                        unread_count: 0,
                        is_muted: false,
                        is_pinned: false,
                        is_online: true,
                        typing_indicator: None,
                        delivery_status: DialogDeliveryStatus::Read,
                        folder: "All".into(),
                    });
                });

                shell.handle_sidebar_context_action(SidebarContextAction::DeleteChat(101), cx);
                shell.handle_sidebar_context_action(SidebarContextAction::ClearHistory(102), cx);
                shell.sidebar.update(cx, |sb, _| sb.select_dialog(103));
                shell.sync_selected_chat_inspector_and_stage(103, cx);
                shell.handle_modal_outcome(ModalActionOutcome::ConfirmLeaveOrDeleteChat, cx);
                shell.sidebar.update(cx, |sb, _| sb.select_dialog(104));
                shell.sync_selected_chat_inspector_and_stage(104, cx);
                shell.handle_modal_outcome(
                    ModalActionOutcome::ConfirmClearChatHistory {
                        also_for_peer: false,
                    },
                    cx,
                );
                shell.handle_settings_router_event(&SettingsRouterEvent::DeleteFolder(7), cx);

                if !direct_only {
                    shell.logout(cx);
                }
                shell.apply_authenticated_session_with_vault(
                    AuthResponse {
                        user_id: 202,
                        username: "user_b".into(),
                        session_id: "sess-b-mut".into(),
                        token: "tok-b".into(),
                        first_name: None,
                        created_at: None,
                        expires_at: None,
                    },
                    None,
                    cx,
                );
            })
            .unwrap();

        app.background_executor()
            .timer(Duration::from_millis(120))
            .await;

        let hits = mut_hits.lock().unwrap();
        assert!(
            hits.is_empty(),
            "Same-turn cx.spawn mutations (direct_only={direct_only}) must abort at preflight_ui with 0 requests: {hits:?}"
        );
    }

    // ------------------------------------------------------------------
    // Sub-case C: r12b_1 Contacts modal toast suppression
    // ------------------------------------------------------------------
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.active_panel = ActiveWorkspacePanel::Chats;
            shell.sidebar.update(cx, |sb, _| {
                sb.select_dialog(CID_FABUSHI);
                sb.is_plus_menu_open = false;
                sb.active_creation_modal = None;
                sb.is_contacts_modal_open = true;
            });
            shell.stage.update(cx, |st, _| st.active_chat_id = Some(CID_FABUSHI));
            shell.overlays.is_passcode_locked = false;
            shell.window_runtime.controller.clear_all_toasts();
            shell.show_chat_toast(CID_FABUSHI, "hidden behind contacts", 2000, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_none(),
                "Contacts modal open must suppress chat-scoped toast"
            );
            shell.sidebar.update(cx, |sb, _| sb.is_contacts_modal_open = false);
            shell.show_chat_toast(CID_FABUSHI, "visible now", 2000, cx);
            assert!(
                shell.window_runtime.controller.current_toast.is_some(),
                "Closing Contacts modal must allow chat-scoped toast"
            );
        })
        .unwrap();
}

// [T5-49] GAP-R12B2-1: DesktopLocalIpcHandler updates its client handle on
// AuthWizardEvent::ServerUrlApplied so local IPC `status` / `user.status` / `chat.list`
// target the newly configured server and session instead of stale srv1.
#[test]
fn t5_49_desktop_local_ipc_handler_updates_on_server_url_applied() {
    run_in_headless_subprocess_with_ipc(
        "t5_49_desktop_local_ipc_handler_updates_on_server_url_applied",
    );
}

async fn t5_49_body(app: AsyncApp) {
    use gpui_telegram_proto::computer_contracts::{
        ComputerNodeDto, ComputerNodeKind, RemoteComputerCommandResult,
    };
    use std::io::{Read, Write};

    let spawn_mock_srv = |chat_id: u64, title: &'static str| {
        let hits: Arc<std::sync::Mutex<Vec<(String, String)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let hold_register_tokens: Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
            Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let register_arrivals: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let remote_exec_hits: Arc<std::sync::Mutex<Vec<(String, String)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let hits_srv = Arc::clone(&hits);
        let hold_reg_srv = Arc::clone(&hold_register_tokens);
        let reg_arr_srv = Arc::clone(&register_arrivals);
        let remote_hits_srv = Arc::clone(&remote_exec_hits);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock srv");
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let hits_srv = Arc::clone(&hits_srv);
                let hold_reg_srv = Arc::clone(&hold_reg_srv);
                let reg_arr_srv = Arc::clone(&reg_arr_srv);
                let remote_hits_srv = Arc::clone(&remote_hits_srv);
                std::thread::spawn(move || {
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("").to_string();
                    let path = first_line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let auth_hdr = req
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                        .map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()).unwrap_or_default())
                        .unwrap_or_default();
                    let body_str = req.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
                    hits_srv.lock().unwrap().push((auth_hdr.clone(), path.clone()));

                    if path == "/api/v1/chats" {
                        let body = format!(
                            r#"[{{"chat_id":{chat_id},"title":"{title}","chat_type":0,"is_group":false,"is_bot":false,"updated_at":1700000001000}}]"#
                        );
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if path == "/api/v1/computers/local/register" {
                        reg_arr_srv.lock().unwrap().push(auth_hdr.clone());
                        while hold_reg_srv.lock().unwrap().contains(&auth_hdr) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        let dto = ComputerNodeDto {
                            node_id: format!("local:user-{chat_id}-srv"),
                            owner_user_id: chat_id,
                            kind: ComputerNodeKind::LocalDevice,
                            display_name: format!("Host {chat_id}"),
                            is_current_device: true,
                            online: true,
                            ..Default::default()
                        };
                        let body = serde_json::to_string(&dto).unwrap();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if path == "/api/v1/computers/exec" {
                        let target_node = serde_json::from_str::<serde_json::Value>(body_str)
                            .ok()
                            .and_then(|v| v.get("node_id").and_then(|n| n.as_str()).map(str::to_string))
                            .unwrap_or_default();
                        remote_hits_srv
                            .lock()
                            .unwrap()
                            .push((auth_hdr, target_node.clone()));
                        let res = RemoteComputerCommandResult {
                            command_id: "cmd-remote-1".to_string(),
                            node_id: target_node,
                            ok: true,
                            exit_code: 0,
                            stdout: "remote-ok".to_string(),
                            stderr: String::new(),
                            entries: Vec::new(),
                            frame_data_url: None,
                            executed_by: "mock-server".to_string(),
                            duration_ms: 1,
                        };
                        let body = serde_json::to_string(&res).unwrap();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if path == "/api/v1/user-vault/envelope" {
                        let body = r#"{"envelope":null}"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if path.contains("/rich") {
                        let body = r#"{"error":"not_found"}"#;
                        let resp = format!(
                            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else {
                        let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                        let _ = stream.write_all(resp.as_bytes());
                    }
                    let _ = stream.flush();
                });
            }
        });
        (
            format!("http://{addr}"),
            hits,
            hold_register_tokens,
            register_arrivals,
            remote_exec_hits,
        )
    };

    let (
        srv1_url,
        srv1_hits,
        hold_srv1_tokens,
        srv1_register_arrivals,
        srv1_remote_exec_hits,
    ) = spawn_mock_srv(1101, "Srv1 Alice Chat");
    let (
        srv2_url,
        srv2_hits,
        hold_srv2_tokens,
        srv2_register_arrivals,
        _srv2_remote_exec_hits,
    ) = spawn_mock_srv(2202, "Srv2 Bob Chat");

    let mut cfg = ClientConfig::default();
    cfg.server_url = srv1_url.clone();
    let client1 = Arc::new(TelegramClient::new(cfg));
    let rt_handle = client1.runtime().clone();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client1.clone(), cx)))
        .unwrap();

    let sock_path = PathBuf::from(
        std::env::var_os("FABUSHI_IPC_SOCKET").expect("FABUSHI_IPC_SOCKET env must be set"),
    );
    let ipc = LocalIpcClient::new(sock_path.clone());

    let ipc_call = |method: &'static str, params: serde_json::Value| {
        let ipc = ipc.clone();
        let rt = rt_handle.clone();
        async move {
            rt.spawn(async move { ipc.call(method, params).await })
                .await
                .expect("tokio ipc join")
        }
    };

    let mut ipc_ready = false;
    for _ in 0..150 {
        if sock_path.exists() {
            if let Ok(r) = ipc_call("status", serde_json::json!({})).await {
                if r.success {
                    ipc_ready = true;
                    break;
                }
            }
        }
        app.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    assert!(ipc_ready, "Desktop LocalIpcServer failed to become ready");

    // Step 1: User A on srv1
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(
                AuthResponse {
                    user_id: 1001,
                    username: "alice_srv1".into(),
                    session_id: "sess-a1".into(),
                    token: "tok_A1".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                cx,
            );
        })
        .unwrap();

    let st_a = ipc_call("status", serde_json::json!({})).await.unwrap();
    assert!(st_a.success);
    let st_a_data = st_a.data.unwrap();
    assert_eq!(st_a_data["authenticated"], true);
    assert_eq!(st_a_data["user_id"], 1001);
    assert_eq!(st_a_data["username"], "alice_srv1");

    let cl_a = ipc_call("chat.list", serde_json::json!({})).await.unwrap();
    assert!(cl_a.success);
    let cl_a_data = cl_a.data.unwrap();
    assert_eq!(cl_a_data["chats"][0]["chat_id"], 1101);
    assert_eq!(cl_a_data["chats"][0]["title"], "Srv1 Alice Chat");
    assert!(
        srv1_hits
            .lock()
            .unwrap()
            .iter()
            .any(|(auth, path)| auth == "Bearer tok_A1" && path == "/api/v1/chats"),
        "srv1 must record Bearer tok_A1 /api/v1/chats"
    );
    assert!(srv2_hits.lock().unwrap().is_empty());
    assert!(
        pump_until(&app, 3000, || {
            srv1_hits
                .lock()
                .unwrap()
                .iter()
                .any(|(a, p)| a == "Bearer tok_A1" && p == "/api/v1/mcp/grants")
        })
        .await,
        "Step 1 Alice startup sync on srv1 did not complete"
    );

    // Step 2: logout + AuthWizardEvent::ServerUrlApplied(srv2_url)
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv2_url.clone()), cx);
        })
        .unwrap();

    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.auth_wizard.read(cx).client.config.server_url, srv2_url);
        })
        .unwrap();

    let st_out = ipc_call("status", serde_json::json!({})).await.unwrap();
    assert!(st_out.success);
    let st_out_data = st_out.data.unwrap();
    assert_eq!(st_out_data["authenticated"], false);
    assert!(st_out_data["user_id"].is_null());

    let ust_out = ipc_call("user.status", serde_json::json!({})).await.unwrap();
    assert!(ust_out.success);
    let ust_out_data = ust_out.data.unwrap();
    assert_eq!(ust_out_data["server_url"], srv2_url);
    assert_eq!(ust_out_data["authenticated"], false);

    let cl_out = ipc_call("chat.list", serde_json::json!({})).await.unwrap();
    assert!(!cl_out.success);
    assert_eq!(cl_out.error.unwrap().code, "unauthenticated");

    // Step 3: User B on srv2 (drain any pre-logout Step 1 OS sockets before clearing hits)
    pump_until_settled(&app, 50, || srv1_hits.lock().unwrap().len()).await;
    srv1_hits.lock().unwrap().clear();
    srv2_hits.lock().unwrap().clear();

    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(
                AuthResponse {
                    user_id: 2002,
                    username: "bob_srv2".into(),
                    session_id: "sess-b2".into(),
                    token: "tok_B2".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                cx,
            );
        })
        .unwrap();

    let st_b = ipc_call("status", serde_json::json!({})).await.unwrap();
    assert!(st_b.success);
    let st_b_data = st_b.data.unwrap();
    assert_eq!(st_b_data["authenticated"], true);
    assert_eq!(st_b_data["user_id"], 2002);
    assert_eq!(st_b_data["username"], "bob_srv2");

    let ust_b = ipc_call("user.status", serde_json::json!({})).await.unwrap();
    assert!(ust_b.success);
    let ust_b_data = ust_b.data.unwrap();
    assert_eq!(ust_b_data["server_url"], srv2_url);
    assert_eq!(ust_b_data["authenticated"], true);
    assert_eq!(ust_b_data["user_id"], 2002);

    let cl_b = ipc_call("chat.list", serde_json::json!({})).await.unwrap();
    assert!(cl_b.success);
    let cl_b_data = cl_b.data.unwrap();
    assert_eq!(cl_b_data["chats"][0]["chat_id"], 2202);
    assert_eq!(cl_b_data["chats"][0]["title"], "Srv2 Bob Chat");
    assert!(
        srv2_hits
            .lock()
            .unwrap()
            .iter()
            .any(|(auth, path)| auth == "Bearer tok_B2" && path == "/api/v1/chats"),
        "srv2 must record Bearer tok_B2 /api/v1/chats"
    );
    assert!(
        srv1_hits.lock().unwrap().is_empty(),
        "srv1 must receive 0 hits after ServerUrlApplied switch: {:?}",
        srv1_hits.lock().unwrap()
    );

    // Step 4: GAP-R14_2-1 mid-flight `device.current` races
    // 4a. Mid-flight ServerUrlApplied (srv2 -> srv1) race while device.current is held
    hold_srv2_tokens
        .lock()
        .unwrap()
        .insert("Bearer tok_B2".to_string());
    hold_srv1_tokens
        .lock()
        .unwrap()
        .insert("Bearer tok_C1".to_string());
    srv2_register_arrivals.lock().unwrap().clear();
    srv1_remote_exec_hits.lock().unwrap().clear();

    let ipc_step4a = ipc.clone();
    let jh_4a = rt_handle.spawn(async move {
        ipc_step4a
            .call("device.current", serde_json::json!({}))
            .await
    });
    assert!(
        pump_until(&app, 3000, || {
            srv2_register_arrivals
                .lock()
                .unwrap()
                .iter()
                .any(|a| a == "Bearer tok_B2")
        })
        .await,
        "srv2 /computers/local/register did not arrive"
    );

    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
            s.apply_authenticated_session(
                AuthResponse {
                    user_id: 3003,
                    username: "charlie_srv1".into(),
                    session_id: "sess-c1".into(),
                    token: "tok_C1".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                cx,
            );
        })
        .unwrap();

    hold_srv2_tokens.lock().unwrap().remove("Bearer tok_B2");
    let res_4a = jh_4a.await.expect("join 4a").expect("ipc 4a");
    assert!(
        !res_4a.success,
        "stale device.current across ServerUrlApplied must fail: {res_4a:?}"
    );
    assert_eq!(
        res_4a.error.as_ref().map(|e| e.code.as_str()),
        Some("session_changed")
    );
    let st_4a = ipc_call("status", serde_json::json!({})).await.unwrap();
    assert!(
        st_4a.data.as_ref().unwrap()["local_node_id"].is_null(),
        "stale device.current must NOT poison local_node_id: {:?}",
        st_4a.data
    );
    let test_4a = ipc_call(
        "device.test",
        serde_json::json!({"node_id": "local:user-2202-srv"}),
    )
    .await
    .unwrap();
    assert!(
        test_4a.success,
        "device.test for stale node_id must route remotely: {test_4a:?}"
    );
    assert!(
        srv1_remote_exec_hits
            .lock()
            .unwrap()
            .iter()
            .any(|(auth, nid)| auth == "Bearer tok_C1" && nid == "local:user-2202-srv"),
        "expected device.test to route remotely to srv1 with Bearer tok_C1, got: {:?}",
        *srv1_remote_exec_hits.lock().unwrap()
    );

    // Drain Charlie's Step 4a registration before Step 4b
    hold_srv1_tokens.lock().unwrap().remove("Bearer tok_C1");
    assert!(
        pump_until(&app, 3000, || {
            srv1_hits
                .lock()
                .unwrap()
                .iter()
                .any(|(a, p)| a == "Bearer tok_C1" && p == "/api/v1/mcp/grants")
        })
        .await,
        "Charlie Step 4a startup sync on srv1 did not complete"
    );

    // 4b. Mid-flight same-server logout + login race (Arc::ptr_eq unchanged; exercises ensure_pinned_session)
    srv1_register_arrivals.lock().unwrap().clear();
    srv1_remote_exec_hits.lock().unwrap().clear();
    {
        let mut g = hold_srv1_tokens.lock().unwrap();
        g.insert("Bearer tok_C1".to_string());
        g.insert("Bearer tok_D1".to_string());
    }

    let ipc_step4b = ipc.clone();
    let jh_4b = rt_handle.spawn(async move {
        ipc_step4b
            .call("device.current", serde_json::json!({}))
            .await
    });
    assert!(
        pump_until(&app, 3000, || {
            srv1_register_arrivals
                .lock()
                .unwrap()
                .iter()
                .any(|a| a == "Bearer tok_C1")
        })
        .await,
        "srv1 /computers/local/register did not arrive"
    );

    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(
                AuthResponse {
                    user_id: 4004,
                    username: "dave_srv1".into(),
                    session_id: "sess-d1".into(),
                    token: "tok_D1".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                cx,
            );
        })
        .unwrap();

    hold_srv1_tokens.lock().unwrap().remove("Bearer tok_C1");
    let res_4b = jh_4b.await.expect("join 4b").expect("ipc 4b");
    assert!(
        !res_4b.success,
        "stale device.current across same-server logout+login must fail: {res_4b:?}"
    );
    assert_eq!(
        res_4b.error.as_ref().map(|e| e.code.as_str()),
        Some("session_changed")
    );
    let st_4b = ipc_call("status", serde_json::json!({})).await.unwrap();
    assert!(
        st_4b.data.as_ref().unwrap()["local_node_id"].is_null(),
        "stale device.current must NOT poison local_node_id after same-server switch"
    );
    let test_4b = ipc_call(
        "device.test",
        serde_json::json!({"node_id": "local:user-1101-srv"}),
    )
    .await
    .unwrap();
    assert!(
        test_4b.success,
        "device.test for stale srv1 node_id must route remotely: {test_4b:?}"
    );
    assert!(
        srv1_remote_exec_hits
            .lock()
            .unwrap()
            .iter()
            .any(|(auth, nid)| auth == "Bearer tok_D1" && nid == "local:user-1101-srv"),
        "expected device.test to route remotely to srv1 with Bearer tok_D1, got: {:?}",
        *srv1_remote_exec_hits.lock().unwrap()
    );
    hold_srv1_tokens.lock().unwrap().remove("Bearer tok_D1");
}

// [T5-50] R13B remaining cx.spawn fences (composer, pts updates, load_historical_messages
// reaction poisoning, append_outgoing_text draft guard, show_toast overlay fallback,
// shell_modals proxy fences, shell_computers_sync same-turn mutations).
#[test]
fn t5_50_r13b_remaining_cx_spawn_modal_proxy_computer_and_toast_fences() {
    run_in_headless_subprocess(
        "t5_50_r13b_remaining_cx_spawn_modal_proxy_computer_and_toast_fences",
    );
}

async fn t5_50_body(app: AsyncApp) {
    use gpui_telegram_client::session_persistence::{ProxyEndpointRecord, SessionPersistenceBridge};
    use gpui_telegram_core::types::{MessageId, PeerId, UserId};
    use gpui_telegram_desktop::views::chat_sidebar::SidebarCreationModal;
    use gpui_telegram_desktop::views::settings::sections::SettingsComputersEvent;
    use gpui_telegram_proto::computer_contracts::{
        CloudAuthMethod, UpdateComputerPolicyRequest, UpsertCloudComputerRequest,
    };
    use gpui_telegram_proto::contracts::DraftDto;
    use gpui_telegram_proto::message::{MessageContent as ProtoMsgContent, MessageItem as ProtoMsg, ReactionCount};
    use gpui_telegram_proto::sync::{ChatActionEvent, UpdateEvent};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let recorded: Arc<std::sync::Mutex<Vec<(String, String, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let hold_send = Arc::new(AtomicBool::new(false));
    let send_arrived = Arc::new(AtomicUsize::new(0));
    let send_done = Arc::new(AtomicUsize::new(0));
    let return_send_500 = Arc::new(AtomicBool::new(false));
    let hold_history_a = Arc::new(AtomicBool::new(false));
    let history_a_arrived = Arc::new(AtomicUsize::new(0));
    let history_a_done = Arc::new(AtomicUsize::new(0));

    let recorded_srv = Arc::clone(&recorded);
    let hold_send_srv = Arc::clone(&hold_send);
    let send_arrived_srv = Arc::clone(&send_arrived);
    let send_done_srv = Arc::clone(&send_done);
    let return_send_500_srv = Arc::clone(&return_send_500);
    let hold_history_a_srv = Arc::clone(&hold_history_a);
    let history_a_arrived_srv = Arc::clone(&history_a_arrived);
    let history_a_done_srv = Arc::clone(&history_a_done);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock api server");
    let addr = listener.local_addr().unwrap();
    let srv_url = format!("http://{addr}");
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let recorded_srv = Arc::clone(&recorded_srv);
            let hold_send_srv = Arc::clone(&hold_send_srv);
            let send_arrived_srv = Arc::clone(&send_arrived_srv);
            let send_done_srv = Arc::clone(&send_done_srv);
            let return_send_500_srv = Arc::clone(&return_send_500_srv);
            let hold_history_a_srv = Arc::clone(&hold_history_a_srv);
            let history_a_arrived_srv = Arc::clone(&history_a_arrived_srv);
            let history_a_done_srv = Arc::clone(&history_a_done_srv);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = req.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let auth_hdr = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                recorded_srv
                    .lock()
                    .unwrap()
                    .push((method.clone(), path.clone(), auth_hdr.clone()));

                if method == "POST" && path == "/api/v1/messages/send" {
                    send_arrived_srv.fetch_add(1, Ordering::SeqCst);
                    while hold_send_srv.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let (status, body) = if return_send_500_srv.load(Ordering::SeqCst) {
                        ("500 Internal Server Error", r#"{"error":"simulated send error"}"#)
                    } else {
                        ("200 OK", r#"{"message_id":701,"pts":10,"date":1700000000000}"#)
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    send_done_srv.fetch_add(1, Ordering::SeqCst);
                } else if method == "GET" && path.starts_with("/api/v1/chats/101/messages") {
                    if auth_hdr.contains("tok-a") && hold_history_a_srv.load(Ordering::SeqCst) {
                        history_a_arrived_srv.fetch_add(1, Ordering::SeqCst);
                        while hold_history_a_srv.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        let body = r#"[{"message_id":42,"sender_id":1001,"text":"user_a_msg","reactions":[{"emoji":"🔥","count":1,"chosen":true}],"reactions_authoritative":true}]"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                        let _ = stream.flush();
                        history_a_done_srv.fetch_add(1, Ordering::SeqCst);
                    } else {
                        let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                        let _ = stream.write_all(resp.as_bytes());
                    }
                } else if path.starts_with("/api/v1/chats/101/members")
                    || path.starts_with("/api/v1/messages/history")
                    || path.starts_with("/api/v1/chats")
                    || path.starts_with("/api/v1/mcp/")
                {
                    let body = if path == "/api/v1/chats" {
                        r#"[{"chat_id":101,"title":"Test Bot Chat","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000}]"#
                    } else {
                        r#"[]"#
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path.starts_with("/api/v1/computers")
                    || path.starts_with("/api/v1/bots/")
                {
                    let body = if path == "/api/v1/computers" {
                        r#"{"computers":[]}"#
                    } else if path.ends_with("/computer") && method == "PUT" {
                        r#"{"bot_id":901,"bound_node_id":"node-b","persona_prompt":"p","preferred_model":"gpt-4o","approval_mode":"ask_every_time","allow_cross_device_control":false,"allow_cloud_self_config":false,"updated_at_ms":1700000000000}"#
                    } else if path.ends_with("/policy") {
                        r#"{"node_id":"node-b","owner_user_id":202,"kind":"cloud_server","display_name":"Node B","ip_address":"10.0.0.2","online":true,"is_current_device":false,"tier_badge":"cloud","run_policy":"disabled","authorized_controllers":[],"bound_bot_ids":[],"last_seen_ms":1700000000000}"#
                    } else if path == "/api/v1/computers/cloud" {
                        r#"{"node_id":"cloud:new","owner_user_id":202,"kind":"cloud_server","display_name":"Cloud B","ip_address":"10.0.0.3","online":true,"is_current_device":false,"tier_badge":"cloud","run_policy":"always_allow_authorized","authorized_controllers":[],"bound_bot_ids":[],"last_seen_ms":1700000000000}"#
                    } else {
                        r#"{}"#
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let client = new_isolated_client(srv_url.clone());
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // ------------------------------------------------------------------
    // Sub-case A: GAP-R13B-1..4 Composer, PTS Updates, load_historical_messages,
    // append_outgoing_text draft guard, and show_toast overlay fallback
    // ------------------------------------------------------------------
    for direct_switch in [false, true] {
        recorded.lock().unwrap().clear();
        shell
            .update(&mut app.clone(), |s, cx| {
                s.apply_authenticated_session_with_vault(
                    AuthResponse {
                        user_id: UID_VIEWER,
                        username: "alice".into(),
                        session_id: "sess-a".into(),
                        token: "tok-a".into(),
                        first_name: None,
                        created_at: None,
                        expires_at: None,
                    },
                    None,
                    cx,
                );
                s.sidebar.update(cx, |sb, _| {
                    sb.bot_ids.insert(101);
                    sb.dialogs.push(DialogItemModel {
                        id: 101,
                        peer_name: "Test Bot".into(),
                        avatar_initials: "TB".into(),
                        avatar_color: 0x8F59F9,
                        last_message: String::new(),
                        timestamp: String::new(),
                        unread_count: 0,
                        is_muted: false,
                        is_pinned: false,
                        is_online: true,
                        typing_indicator: None,
                        delivery_status: DialogDeliveryStatus::Read,
                        folder: "All".into(),
                    });
                    sb.select_dialog(101);
                });
                s.stage.update(cx, |st, _| {
                    st.set_chat(101, "Test Bot", "bot", "TB", 0x8F59F9);
                    st.is_bot_chat = true;
                });
            })
            .unwrap();
        assert!(
            pump_until(&app, 4000, || {
                let g = recorded.lock().unwrap();
                g.iter().any(|(m, p, a)| {
                    m == "GET" && p == "/api/v1/chats/101/pins" && a.contains("tok-a")
                }) && g.iter().any(|(m, p, a)| {
                    m == "GET" && p == "/api/v1/chats/101/members" && a.contains("tok-a")
                }) && g.iter().any(|(m, p, a)| {
                    m == "GET"
                        && p.starts_with("/api/v1/chats/101/messages")
                        && a.contains("tok-a")
                })
            })
            .await,
            "direct_switch={direct_switch}: Alice initial chat 101 sync did not finish"
        );
        pump_until_settled(&app, 50, || recorded.lock().unwrap().len()).await;
        recorded.lock().unwrap().clear();

        shell
            .update(&mut app.clone(), |s, cx| {
                s.composer.update(cx, |c, _| {
                    c.pending_edit_commit = Some((55, "user a edited".into()));
                });
                s.handle_composer_pending_actions(cx);
                s.send_rich_text_options("user a silent".into(), true, None, cx);
                s.apply_update_event(
                    ChatId(101),
                    UpdateEvent::ChatAction(ChatActionEvent::UserJoined {
                        user_id: UserId(999),
                    }),
                    cx,
                );
                let mut incoming = ProtoMsg::new(
                    MessageId(77),
                    ChatId(101),
                    UserId(9001),
                    PeerId::Chat(ChatId(101)),
                    ProtoMsgContent::plain_text("bot reply"),
                );
                incoming.sender_id = UserId(9001);
                s.apply_update_event(ChatId(101), UpdateEvent::NewMessage(incoming), cx);
                s.load_historical_messages(101, cx);

                if !direct_switch {
                    s.logout(cx);
                }
                s.apply_authenticated_session_with_vault(
                    AuthResponse {
                        user_id: 202,
                        username: "bob".into(),
                        session_id: "sess-b".into(),
                        token: "tok-b".into(),
                        first_name: None,
                        created_at: None,
                        expires_at: None,
                    },
                    None,
                    cx,
                );
            })
            .unwrap();

        assert!(
            pump_until(&app, 3000, || {
                recorded
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(m, p, a)| m == "GET" && p == "/api/v1/chats" && a.contains("tok-b"))
            })
            .await,
            "direct_switch={direct_switch}: Bob /api/v1/chats did not arrive"
        );
        app.background_executor()
            .timer(Duration::from_millis(120))
            .await;

        let hits = recorded.lock().unwrap().clone();
        for (m, p, auth) in &hits {
            assert!(
                !(auth.contains("tok-b")
                    && (p.contains("/messages/55")
                        || p == "/api/v1/messages/send"
                        || p.contains("/members")
                        || p.starts_with("/api/v1/messages/history"))),
                "direct_switch={direct_switch}: stale User A cx.spawn leaked under tok-b: ({m}, {p}, {auth})"
            );
            assert!(
                !(auth.contains("tok-a")
                    && (p.contains("/messages/55")
                        || p == "/api/v1/messages/send"
                        || p.contains("/members")
                        || p.starts_with("/api/v1/messages/history"))),
                "direct_switch={direct_switch}: stale User A cx.spawn failed to abort at preflight_ui: ({m}, {p}, {auth})"
            );
        }
    }

    // Sub-case A2: In-flight load_historical_messages cross-user local_store reaction poisoning
    hold_history_a.store(true, Ordering::SeqCst);
    history_a_arrived.store(0, Ordering::SeqCst);
    history_a_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "alice".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.load_historical_messages(101, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || history_a_arrived.load(Ordering::SeqCst) >= 1).await,
        "User A GET /api/v1/chats/101/messages did not arrive"
    );

    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "bob".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.client.local_store.update_reactions(
                ChatId(101),
                MessageId(42),
                vec![ReactionCount::new("❤️", 1, true)],
            );
        })
        .unwrap();

    hold_history_a.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || history_a_done.load(Ordering::SeqCst) >= 1).await,
        "User A GET /api/v1/chats/101/messages response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    shell
        .read_with(&app, |s, _| {
            let rx = s
                .client
                .local_store
                .get_reactions(ChatId(101), MessageId(42))
                .expect("User B reaction on (101, 42) must remain");
            assert_eq!(rx[0].emoji, "❤️", "User A history poisoned User B local_store reactions!");
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            let g = recorded.lock().unwrap();
            g.iter()
                .any(|(m, p, a)| m == "GET" && p == "/api/v1/chats" && a.contains("tok-b"))
                && g.iter().any(|(m, p, a)| {
                    m == "GET" && p == "/api/v1/fabushi-bots" && a.contains("tok-b")
                })
                && g.iter().any(|(m, p, a)| {
                    m == "GET" && p == "/api/v1/mcp/grants" && a.contains("tok-b")
                })
        })
        .await,
        "Bob startup refresh_chats/miniapps/grants did not finish before Sub-case A3"
    );
    pump_until_settled(&app, 50, || recorded.lock().unwrap().len()).await;

    // Sub-case A3: append_outgoing_text draft guard (failed send AND mid-send switch)
    return_send_500.store(true, Ordering::SeqCst);
    hold_send.store(false, Ordering::SeqCst);
    send_done.store(0, Ordering::SeqCst);
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, _| {
            s.client.disconnect_ws();
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .update(&mut app.clone(), |s, cx| {
            s.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: 101,
                    peer_name: "Bob Chat".into(),
                    avatar_initials: "BC".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(101);
            });
            s.stage.update(cx, |st, _| st.set_chat(101, "Bob Chat", "user", "BC", 0x3A7AFE));
            s.client.local_store.set_draft(
                ChatId(101),
                Some(DraftDto {
                    chat_id: 101,
                    text: "keep on fail".into(),
                    entities: Vec::new(),
                    reply_to_msg_id: None,
                    topic_id: None,
                    date_ms: 1_700_000_000_000,
                }),
            );
            s.client.network_listener.set_online(true);
            s.append_outgoing_text("fail msg".into(), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || send_done.load(Ordering::SeqCst) >= 1).await,
        "failed send did not finish"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |_, c_cx| c_cx.notify());
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .read_with(&app, |s, _| {
            assert_eq!(
                s.client
                    .local_store
                    .get_draft(ChatId(101))
                    .map(|d| d.text),
                Some("keep on fail".to_string()),
                "failed send must NOT clear local_store draft"
            );
        })
        .unwrap();
    assert!(
        !recorded
            .lock()
            .unwrap()
            .iter()
            .any(|(m, p, _)| m == "PUT" && p == "/api/v1/chats/101/draft"),
        "failed send must NOT call save_draft"
    );

    // Mid-send switch: User A sends -> held -> logout + User B -> User B sets draft -> release
    return_send_500.store(false, Ordering::SeqCst);
    hold_send.store(true, Ordering::SeqCst);
    send_arrived.store(0, Ordering::SeqCst);
    send_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "alice".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: 101,
                    peer_name: "Alice Chat".into(),
                    avatar_initials: "AC".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(101);
            });
            s.stage.update(cx, |st, _| st.set_chat(101, "Alice Chat", "user", "AC", 0x3A7AFE));
            s.client.disconnect_ws();
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .update(&mut app.clone(), |s, cx| {
            s.client.network_listener.set_online(true);
            s.append_outgoing_text("user a msg".into(), cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) >= 1).await,
        "User A append_outgoing_text POST /api/v1/messages/send did not arrive"
    );

    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "bob".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.client.local_store.set_draft(
                ChatId(101),
                Some(DraftDto {
                    chat_id: 101,
                    text: "user b important draft".into(),
                    entities: Vec::new(),
                    reply_to_msg_id: None,
                    topic_id: None,
                    date_ms: 1_700_000_000_001,
                }),
            );
        })
        .unwrap();

    hold_send.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || send_done.load(Ordering::SeqCst) >= 1).await,
        "User A POST /api/v1/messages/send response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    shell
        .read_with(&app, |s, _| {
            assert_eq!(
                s.client
                    .local_store
                    .get_draft(ChatId(101))
                    .map(|d| d.text),
                Some("user b important draft".to_string()),
                "User A's stale send completion wiped User B's local_store draft!"
            );
        })
        .unwrap();
    assert!(
        !recorded
            .lock()
            .unwrap()
            .iter()
            .any(|(m, p, _)| m == "PUT" && p == "/api/v1/chats/101/draft"),
        "stale User A send must NOT issue PUT /api/v1/chats/101/draft after switch: {:?}",
        *recorded.lock().unwrap()
    );

    // Sub-case A4: AppShell::show_toast fallback during Contacts / Creation / Passcode overlays
    shell
        .update(&mut app.clone(), |s, cx| {
            s.sidebar.update(cx, |sb, _| {
                sb.dialogs.push(DialogItemModel {
                    id: 101,
                    peer_name: "Chat 101".into(),
                    avatar_initials: "C1".into(),
                    avatar_color: 0x3A7AFE,
                    last_message: String::new(),
                    timestamp: String::new(),
                    unread_count: 0,
                    is_muted: false,
                    is_pinned: false,
                    is_online: true,
                    typing_indicator: None,
                    delivery_status: DialogDeliveryStatus::Read,
                    folder: "All".into(),
                });
                sb.select_dialog(101);
            });
            s.stage.update(cx, |st, _| st.set_chat(101, "Chat 101", "user", "C1", 0x3A7AFE));
            s.active_panel = ActiveWorkspacePanel::Chats;

            for case_idx in 0..3u8 {
                s.window_runtime.controller.clear_all_toasts();
                s.sidebar.update(cx, |sb, _| {
                    sb.is_contacts_modal_open = case_idx == 0;
                    sb.active_creation_modal = (case_idx == 1).then_some(SidebarCreationModal::CreateGroup);
                });
                s.overlays.is_passcode_locked = case_idx == 2;

                s.show_chat_toast(101, "suppressed chat toast", 2000, cx);
                assert!(
                    s.window_runtime.controller.current_toast.is_none(),
                    "show_chat_toast must remain suppressed in overlay case {case_idx}"
                );

                s.show_toast("fallback toast", 2000, cx);
                let toast = s
                    .window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .unwrap_or_else(|| panic!("show_toast dropped toast in overlay case {case_idx}"));
                assert_eq!(toast.chat_id, None, "fallback toast must have chat_id=None in case {case_idx}");
                assert_eq!(toast.text, "fallback toast");
            }
            s.sidebar.update(cx, |sb, _| {
                sb.is_contacts_modal_open = false;
                sb.active_creation_modal = None;
            });
            s.overlays.is_passcode_locked = false;
            s.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();

    // ------------------------------------------------------------------
    // Sub-case B: shell_modals.rs AddProxy / SelectProxy fences
    // ------------------------------------------------------------------
    let proxy_arrived = Arc::new(AtomicUsize::new(0));
    let hold_proxy = Arc::new(AtomicBool::new(true));
    let proxy_arrived_srv = Arc::clone(&proxy_arrived);
    let hold_proxy_srv = Arc::clone(&hold_proxy);
    let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind proxy probe port");
    let proxy_addr = proxy_listener.local_addr().unwrap();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = proxy_listener.accept() {
            let arr = Arc::clone(&proxy_arrived_srv);
            let hold = Arc::clone(&hold_proxy_srv);
            std::thread::spawn(move || {
                arr.fetch_add(1, Ordering::SeqCst);
                while hold.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                drop(stream);
            });
        }
    });

    // B1: Same-turn AddProxy + logout + User B login
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "alice".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(
                ModalActionOutcome::AddProxy(ProxyEndpointRecord {
                    id: 9101,
                    kind: "HTTP".into(),
                    host: "127.0.0.1".into(),
                    port: proxy_addr.port(),
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 0,
                    enabled: false,
                }),
                cx,
            );
            s.logout(cx);
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "bob".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    assert!(
        !SessionPersistenceBridge::new(srv_url.clone())
            .state
            .proxies
            .iter()
            .any(|p| p.id == 9101),
        "stale same-turn AddProxy must NOT persist proxy 9101"
    );

    // B2: Mid-probe AddProxy + ServerUrlApplied(srv2) + User B login
    let srv2_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock srv2");
    let srv2_url = format!("http://{}", srv2_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = srv2_listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let body = if first_line.contains("/chats") || first_line.contains("/messages") {
                    "[]"
                } else {
                    "{}"
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            });
        }
    });
    hold_proxy.store(true, Ordering::SeqCst);
    proxy_arrived.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "alice".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(
                ModalActionOutcome::AddProxy(ProxyEndpointRecord {
                    id: 9102,
                    kind: "HTTP".into(),
                    host: "127.0.0.1".into(),
                    port: proxy_addr.port(),
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 0,
                    enabled: true,
                }),
                cx,
            );
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || proxy_arrived.load(Ordering::SeqCst) >= 1).await,
        "mid-probe AddProxy did not reach proxy listener"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv2_url.clone()), cx);
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "bob".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();
    hold_proxy.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    assert!(
        !SessionPersistenceBridge::new(srv_url.clone())
            .state
            .proxies
            .iter()
            .any(|p| p.id == 9102),
        "mid-probe AddProxy must NOT persist proxy 9102 after ServerUrlApplied"
    );
    shell
        .read_with(&app, |s, _| {
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "stale AddProxy toast must NOT appear on User B"
            );
        })
        .unwrap();

    // Restore srv_url for Sub-case C and test same-turn SelectProxy
    let mut b3 = SessionPersistenceBridge::new(srv_url.clone());
    b3.state.proxies.push(ProxyEndpointRecord {
        id: 9103,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: proxy_addr.port(),
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: false,
    });
    b3.save_to_disk();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv_url.clone()), cx);
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: UID_VIEWER,
                    username: "alice".into(),
                    session_id: "sess-a".into(),
                    token: "tok-a".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
            s.modal_container.update(cx, |m, _| m.dialog.proxies.clear());
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9103 }, cx);
            s.logout(cx);
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 202,
                    username: "bob".into(),
                    session_id: "sess-b".into(),
                    token: "tok-b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    let saved_9103 = SessionPersistenceBridge::new(srv_url.clone())
        .state
        .proxies
        .into_iter()
        .find(|p| p.id == 9103)
        .expect("9103 present");
    assert!(
        !saved_9103.enabled,
        "stale same-turn SelectProxy must NOT enable proxy 9103"
    );

    // ------------------------------------------------------------------
    // Sub-case C: shell_computers_sync.rs same-turn mutations + positive control
    // ------------------------------------------------------------------
    for direct_switch in [false, true] {
        recorded.lock().unwrap().clear();
        shell
            .update(&mut app.clone(), |s, cx| {
                s.apply_authenticated_session_with_vault(
                    AuthResponse {
                        user_id: UID_VIEWER,
                        username: "alice".into(),
                        session_id: "sess-a".into(),
                        token: "tok-a".into(),
                        first_name: None,
                        created_at: None,
                        expires_at: None,
                    },
                    None,
                    cx,
                );
            })
            .unwrap();
        assert!(
            pump_until(&app, 4000, || {
                let g = recorded.lock().unwrap();
                g.iter().any(|(m, p, a)| {
                    m == "GET" && p == "/api/v1/mcp/grants" && a.contains("tok-a")
                }) && g.iter().any(|(m, p, a)| {
                    m == "GET"
                        && p == "/api/v1/hermes/magpie/subscriptions"
                        && a.contains("tok-a")
                }) && g
                    .iter()
                    .filter(|(m, p, a)| m == "GET" && p == "/api/v1/chats" && a.contains("tok-a"))
                    .count()
                    >= 2
            })
            .await,
            "direct_switch={direct_switch}: Alice startup sync did not finish; recorded={:?}",
            recorded.lock().unwrap().clone()
        );
        pump_until_settled(&app, 50, || {
            recorded
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, p, a)| {
                    a.contains("tok-a")
                        && !p.starts_with("/api/v1/computers/local/poll")
                        && !p.starts_with("/api/v1/tailnet/derp")
                        && !p.starts_with("/api/v1/ws")
                        && *p != "/api/v1/computers"
                })
                .count()
        })
        .await;
        recorded.lock().unwrap().clear();

        shell
            .update(&mut app.clone(), |s, cx| {
                use gpui_telegram_desktop::platform::mac::tray_mac::tray_action_ids;
                use gpui_telegram_desktop::views::computer_workbench::{
                    ComputerWorkbench, WorkbenchCommand,
                };
                use gpui_telegram_proto::computer_contracts::{ComputerNodeDto, ComputerNodeKind};
                use gpui_telegram_proto::remote_control_contracts::{
                    AssistStartRequest, RemoteControlMode,
                };

                s.computers.computers = vec![
                    ComputerNodeDto {
                        node_id: "node-local-a".into(),
                        kind: ComputerNodeKind::LocalDevice,
                        display_name: "Local A".into(),
                        online: true,
                        ..Default::default()
                    },
                    ComputerNodeDto {
                        node_id: "node-cloud-a".into(),
                        kind: ComputerNodeKind::CloudServer,
                        display_name: "Cloud A".into(),
                        online: true,
                        ..Default::default()
                    },
                ];
                let dummy_wb = cx.new(|_| ComputerWorkbench::new("Cloud A".into()));
                let dummy_wb_2 = dummy_wb.clone();

                s.handle_computer_settings_event(
                    &SettingsComputersEvent::SaveCloudServer(UpsertCloudComputerRequest {
                        node_id: None,
                        display_name: "Cloud A".into(),
                        host: "10.0.0.1".into(),
                        port: 22,
                        username: "root".into(),
                        auth_method: CloudAuthMethod::SshPassword,
                        secret: Some("pw".into()),
                        agent_endpoint_url: None,
                        workdir: None,
                        bind_bot_id: None,
                        verify_connection: false,
                    }),
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::UpdatePolicy {
                        node_id: "node-a".into(),
                        req: UpdateComputerPolicyRequest {
                            run_policy: Some(ComputerRunPolicy::Disabled),
                            ..Default::default()
                        },
                    },
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::DeleteNode("node-a".into()),
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::RevokeMcpGrant("fam-a".into()),
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::DisconnectAllRemoteControl,
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::TestNode("node-local-a".into()),
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::TestNode("node-cloud-a".into()),
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::ResetAssistTempCode,
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::SetAssistCustomCode("654321".into()),
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::SetAssistAllow(false),
                    cx,
                );
                s.handle_computer_settings_event(
                    &SettingsComputersEvent::StartAssistConnect(AssistStartRequest {
                        assist_id: "assist-a".into(),
                        access_code: "123456".into(),
                        requester_device_id: None,
                        requester_name: Some("Alice".into()),
                        requester_platform: None,
                        mode: RemoteControlMode::Desktop,
                    }),
                    cx,
                );
                s.decide_pending_computer_approval("cmd-srv-a".into(), true, cx);
                s.decide_pending_computer_approval("p2p:cmd-p2p-a".into(), true, cx);
                s.handle_workbench_command(
                    "node-cloud-a".into(),
                    WorkbenchCommand::RunShell {
                        command: "echo a".into(),
                        cwd: None,
                    },
                    dummy_wb,
                    cx,
                );
                s.handle_workbench_command(
                    "node-cloud-a".into(),
                    WorkbenchCommand::RefreshScreen,
                    dummy_wb_2,
                    cx,
                );
                s.handle_tray_action(tray_action_ids::TOGGLE_ALLOW_REMOTE, cx);
                s.disconnect_remote_session("sess-remote-a".into(), cx);
                s.refresh_remote_assist_and_grants(cx);
                s.refresh_computers(cx);
                s.sync_active_bot_computer_to_inspector(cx);
                s.handle_computer_inspector_action(
                    InspectorAction::SaveBotComputerConfig {
                        bot_id: 901,
                        persona_prompt: "user-a-prompt".into(),
                        preferred_model: "gpt-4o".into(),
                        approval_mode: ComputerRunPolicy::Disabled,
                        allow_cross_device_control: false,
                        allow_cloud_self_config: false,
                    },
                    cx,
                );

                if !direct_switch {
                    s.logout(cx);
                }
                s.apply_authenticated_session_with_vault(
                    AuthResponse {
                        user_id: 202,
                        username: "bob".into(),
                        session_id: "sess-b".into(),
                        token: "tok-b".into(),
                        first_name: None,
                        created_at: None,
                        expires_at: None,
                    },
                    None,
                    cx,
                );
            })
            .unwrap();

        app.background_executor()
            .timer(Duration::from_millis(120))
            .await;

        let hits = recorded.lock().unwrap().clone();
        for (m, p, auth) in &hits {
            let is_comp_mut = p == "/api/v1/computers/cloud"
                || p == "/api/v1/computers/node-a/policy"
                || (m == "DELETE" && p == "/api/v1/computers/node-a")
                || (m == "DELETE" && p == "/api/v1/mcp/grants/fam-a")
                || p == "/api/v1/remote/sessions/disconnect-all"
                || p == "/api/v1/computers/remote/execute"
                || p == "/api/v1/computers/node-cloud-a/test"
                || (p.starts_with("/api/v1/remote/assist/") && !auth.contains("tok-b"))
                || p.starts_with("/api/v1/computers/commands/cmd-srv-a")
                || p == "/api/v1/remote/sessions/sess-remote-a/disconnect"
                || (p == "/api/v1/remote/sessions" && !auth.contains("tok-b"))
                || (p == "/api/v1/mcp/grants" && !auth.contains("tok-b"))
                || (m == "GET" && p == "/api/v1/computers" && !auth.contains("tok-b"))
                || (m == "PUT" && p == "/api/v1/bots/901/computer");
            assert!(
                !is_comp_mut,
                "direct_switch={direct_switch}: stale computer closure leaked: ({m}, {p}, {auth})"
            );
        }
    }

    // Positive control: User B staying logged in sends SaveBotComputerConfig under tok-b
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_computer_inspector_action(
                InspectorAction::SaveBotComputerConfig {
                    bot_id: 901,
                    persona_prompt: "user-b-prompt".into(),
                    preferred_model: "gpt-4o".into(),
                    approval_mode: ComputerRunPolicy::AskEveryTime,
                    allow_cross_device_control: false,
                    allow_cloud_self_config: false,
                },
                cx,
            );
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            recorded.lock().unwrap().iter().any(|(m, p, auth)| {
                m == "PUT" && p == "/api/v1/bots/901/computer" && auth.contains("tok-b")
            })
        })
        .await,
        "positive control SaveBotComputerConfig for User B did not send under tok-b"
    );
}

// [T5-51] AuthWizard single shared Arc<TelegramClient> invariant, cross-server
// submit_login / submit_register / submit_email_register race fences, and
// active-session overwrite guard.
#[test]
fn t5_51_auth_wizard_single_client_sync_and_cross_server_session_races() {
    run_in_headless_subprocess(
        "t5_51_auth_wizard_single_client_sync_and_cross_server_session_races",
    );
}

async fn t5_51_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::auth::auth_wizard_state::{AuthWizardTab, LoginInputField};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let hold_srv1 = Arc::new(AtomicBool::new(false));
    let srv1_login_arrived = Arc::new(AtomicUsize::new(0));
    let srv1_login_done = Arc::new(AtomicUsize::new(0));
    let srv1_reg_arrived = Arc::new(AtomicUsize::new(0));
    let srv1_reg_done = Arc::new(AtomicUsize::new(0));
    let srv1_email_reg_arrived = Arc::new(AtomicUsize::new(0));
    let srv1_email_reg_done = Arc::new(AtomicUsize::new(0));
    let srv1_alipay_start_arrived = Arc::new(AtomicUsize::new(0));
    let srv1_alipay_start_done = Arc::new(AtomicUsize::new(0));
    let srv1_alipay_poll_arrived = Arc::new(AtomicUsize::new(0));
    let srv1_alipay_poll_done = Arc::new(AtomicUsize::new(0));
    let srv1_email_code_arrived = Arc::new(AtomicUsize::new(0));
    let srv1_email_code_done = Arc::new(AtomicUsize::new(0));
    let srv1_email_cooldown_secs = Arc::new(std::sync::atomic::AtomicU32::new(45));

    let hold_srv1_bg = Arc::clone(&hold_srv1);
    let l_arr = Arc::clone(&srv1_login_arrived);
    let l_done = Arc::clone(&srv1_login_done);
    let r_arr = Arc::clone(&srv1_reg_arrived);
    let r_done = Arc::clone(&srv1_reg_done);
    let er_arr = Arc::clone(&srv1_email_reg_arrived);
    let er_done = Arc::clone(&srv1_email_reg_done);
    let as_arr = Arc::clone(&srv1_alipay_start_arrived);
    let as_done = Arc::clone(&srv1_alipay_start_done);
    let ap_arr = Arc::clone(&srv1_alipay_poll_arrived);
    let ap_done = Arc::clone(&srv1_alipay_poll_done);
    let ec_arr = Arc::clone(&srv1_email_code_arrived);
    let ec_done = Arc::clone(&srv1_email_code_done);
    let ec_cooldown = Arc::clone(&srv1_email_cooldown_secs);

    let l1 = std::net::TcpListener::bind("127.0.0.1:0").expect("bind srv1");
    let srv1_url = format!("http://{}", l1.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = l1.accept() {
            let hold = Arc::clone(&hold_srv1_bg);
            let l_arr = Arc::clone(&l_arr);
            let l_done = Arc::clone(&l_done);
            let r_arr = Arc::clone(&r_arr);
            let r_done = Arc::clone(&r_done);
            let er_arr = Arc::clone(&er_arr);
            let er_done = Arc::clone(&er_done);
            let as_arr = Arc::clone(&as_arr);
            let as_done = Arc::clone(&as_done);
            let ap_arr = Arc::clone(&ap_arr);
            let ap_done = Arc::clone(&ap_done);
            let ec_arr = Arc::clone(&ec_arr);
            let ec_done = Arc::clone(&ec_done);
            let ec_cooldown = Arc::clone(&ec_cooldown);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let path = first_line.split_whitespace().nth(1).unwrap_or("");

                if path.starts_with("/api/v1/auth/kdf-params") {
                    let body = r#"{"error":"v1"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path == "/api/v1/auth/login"
                    || path == "/api/v1/auth/register"
                    || path == "/api/v1/auth/email/register"
                {
                    if path == "/api/v1/auth/login" {
                        l_arr.fetch_add(1, Ordering::SeqCst);
                    } else if path == "/api/v1/auth/register" {
                        r_arr.fetch_add(1, Ordering::SeqCst);
                    } else {
                        er_arr.fetch_add(1, Ordering::SeqCst);
                    }
                    while hold.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let body = r#"{"token":"tok_srv1","session_id":"sess-srv1","user_id":1001,"username":"alice_srv1"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    if path == "/api/v1/auth/login" {
                        l_done.fetch_add(1, Ordering::SeqCst);
                    } else if path == "/api/v1/auth/register" {
                        r_done.fetch_add(1, Ordering::SeqCst);
                    } else {
                        er_done.fetch_add(1, Ordering::SeqCst);
                    }
                } else if path == "/api/v1/auth/alipay/start" {
                    as_arr.fetch_add(1, Ordering::SeqCst);
                    while hold.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let body = r#"{"attempt_id":"att-srv1","login_url":"https://srv1.example/alipay","expires_at_ms":1900000000000}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    as_done.fetch_add(1, Ordering::SeqCst);
                } else if path.starts_with("/api/v1/auth/alipay/poll/") {
                    ap_arr.fetch_add(1, Ordering::SeqCst);
                    while hold.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let body = r#"{"status":"completed","token":"tok_alipay_srv1","user_id":1099,"username":"alipay_srv1"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    ap_done.fetch_add(1, Ordering::SeqCst);
                } else if path == "/api/v1/auth/email/send-code" {
                    ec_arr.fetch_add(1, Ordering::SeqCst);
                    while hold.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let cd_secs = ec_cooldown.load(Ordering::SeqCst);
                    let body = format!(
                        r#"{{"sent":true,"cooldown_seconds":{cd_secs},"expires_in_seconds":600}}"#
                    );
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                    ec_done.fetch_add(1, Ordering::SeqCst);
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let l2 = std::net::TcpListener::bind("127.0.0.1:0").expect("bind srv2");
    let srv2_url = format!("http://{}", l2.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = l2.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let path = first_line.split_whitespace().nth(1).unwrap_or("");
                if path.starts_with("/api/v1/auth/kdf-params") {
                    let body = r#"{"error":"v1"}"#;
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path == "/api/v1/auth/login" {
                    let body = r#"{"token":"tok_srv2","session_id":"sess-srv2","user_id":2002,"username":"bob_srv2"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path == "/api/v1/auth/alipay/start" {
                    let body = r#"{"attempt_id":"att-srv2","login_url":"https://srv2.example/alipay","expires_at_ms":1900000000000}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path == "/api/v1/auth/email/send-code" {
                    let body = r#"{"sent":true,"cooldown_seconds":30,"expires_in_seconds":600}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                } else if path == "/api/v1/chats" || path.contains("/messages") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
                    let _ = stream.write_all(resp.as_bytes());
                } else {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
                    let _ = stream.write_all(resp.as_bytes());
                }
            });
        }
    });

    let mut cfg = ClientConfig::default();
    cfg.server_url = srv1_url.clone();
    let client_srv1 = Arc::new(TelegramClient::new(cfg));
    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client_srv1, cx)))
        .unwrap();

    // ------------------------------------------------------------------
    // Sub-case A: Single Shared Arc<TelegramClient> Invariant
    // ------------------------------------------------------------------
    shell
        .update(&mut app.clone(), |s, cx| {
            assert!(
                Arc::ptr_eq(&s.auth_wizard.read(cx).client, &s.client),
                "initial AppShell and AuthWizardView must share the same Arc<TelegramClient>"
            );
            s.auth_wizard.update(cx, |w, wcx| {
                w.server_url_draft = srv2_url.clone();
                w.apply_custom_server_url(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.client.config.server_url == srv2_url)
                .unwrap_or(false)
        })
        .await,
        "ServerUrlApplied did not update AppShell client to srv2"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            assert!(
                Arc::ptr_eq(&s.auth_wizard.read(cx).client, &s.client),
                "after apply_custom_server_url(srv2), wizard and shell must share the SAME Arc<TelegramClient>"
            );
            assert_eq!(s.auth_wizard.read(cx).client.config.server_url, srv2_url);
            s.apply_authenticated_session(
                AuthResponse {
                    user_id: 2002,
                    username: "bob_srv2".into(),
                    session_id: "sess-b".into(),
                    token: "tok_srv2".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                cx,
            );
            s.logout(cx);
            assert!(
                Arc::ptr_eq(&s.auth_wizard.read(cx).client, &s.client),
                "after logout, wizard and shell must share the same Arc<TelegramClient>"
            );
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
            assert!(
                Arc::ptr_eq(&s.auth_wizard.read(cx).client, &s.client),
                "after direct ServerUrlApplied(srv1), wizard and shell must share the same Arc<TelegramClient>"
            );
            assert_eq!(s.client.config.server_url, srv1_url);
        })
        .unwrap();

    // ------------------------------------------------------------------
    // Sub-case B: Cross-Server submit_login / submit_register / submit_email_register Race
    // ------------------------------------------------------------------
    // B1: submit_login on srv1 -> switch to srv2 while in-flight
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_login_arrived.store(0, Ordering::SeqCst);
    srv1_login_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.username_input = "alice_srv1".into();
                w.password_input = "pw1".into();
                w.submit_login(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || srv1_login_arrived.load(Ordering::SeqCst) >= 1).await,
        "srv1 login did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.server_url_draft = srv2_url.clone();
                w.apply_custom_server_url(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.client.config.server_url == srv2_url)
                .unwrap_or(false)
        })
        .await
    );
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_login_done.load(Ordering::SeqCst) >= 1).await,
        "srv1 login response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let w = s.auth_wizard.read(cx);
            assert!(!s.is_authenticated, "stale srv1 login must NOT authenticate shell on srv2");
            assert!(!w.is_authenticated, "stale srv1 login must NOT mark wizard authenticated");
            assert!(s.session_sync.user_id.is_none());
            assert!(s.client.token().is_none());
            assert_eq!(s.client.config.server_url, srv2_url);
            assert!(Arc::ptr_eq(&w.client, &s.client));
        })
        .unwrap();

    // B2: submit_register on srv1 -> switch to srv2 while in-flight
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
        })
        .unwrap();
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_reg_arrived.store(0, Ordering::SeqCst);
    srv1_reg_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.username_input = "alice_reg".into();
                w.password_input = "pw1".into();
                w.submit_register(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 5000, || srv1_reg_arrived.load(Ordering::SeqCst) >= 1).await,
        "srv1 register did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.server_url_draft = srv2_url.clone();
                w.apply_custom_server_url(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.client.config.server_url == srv2_url)
                .unwrap_or(false)
        })
        .await
    );
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_reg_done.load(Ordering::SeqCst) >= 1).await,
        "srv1 register response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, cx| {
            assert!(!s.is_authenticated, "stale srv1 register must NOT authenticate shell on srv2");
            assert!(!s.auth_wizard.read(cx).is_authenticated);
        })
        .unwrap();

    // B3: submit_email_register on srv1 -> switch to srv2 while in-flight
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
        })
        .unwrap();
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_email_reg_arrived.store(0, Ordering::SeqCst);
    srv1_email_reg_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::EmailRegister, wcx);
                w.email_input = "alice@example.com".into();
                w.email_code_input = "123456".into();
                w.username_input = "alice_email".into();
                w.password_input = "pw1".into();
                w.submit_email_register(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 5000, || srv1_email_reg_arrived.load(Ordering::SeqCst) >= 1).await,
        "srv1 email_register did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.server_url_draft = srv2_url.clone();
                w.apply_custom_server_url(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.client.config.server_url == srv2_url)
                .unwrap_or(false)
        })
        .await
    );
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_email_reg_done.load(Ordering::SeqCst) >= 1).await,
        "srv1 email_register response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, cx| {
            assert!(!s.is_authenticated, "stale srv1 email_register must NOT authenticate shell on srv2");
            assert!(!s.auth_wizard.read(cx).is_authenticated);
        })
        .unwrap();

    // Positive control: login on srv2 completes cleanly
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::PasswordLogin, wcx);
                w.username_input = "bob_srv2".into();
                w.password_input = "pw2".into();
                w.submit_login(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.is_authenticated)
                .unwrap_or(false)
        })
        .await,
        "positive control login on srv2 did not authenticate"
    );
    shell
        .read_with(&app, |s, _| {
            assert_eq!(s.session_sync.user_id, Some(2002));
            assert_eq!(s.client.token().as_deref(), Some("tok_srv2"));
        })
        .unwrap();

    // ------------------------------------------------------------------
    // Sub-case C: Active-Session Overwrite Race
    // ------------------------------------------------------------------
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
        })
        .unwrap();
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_login_arrived.store(0, Ordering::SeqCst);
    srv1_login_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.username_input = "alice_srv1".into();
                w.password_input = "pw1".into();
                w.submit_login(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || srv1_login_arrived.load(Ordering::SeqCst) >= 1).await,
        "Sub-case C srv1 login did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session_with_vault(
                AuthResponse {
                    user_id: 2002,
                    username: "bob".into(),
                    session_id: "sess-b".into(),
                    token: "tok_user_b".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                None,
                cx,
            );
        })
        .unwrap();
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_login_done.load(Ordering::SeqCst) >= 1).await,
        "Sub-case C srv1 login response did not finish"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, cx| {
            assert!(s.is_authenticated);
            assert_eq!(s.session_sync.user_id, Some(2002));
            assert_eq!(s.session_sync.session_token.as_deref(), Some("tok_user_b"));
            assert_eq!(s.client.current_user_id(), Some(2002));
            assert_eq!(s.client.token().as_deref(), Some("tok_user_b"));
            assert!(s.auth_wizard.read(cx).is_authenticated);
        })
        .unwrap();

    // ------------------------------------------------------------------
    // Sub-case D: start_alipay_flow & send_email_code Server-Switch / Tab / Cooldown Races
    // ------------------------------------------------------------------
    // D1: start_alipay_flow held on srv1 -> switch to srv2 while already on AlipayQr tab ->
    //     sync_client_for_server_url echo does NOT cancel srv2's start_alipay_flow!
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
        })
        .unwrap();
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_alipay_start_arrived.store(0, Ordering::SeqCst);
    srv1_alipay_start_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::AlipayQr, wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || srv1_alipay_start_arrived.load(Ordering::SeqCst) >= 1).await,
        "srv1 alipay/start did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.server_url_draft = srv2_url.clone();
                w.apply_custom_server_url(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.client.config.server_url == srv2_url
                        && s.auth_wizard.read(cx).alipay_qr_payload.as_deref()
                            == Some("https://srv2.example/alipay")
                })
                .unwrap_or(false)
        })
        .await,
        "srv2 start_alipay_flow must complete without being self-cancelled by ServerUrlApplied echo"
    );
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_alipay_start_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(
                s.auth_wizard.read(cx).alipay_qr_payload.as_deref(),
                Some("https://srv2.example/alipay"),
                "stale srv1 alipay QR must not overwrite srv2 QR"
            );
        })
        .unwrap();

    // D2: alipay_poll in-flight on srv1 -> active session applied -> stale poll does NOT overwrite
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::PasswordLogin, wcx);
                w.switch_auth_tab(AuthWizardTab::AlipayQr, wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.auth_wizard.read(cx).alipay_attempt_id.as_deref() == Some("att-srv1")
                })
                .unwrap_or(false)
        })
        .await,
        "srv1 alipay_attempt_id not populated"
    );
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_alipay_poll_arrived.store(0, Ordering::SeqCst);
    srv1_alipay_poll_done.store(0, Ordering::SeqCst);
    assert!(
        pump_until(&app, 4000, || srv1_alipay_poll_arrived.load(Ordering::SeqCst) >= 1).await,
        "srv1 alipay/poll did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(
                AuthResponse {
                    user_id: 2002,
                    username: "bob_active".into(),
                    session_id: "sess-bob".into(),
                    token: "tok_bob".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                },
                cx,
            );
        })
        .unwrap();
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_alipay_poll_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .read_with(&app, |s, _| {
            assert_eq!(s.session_sync.user_id, Some(2002));
            assert_eq!(s.client.token().as_deref(), Some("tok_bob"));
        })
        .unwrap();

    // D3: send_email_code cooldown resets on ServerUrlApplied & logout, and mid-flight completion
    //     after tab switch / server switch does NOT mutate active_login_field or login_error.
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::EmailRegister, wcx);
                w.email_input = "alice@example.com".into();
                w.send_email_code(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| s.auth_wizard.read(cx).email_code_cooldown > 0)
                .unwrap_or(false)
        })
        .await,
        "email_code_cooldown did not become > 0"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.server_url_draft = srv2_url.clone();
                w.apply_custom_server_url(wcx);
            });
        })
        .unwrap();
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(
                s.auth_wizard.read(cx).email_code_cooldown,
                0,
                "email_code_cooldown must reset to 0 on server switch"
            );
        })
        .unwrap();

    // Mid-flight send_email_code on srv1 -> tab switch to PasswordLogin -> release
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv1_url.clone()), cx);
        })
        .unwrap();
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_email_code_arrived.store(0, Ordering::SeqCst);
    srv1_email_code_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::EmailRegister, wcx);
                w.email_input = "alice@example.com".into();
                w.send_email_code(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || srv1_email_code_arrived.load(Ordering::SeqCst) >= 1).await
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::PasswordLogin, wcx);
            });
        })
        .unwrap();
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_email_code_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let w = s.auth_wizard.read(cx);
            assert_eq!(w.auth_wizard_tab, AuthWizardTab::PasswordLogin);
            assert_eq!(
                w.active_login_field,
                LoginInputField::Username,
                "stale send_email_code must not overwrite active_login_field after tab switch"
            );
            assert!(!w.is_sending_email_code);
            assert_eq!(w.email_code_cooldown, 0);
        })
        .unwrap();

    // Round-trip tab-switch cancellation (EmailRegister -> PasswordLogin -> EmailRegister)
    hold_srv1.store(true, Ordering::SeqCst);
    srv1_email_code_arrived.store(0, Ordering::SeqCst);
    srv1_email_code_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::EmailRegister, wcx);
                w.email_input = "alice@example.com".into();
                w.send_email_code(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || srv1_email_code_arrived.load(Ordering::SeqCst) >= 1).await
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::PasswordLogin, wcx);
                w.switch_auth_tab(AuthWizardTab::EmailRegister, wcx);
            });
        })
        .unwrap();
    hold_srv1.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || srv1_email_code_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let w = s.auth_wizard.read(cx);
            assert_eq!(w.auth_wizard_tab, AuthWizardTab::EmailRegister);
            assert!(!w.is_sending_email_code);
            assert_eq!(
                w.email_code_cooldown, 0,
                "in-flight send_email_code must stay cancelled after EmailRegister -> PasswordLogin -> EmailRegister"
            );
        })
        .unwrap();

    // Active cooldown survival across tab switches (Probe 2 regression test)
    srv1_email_cooldown_secs.store(2, Ordering::SeqCst);
    let hits_before = srv1_email_code_arrived.load(Ordering::SeqCst);
    let done_before = srv1_email_code_done.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::EmailRegister, wcx);
                w.email_input = "alice@example.com".into();
                w.send_email_code(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            srv1_email_code_arrived.load(Ordering::SeqCst) == hits_before + 1
                && shell
                    .read_with(&app, |s, cx| s.auth_wizard.read(cx).email_code_cooldown == 2)
                    .unwrap_or(false)
        })
        .await,
        "2-second email_code_cooldown did not start"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::PasswordLogin, wcx);
            });
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(150))
        .await;
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.switch_auth_tab(AuthWizardTab::EmailRegister, wcx);
                w.send_email_code(wcx);
            });
        })
        .unwrap();
    assert_eq!(
        srv1_email_code_arrived.load(Ordering::SeqCst),
        hits_before + 1,
        "send_email_code must stay blocked while cooldown > 0 after tab switch"
    );
    assert!(
        pump_until(&app, 4500, || {
            shell
                .read_with(&app, |s, cx| s.auth_wizard.read(cx).email_code_cooldown == 0)
                .unwrap_or(false)
        })
        .await,
        "email_code_cooldown must count down to 0 across tab switch (not freeze)"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.auth_wizard.update(cx, |w, wcx| {
                w.send_email_code(wcx);
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            srv1_email_code_done.load(Ordering::SeqCst) >= done_before + 2
                && shell
                    .read_with(&app, |s, cx| s.auth_wizard.read(cx).email_code_cooldown > 0)
                    .unwrap_or(false)
        })
        .await,
        "second send_email_code must succeed after cooldown reaches 0"
    );
    assert_eq!(
        srv1_email_code_arrived.load(Ordering::SeqCst),
        hits_before + 2
    );
}

// [T5-52] GAP-R14-1 (shell_magpie_sync & local_subscription_bridge per-step epoch fences)
// and OBS-R14-2 (shell_modals AddProxy / SelectProxy mid-probe abort runtime-vs-persisted parity).
#[test]
fn t5_52_magpie_sync_per_step_epoch_fences_and_mid_probe_proxy_divergence() {
    run_in_headless_subprocess(
        "t5_52_magpie_sync_per_step_epoch_fences_and_mid_probe_proxy_divergence",
    );
}

async fn t5_52_body(app: AsyncApp) {
    use gpui_telegram_client::ProxyEndpointRecord;
    use gpui_telegram_client::SessionPersistenceBridge;
    use gpui_telegram_desktop::views::app_shell::local_subscription_bridge::sync_local_subscriptions_to_server_pinned;
    use gpui_telegram_desktop::views::settings::sections::SettingsAiProvidersEvent;
    use magpie_gateway::{
        ClientSubscriptionSyncItem, ProtocolKind, SubscriptionAccount, SubscriptionKind,
    };
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let recorded: Arc<std::sync::Mutex<Vec<(String, String, String)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let hold_catalog_a = Arc::new(AtomicBool::new(false));
    let catalog_a_arrived = Arc::new(AtomicUsize::new(0));
    let catalog_a_done = Arc::new(AtomicUsize::new(0));
    let hold_activate_a = Arc::new(AtomicBool::new(false));
    let activate_a_arrived = Arc::new(AtomicUsize::new(0));
    let activate_a_done = Arc::new(AtomicUsize::new(0));
    let hold_sync_a = Arc::new(AtomicBool::new(false));
    let sync_a_arrived = Arc::new(AtomicUsize::new(0));
    let sync_a_done = Arc::new(AtomicUsize::new(0));

    let rec_srv = Arc::clone(&recorded);
    let h_cat = Arc::clone(&hold_catalog_a);
    let cat_arr = Arc::clone(&catalog_a_arrived);
    let cat_done = Arc::clone(&catalog_a_done);
    let h_act = Arc::clone(&hold_activate_a);
    let act_arr = Arc::clone(&activate_a_arrived);
    let act_done = Arc::clone(&activate_a_done);
    let h_sync = Arc::clone(&hold_sync_a);
    let sync_arr = Arc::clone(&sync_a_arrived);
    let sync_done = Arc::clone(&sync_a_done);

    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind api srv");
    let srv_url = format!("http://{}", api_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            let rec = Arc::clone(&rec_srv);
            let h_cat = Arc::clone(&h_cat);
            let cat_arr = Arc::clone(&cat_arr);
            let cat_done = Arc::clone(&cat_done);
            let h_act = Arc::clone(&h_act);
            let act_arr = Arc::clone(&act_arr);
            let act_done = Arc::clone(&act_done);
            let h_sync = Arc::clone(&h_sync);
            let sync_arr = Arc::clone(&sync_arr);
            let sync_done = Arc::clone(&sync_done);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let auth_hdr = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()).unwrap_or_default())
                    .unwrap_or_default();

                rec.lock()
                    .unwrap()
                    .push((method.clone(), path.clone(), auth_hdr.clone()));

                let write_json = |s: &mut std::net::TcpStream, body: &str| {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes());
                    let _ = s.flush();
                };

                if method == "GET" && path.starts_with("/api/v1/hermes/magpie/models/catalog") {
                    if auth_hdr.contains("tok-a") {
                        cat_arr.fetch_add(1, Ordering::SeqCst);
                        while h_cat.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    write_json(
                        &mut stream,
                        r#"{"fetched_at_ms":1700000000000,"source":"mock","providers":{"openai":["gpt-5"]}}"#,
                    );
                    if auth_hdr.contains("tok-a") {
                        cat_done.fetch_add(1, Ordering::SeqCst);
                    }
                } else if method == "POST"
                    && path == "/api/v1/hermes/magpie/subscriptions/sub-a/activate"
                {
                    if auth_hdr.contains("tok-a") {
                        act_arr.fetch_add(1, Ordering::SeqCst);
                        while h_act.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    write_json(
                        &mut stream,
                        r#"{"ok":true,"active_provider_id":"sub-a","active_model":"gpt-5"}"#,
                    );
                    if auth_hdr.contains("tok-a") {
                        act_done.fetch_add(1, Ordering::SeqCst);
                    }
                } else if method == "POST" && path == "/api/v1/hermes/magpie/subscriptions/sync" {
                    if auth_hdr.contains("tok-a") {
                        sync_arr.fetch_add(1, Ordering::SeqCst);
                        while h_sync.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    write_json(&mut stream, r#"{"ok":true,"synced_count":1,"state":null}"#);
                    if auth_hdr.contains("tok-a") {
                        sync_done.fetch_add(1, Ordering::SeqCst);
                    }
                } else if path == "/api/v1/hermes/magpie/state" {
                    write_json(
                        &mut stream,
                        r#"{"presets":[],"providers":[],"active_provider":"openai","active_model":"gpt-5","allow_local_fallback":false,"has_active_subscription":true,"has_configured_provider":true,"needs_provider_selection":false,"subscriptions":[],"routing_groups":[],"routing_strategy":"round_robin"}"#,
                    );
                } else if path == "/api/v1/hermes/magpie/subscriptions"
                    || path == "/api/v1/chats"
                    || path.contains("/messages")
                {
                    write_json(&mut stream, "[]");
                } else if path == "/api/v1/hermes/magpie/subscriptions/scan" {
                    write_json(&mut stream, r#"{"subscriptions":[]}"#);
                } else if path == "/api/v1/user-vault/envelope" {
                    write_json(&mut stream, r#"{"envelope":null}"#);
                } else {
                    write_json(&mut stream, "{}");
                }
            });
        }
    });

    // Slow mock HTTP CONNECT proxy for OBS-R14-2
    let hold_proxy = Arc::new(AtomicBool::new(false));
    let proxy_arrived = Arc::new(AtomicUsize::new(0));
    let proxy_done = Arc::new(AtomicUsize::new(0));
    let h_prx = Arc::clone(&hold_proxy);
    let prx_arr = Arc::clone(&proxy_arrived);
    let prx_done = Arc::clone(&proxy_done);

    let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind proxy srv");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = proxy_listener.accept() {
            let h_prx = Arc::clone(&h_prx);
            let prx_arr = Arc::clone(&prx_arr);
            let prx_done = Arc::clone(&prx_done);
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                prx_arr.fetch_add(1, Ordering::SeqCst);
                while h_prx.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
                let _ = stream.flush();
                prx_done.fetch_add(1, Ordering::SeqCst);
            });
        }
    });

    let client = new_isolated_client(srv_url.clone());
    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .unwrap();

    let auth_a = AuthResponse {
        user_id: 1001,
        username: "alice".into(),
        session_id: "sess-a".into(),
        token: "tok-a".into(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };
    let auth_b = AuthResponse {
        user_id: 2002,
        username: "bob".into(),
        session_id: "sess-b".into(),
        token: "tok-b".into(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };

    let wait_for_auth_a_magpie_settled = |app: &AsyncApp,
                                          recorded: &Arc<
        std::sync::Mutex<Vec<(String, String, String)>>,
    >| {
        let app = app.clone();
        let recorded = Arc::clone(recorded);
        async move {
            assert!(
                pump_until(&app, 4000, || {
                    let g = recorded.lock().unwrap();
                    g.iter().any(|(m, p, a)| {
                        m == "GET" && p == "/api/v1/hermes/magpie/subscriptions" && a.contains("tok-a")
                    }) && g.iter().any(|(m, p, a)| {
                        m == "GET" && p == "/api/v1/hermes/magpie/state" && a.contains("tok-a")
                    })
                })
                .await,
                "Alice magpie startup sync did not settle"
            );
            pump_until_settled(&app, 50, || recorded.lock().unwrap().len()).await;
            recorded.lock().unwrap().clear();
        }
    };

    // ------------------------------------------------------------------
    // Sub-case A: GAP-R14-1 Per-Step Epoch Fences
    // ------------------------------------------------------------------
    // A1: refresh_magpie_state held at /models/catalog -> logout + login(User B)
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_a.clone(), cx);
        })
        .unwrap();
    wait_for_auth_a_magpie_settled(&app, &recorded).await;

    hold_catalog_a.store(true, Ordering::SeqCst);
    catalog_a_arrived.store(0, Ordering::SeqCst);
    catalog_a_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.refresh_magpie_state(cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || catalog_a_arrived.load(Ordering::SeqCst) >= 1).await,
        "A1 catalog request did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    recorded.lock().unwrap().clear();
    hold_catalog_a.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || catalog_a_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    for (m, p, auth) in recorded.lock().unwrap().iter() {
        if auth.contains("tok-a") && !p.starts_with("/api/v1/hermes/magpie/models/catalog") {
            assert!(
                p != "/api/v1/hermes/magpie/state" && p != "/api/v1/hermes/magpie/subscriptions",
                "A1 leaked tok-a request after catalog: ({m}, {p}, {auth})"
            );
        }
    }

    // A2: bootstrap_local_subscriptions_on_startup held at /models/catalog -> logout + login(User B)
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_a.clone(), cx);
        })
        .unwrap();
    wait_for_auth_a_magpie_settled(&app, &recorded).await;

    hold_catalog_a.store(true, Ordering::SeqCst);
    catalog_a_arrived.store(0, Ordering::SeqCst);
    catalog_a_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.magpie_local.bootstrapped_auth_generation = None;
            s.bootstrap_local_subscriptions_on_startup(cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || catalog_a_arrived.load(Ordering::SeqCst) >= 1).await,
        "A2 catalog request did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    recorded.lock().unwrap().clear();
    hold_catalog_a.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || catalog_a_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    for (m, p, auth) in recorded.lock().unwrap().iter() {
        if auth.contains("tok-a") {
            assert!(
                p != "/api/v1/hermes/magpie/subscriptions/sync"
                    && p != "/api/v1/hermes/magpie/state"
                    && p != "/api/v1/hermes/magpie/subscriptions",
                "A2 leaked tok-a request after catalog: ({m}, {p}, {auth})"
            );
        }
    }

    // A3: ScanSubscriptions & ActivateSubscription & FetchSubscriptionModels
    // A3.1 ScanSubscriptions
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_a.clone(), cx);
        })
        .unwrap();
    wait_for_auth_a_magpie_settled(&app, &recorded).await;
    hold_catalog_a.store(true, Ordering::SeqCst);
    catalog_a_arrived.store(0, Ordering::SeqCst);
    catalog_a_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_ai_providers_settings_event(&SettingsAiProvidersEvent::ScanSubscriptions, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || catalog_a_arrived.load(Ordering::SeqCst) >= 1).await,
        "A3.1 ScanSubscriptions catalog did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    recorded.lock().unwrap().clear();
    hold_catalog_a.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || catalog_a_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    for (m, p, auth) in recorded.lock().unwrap().iter() {
        if auth.contains("tok-a") {
            assert!(
                p != "/api/v1/hermes/magpie/subscriptions/scan"
                    && p != "/api/v1/hermes/magpie/subscriptions/sync"
                    && p != "/api/v1/hermes/magpie/subscriptions"
                    && p != "/api/v1/hermes/magpie/state",
                "A3.1 ScanSubscriptions leaked tok-a request: ({m}, {p}, {auth})"
            );
        }
    }

    // A3.2 ActivateSubscription
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_a.clone(), cx);
        })
        .unwrap();
    wait_for_auth_a_magpie_settled(&app, &recorded).await;
    hold_activate_a.store(true, Ordering::SeqCst);
    activate_a_arrived.store(0, Ordering::SeqCst);
    activate_a_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_ai_providers_settings_event(
                &SettingsAiProvidersEvent::ActivateSubscription {
                    subscription_id: "sub-a".into(),
                    model: None,
                },
                cx,
            );
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || activate_a_arrived.load(Ordering::SeqCst) >= 1).await,
        "A3.2 ActivateSubscription did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    recorded.lock().unwrap().clear();
    hold_activate_a.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || activate_a_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    for (m, p, auth) in recorded.lock().unwrap().iter() {
        if auth.contains("tok-a") {
            assert!(
                p != "/api/v1/hermes/magpie/state"
                    && p != "/api/v1/hermes/magpie/providers"
                    && p != "/api/v1/hermes/magpie/subscriptions",
                "A3.2 ActivateSubscription leaked tok-a request: ({m}, {p}, {auth})"
            );
        }
    }

    // A3.3 FetchSubscriptionModels
    let dummy_item = ClientSubscriptionSyncItem {
        account: SubscriptionAccount {
            id: "sub-codex-a".into(),
            kind: SubscriptionKind::CodexChatGpt,
            label: "Codex A".into(),
            email: None,
            account_id: None,
            plan: None,
            active: true,
            base_url: Some(srv_url.clone()),
            protocol: ProtocolKind::CodexResponses,
            default_model: Some("gpt-5".into()),
            source_path: "/tmp/mock".into(),
            expires_at_ms: None,
            access_token: Some("eyJ-test".into()),
            headers: BTreeMap::new(),
        },
        access_token: Some("eyJ-test".into()),
        raw_headers: BTreeMap::new(),
        vertex_adc: None,
        codex_oauth: None,
        region_id: None,
        available_models: vec!["gpt-5".into()],
    };
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_a.clone(), cx);
        })
        .unwrap();
    wait_for_auth_a_magpie_settled(&app, &recorded).await;
    hold_catalog_a.store(true, Ordering::SeqCst);
    catalog_a_arrived.store(0, Ordering::SeqCst);
    catalog_a_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.magpie_local.discovered_items = vec![dummy_item.clone()];
            s.handle_ai_providers_settings_event(
                &SettingsAiProvidersEvent::FetchSubscriptionModels("sub-codex-a".into()),
                cx,
            );
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || catalog_a_arrived.load(Ordering::SeqCst) >= 1).await,
        "A3.3 FetchSubscriptionModels catalog did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    recorded.lock().unwrap().clear();
    hold_catalog_a.store(false, Ordering::SeqCst);
    assert!(
        pump_until(&app, 3000, || catalog_a_done.load(Ordering::SeqCst) >= 1).await
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, _| {
            assert!(
                s.magpie_local.discovered_items.is_empty(),
                "User B discovered_items must stay clean"
            );
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "User B toast must stay clean"
            );
        })
        .unwrap();

    // A4: Direct local_subscription_bridge::sync_local_subscriptions_to_server_pinned race
    let iso_client = new_isolated_client(srv_url.clone());
    iso_client.set_auth_session(auth_a.clone());
    let epoch_a = iso_client.session_epoch();
    recorded.lock().unwrap().clear();
    hold_sync_a.store(true, Ordering::SeqCst);
    sync_a_arrived.store(0, Ordering::SeqCst);
    sync_a_done.store(0, Ordering::SeqCst);

    let iso_bg = iso_client.clone();
    let srv_bg = srv_url.clone();
    let item_bg = dummy_item.clone();
    let jh_sync = iso_client.runtime().spawn(async move {
        sync_local_subscriptions_to_server_pinned(
            &iso_bg,
            epoch_a,
            &srv_bg,
            "tok-a",
            vec![item_bg],
            true,
        )
        .await
    });
    assert!(
        pump_until(&app, 3000, || sync_a_arrived.load(Ordering::SeqCst) >= 1).await,
        "A4 sync_local_subscriptions_to_server_pinned did not hit /subscriptions/sync"
    );
    iso_client.set_auth_session(auth_b.clone());
    hold_sync_a.store(false, Ordering::SeqCst);
    let res_a4 = jh_sync.await.expect("join a4");
    assert!(
        res_a4.is_err(),
        "sync_local_subscriptions_to_server_pinned must return Err on mid-flight session switch"
    );
    for (m, p, auth) in recorded.lock().unwrap().iter() {
        if auth.contains("tok-a") {
            assert!(
                p != "/api/v1/hermes/magpie/state" && p != "/api/v1/hermes/magpie/providers",
                "A4 leaked post-sync tok-a request: ({m}, {p}, {auth})"
            );
        }
    }

    // A5: Positive control — User B undisturbed
    recorded.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.magpie_local.latest_state = None;
            s.refresh_magpie_state(cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.magpie_local.latest_state.is_some())
                .unwrap_or(false)
        })
        .await,
        "A5 positive control refresh_magpie_state did not populate latest_state"
    );
    assert!(
        recorded.lock().unwrap().iter().any(|(_, p, auth)| p == "/api/v1/hermes/magpie/state" && auth.contains("tok-b")),
        "A5 positive control must hit /magpie/state with tok-b"
    );
    assert!(
        recorded.lock().unwrap().iter().any(|(_, p, auth)| p == "/api/v1/hermes/magpie/subscriptions" && auth.contains("tok-b")),
        "A5 positive control must hit /magpie/subscriptions with tok-b"
    );

    // ------------------------------------------------------------------
    // Sub-case B: OBS-R14-2 Mid-Probe SelectProxy & AddProxy Runtime-vs-Persisted Parity
    // ------------------------------------------------------------------
    let mut bridge = SessionPersistenceBridge::new(srv_url.clone());
    bridge.state.proxies = vec![
        ProxyEndpointRecord {
            id: 9201,
            kind: "HTTP".into(),
            host: "127.0.0.1".into(),
            port: proxy_port,
            secret_or_user: String::new(),
            username: None,
            password: None,
            ping_ms: 0,
            enabled: false,
        },
        ProxyEndpointRecord {
            id: 9202,
            kind: "HTTP".into(),
            host: "127.0.0.1".into(),
            port: proxy_port,
            secret_or_user: String::new(),
            username: None,
            password: None,
            ping_ms: 0,
            enabled: false,
        },
    ];
    bridge.save_to_disk();

    // B1: SelectProxy uncached (9201) mid-probe abort
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_a.clone(), cx);
            s.modal_container.update(cx, |m, _| m.dialog.proxies.clear());
        })
        .unwrap();
    hold_proxy.store(true, Ordering::SeqCst);
    proxy_arrived.store(0, Ordering::SeqCst);
    proxy_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9201 }, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || proxy_arrived.load(Ordering::SeqCst) >= 1).await,
        "B1 uncached proxy probe did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    hold_proxy.store(false, Ordering::SeqCst);
    assert!(pump_until(&app, 3000, || proxy_done.load(Ordering::SeqCst) >= 1).await);
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, _| {
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(
                disk.state.proxies.iter().any(|p| p.id == 9201 && !p.enabled),
                "B1 disk proxy 9201 must remain enabled=false"
            );
            assert!(
                s.client.http_client.proxy().is_none(),
                "B1 http_client.proxy() must remain None after mid-probe abort"
            );
            assert!(
                s.client.ws_transport.proxy().is_none(),
                "B1 ws_transport.proxy() must remain None after mid-probe abort"
            );
            assert!(s.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();

    // B2: SelectProxy cached (9202) & AddProxy (9203) mid-probe aborts
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_a.clone(), cx);
            s.modal_container.update(cx, |m, _| {
                m.dialog.proxies = vec![ProxyEndpointRecord {
                    id: 9202,
                    kind: "HTTP".into(),
                    host: "127.0.0.1".into(),
                    port: proxy_port,
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 0,
                    enabled: false,
                }];
            });
        })
        .unwrap();
    hold_proxy.store(true, Ordering::SeqCst);
    proxy_arrived.store(0, Ordering::SeqCst);
    proxy_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9202 }, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || proxy_arrived.load(Ordering::SeqCst) >= 1).await,
        "B2 cached proxy probe did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    hold_proxy.store(false, Ordering::SeqCst);
    assert!(pump_until(&app, 3000, || proxy_done.load(Ordering::SeqCst) >= 1).await);
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, _| {
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(disk.state.proxies.iter().any(|p| p.id == 9202 && !p.enabled));
            assert!(s.client.http_client.proxy().is_none());
            assert!(s.client.ws_transport.proxy().is_none());
        })
        .unwrap();

    // AddProxy (9203) mid-probe abort
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_a.clone(), cx);
        })
        .unwrap();
    hold_proxy.store(true, Ordering::SeqCst);
    proxy_arrived.store(0, Ordering::SeqCst);
    proxy_done.store(0, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(
                ModalActionOutcome::AddProxy(ProxyEndpointRecord {
                    id: 9203,
                    kind: "HTTP".into(),
                    host: "127.0.0.1".into(),
                    port: proxy_port,
                    secret_or_user: String::new(),
                    username: None,
                    password: None,
                    ping_ms: 0,
                    enabled: true,
                }),
                cx,
            );
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || proxy_arrived.load(Ordering::SeqCst) >= 1).await,
        "B2 AddProxy probe did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.logout(cx);
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    hold_proxy.store(false, Ordering::SeqCst);
    assert!(pump_until(&app, 3000, || proxy_done.load(Ordering::SeqCst) >= 1).await);
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    shell
        .read_with(&app, |s, _| {
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(
                !disk.state.proxies.iter().any(|p| p.id == 9203),
                "B2 aborted AddProxy 9203 must not persist to disk"
            );
            assert!(s.client.http_client.proxy().is_none());
            assert!(s.client.ws_transport.proxy().is_none());
        })
        .unwrap();

    // B3: Positive control — User B stays logged in, SelectProxy(9201) persists & applies proxy
    shell
        .update(&mut app.clone(), |s, cx| {
            s.modal_container.update(cx, |m, _| m.dialog.proxies.clear());
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9201 }, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            SessionPersistenceBridge::new(srv_url.clone())
                .state
                .proxies
                .iter()
                .any(|p| p.id == 9201 && p.enabled)
        })
        .await,
        "B3 positive control SelectProxy(9201) did not persist"
    );
    shell
        .read_with(&app, |s, _| {
            assert!(s.client.http_client.proxy().is_some());
            assert!(s.client.ws_transport.proxy().is_some());
        })
        .unwrap();
}

// [T5-53] GAP-R15-1: Proxy validation (invalid host / MTProto) and uncached SelectProxy must not
// diverge settings_advanced UI from runtime or disk.
#[test]
fn t5_53_gap_r15_1_proxy_validation_and_uncached_select_no_ui_divergence() {
    run_in_headless_subprocess(
        "t5_53_gap_r15_1_proxy_validation_and_uncached_select_no_ui_divergence",
    );
}

async fn t5_53_body(app: AsyncApp) {
    use gpui_telegram_client::ProxyEndpointRecord;
    use gpui_telegram_client::SessionPersistenceBridge;
    use gpui_telegram_desktop::views::settings::sections::settings_advanced::ProxyType;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind api srv");
    let srv_url = format!("http://{}", api_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let path = first_line.split_whitespace().nth(1).unwrap_or("");
                let body = if path == "/api/v1/chats" || path.contains("/messages") {
                    "[]"
                } else if path == "/api/v1/user-vault/envelope" {
                    r#"{"envelope":null}"#
                } else {
                    "{}"
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind proxy srv");
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = proxy_listener.accept() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
                let _ = stream.flush();
            });
        }
    });

    let mut bridge = SessionPersistenceBridge::new(srv_url.clone());
    bridge.state.proxies = vec![ProxyEndpointRecord {
        id: 9201,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    }];
    bridge.save_to_disk();

    let client = new_isolated_client(srv_url.clone());
    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .unwrap();

    let auth_a = AuthResponse {
        user_id: 1001,
        username: "alice".into(),
        session_id: "sess-a".into(),
        token: "tok-a".into(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_a.clone(), cx);
            s.modal_container.update(cx, |m, _| m.dialog.proxies.clear());
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9201 }, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    let ui_ok = s
                        .settings_router
                        .read(cx)
                        .settings_advanced
                        .as_ref()
                        .is_some_and(|sa| {
                            let r = sa.read(cx);
                            r.proxy_type == ProxyType::Http
                                && r.proxy_host == "127.0.0.1"
                                && r.proxy_port == proxy_port
                        });
                    ui_ok
                        && s.client.http_client.proxy().is_some()
                        && s.client.ws_transport.proxy().is_some()
                })
                .unwrap_or(false)
        })
        .await,
        "initial proxy 9201 activation did not converge"
    );

    // Sub-case A: Uncached SelectProxy { proxy_id: 9999 } while 9201 is active
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.modal_container.update(cx, |m, _| m.dialog.proxies.clear());
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9999 }, cx);

            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .expect("settings_advanced present")
                .read(cx);
            assert_eq!(
                sa.proxy_type,
                ProxyType::Http,
                "uncached SelectProxy(9999) must NOT synchronously clobber UI to Direct"
            );
            assert_eq!(sa.proxy_host, "127.0.0.1");
            assert_eq!(sa.proxy_port, proxy_port);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                .unwrap_or(false)
        })
        .await,
        "missing proxy 9999 must surface a toast"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Http);
            assert_eq!(sa.proxy_host, "127.0.0.1");
            assert_eq!(sa.proxy_port, proxy_port);
            let hp = s
                .client
                .http_client
                .proxy()
                .expect("http_client proxy must remain 9201");
            assert_eq!(hp.host, "127.0.0.1");
            assert_eq!(hp.port, proxy_port);
            let wp = s
                .client
                .ws_transport
                .proxy()
                .expect("ws_transport proxy must remain 9201");
            assert_eq!(wp.host, "127.0.0.1");
            assert_eq!(wp.port, proxy_port);
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(
                disk.state
                    .proxies
                    .iter()
                    .any(|p| p.id == 9201 && p.enabled),
                "disk must still keep 9201 enabled"
            );
        })
        .unwrap();

    // Sub-case B: AddProxy with enabled: true and invalid host "not a valid host"
    let invalid_9204 = ProxyEndpointRecord {
        id: 9204,
        kind: "HTTP".into(),
        host: "not a valid host".into(),
        port: 1080,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    let invalid_9204_direct = invalid_9204.clone();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(invalid_9204), cx);

            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(
                sa.proxy_type,
                ProxyType::Http,
                "invalid AddProxy must NOT synchronously mutate settings_advanced"
            );
            assert_eq!(sa.proxy_host, "127.0.0.1");
            assert_eq!(sa.proxy_port, proxy_port);
            let toast_msg = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            assert!(
                toast_msg.contains("Proxy error:"),
                "expected Proxy error toast, got: {toast_msg:?}"
            );
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;

    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Http);
            assert_eq!(sa.proxy_host, "127.0.0.1");
            assert_eq!(sa.proxy_port, proxy_port);
            assert_eq!(s.client.http_client.proxy().unwrap().port, proxy_port);
            assert_eq!(s.client.ws_transport.proxy().unwrap().port, proxy_port);
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(
                !disk.state.proxies.iter().any(|p| p.id == 9204),
                "invalid proxy 9204 must NOT persist to disk"
            );
            assert!(disk.state.proxies.iter().any(|p| p.id == 9201 && p.enabled));
        })
        .unwrap();

    let epoch_before_direct = shell.read_with(&app, |s, _| s.client.session_epoch()).unwrap();
    let direct_res = shell
        .read_with(&app, |s, _| s.client.clone())
        .unwrap()
        .upsert_proxy_for_session(Some("tok-a"), epoch_before_direct, invalid_9204_direct)
        .await;
    assert!(
        direct_res.is_err(),
        "client.upsert_proxy_for_session must reject invalid host"
    );
    shell
        .read_with(&app, |s, _| {
            assert_eq!(s.client.http_client.proxy().unwrap().port, proxy_port);
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(!disk.state.proxies.iter().any(|p| p.id == 9204));
            assert!(disk.state.proxies.iter().any(|p| p.id == 9201 && p.enabled));
        })
        .unwrap();

    // Sub-case C: MTProto compatibility + invalid MTProto host rejection
    let valid_mtproto = ProxyEndpointRecord {
        id: 9205,
        kind: "MTProto".into(),
        host: "mt.telegram.example".into(),
        port: 443,
        secret_or_user: "ee1122".into(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(valid_mtproto), cx);
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Mtproto);
            assert_eq!(sa.proxy_host, "mt.telegram.example");
            assert_eq!(sa.proxy_port, 443);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            SessionPersistenceBridge::new(srv_url.clone())
                .state
                .proxies
                .iter()
                .any(|p| p.id == 9205 && p.enabled)
        })
        .await,
        "valid MTProto proxy 9205 must persist enabled"
    );
    shell
        .read_with(&app, |s, _| {
            assert!(
                s.client.http_client.proxy().is_none(),
                "MTProto proxy leaves HTTP proxy None"
            );
        })
        .unwrap();

    let invalid_mtproto = ProxyEndpointRecord {
        id: 9206,
        kind: "MTProto".into(),
        host: "not a valid host".into(),
        port: 443,
        secret_or_user: "ee1122".into(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(invalid_mtproto), cx);
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Mtproto);
            assert_eq!(sa.proxy_host, "mt.telegram.example");
            assert_eq!(sa.proxy_port, 443);
            let toast_msg = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            assert!(toast_msg.contains("Proxy error:"));
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    let disk_after_mtproto = SessionPersistenceBridge::new(srv_url.clone());
    assert!(!disk_after_mtproto.state.proxies.iter().any(|p| p.id == 9206));
    assert!(disk_after_mtproto
        .state
        .proxies
        .iter()
        .any(|p| p.id == 9205 && p.enabled));

    // Sub-case D: Concurrent same-session AddProxy + SelectProxy race
    let hold_probe = Arc::new(AtomicBool::new(true));
    let fail_probe = Arc::new(AtomicBool::new(false));
    let probe_arrived = Arc::new(AtomicUsize::new(0));
    let held_proxy_listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind held proxy srv");
    let held_proxy_port = held_proxy_listener.local_addr().unwrap().port();
    {
        let hold_srv = Arc::clone(&hold_probe);
        let fail_srv = Arc::clone(&fail_probe);
        let arr_srv = Arc::clone(&probe_arrived);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = held_proxy_listener.accept() {
                let hold_srv = Arc::clone(&hold_srv);
                let fail_srv = Arc::clone(&fail_srv);
                let arr_srv = Arc::clone(&arr_srv);
                std::thread::spawn(move || {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf);
                    arr_srv.fetch_add(1, Ordering::SeqCst);
                    while hold_srv.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if !fail_srv.load(Ordering::SeqCst) {
                        let _ = stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
                        let _ = stream.flush();
                    }
                });
            }
        });
    }

    // D1: Slow AddProxy(9207) superseded by SelectProxy(0 = Direct)
    hold_probe.store(true, Ordering::SeqCst);
    fail_probe.store(false, Ordering::SeqCst);
    probe_arrived.store(0, Ordering::SeqCst);
    let slow_9207 = ProxyEndpointRecord {
        id: 9207,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: held_proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(slow_9207), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || probe_arrived.load(Ordering::SeqCst) >= 1).await,
        "held probe for 9207 did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 0 }, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    let ui_direct = s
                        .settings_router
                        .read(cx)
                        .settings_advanced
                        .as_ref()
                        .is_some_and(|sa| sa.read(cx).proxy_type == ProxyType::Direct);
                    ui_direct
                        && s.client.http_client.proxy().is_none()
                        && s.client.ws_transport.proxy().is_none()
                        && s.window_runtime.controller.current_toast.is_some()
                })
                .unwrap_or(false)
        })
        .await,
        "SelectProxy(0) Direct did not converge"
    );
    shell
        .update(&mut app.clone(), |s, _| {
            s.window_runtime.controller.clear_all_toasts();
        })
        .unwrap();
    hold_probe.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Direct);
            assert!(s.client.http_client.proxy().is_none());
            assert!(s.client.ws_transport.proxy().is_none());
            let toast_txt = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            assert!(
                !toast_txt.contains("Proxy saved") && !toast_txt.contains("代理节点已保存"),
                "superseded AddProxy(9207) must NOT show saved toast: {toast_txt:?}"
            );
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(
                !disk.state.proxies.iter().any(|p| p.id == 9207),
                "superseded AddProxy(9207) must NOT persist to disk"
            );
        })
        .unwrap();

    // D2: Slow failing AddProxy(9208) superseded by SelectProxy(9201)
    hold_probe.store(true, Ordering::SeqCst);
    fail_probe.store(true, Ordering::SeqCst);
    probe_arrived.store(0, Ordering::SeqCst);
    let slow_9208 = ProxyEndpointRecord {
        id: 9208,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: held_proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(slow_9208), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || probe_arrived.load(Ordering::SeqCst) >= 1).await,
        "held probe for 9208 did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9201 }, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    let ui_9201 = s
                        .settings_router
                        .read(cx)
                        .settings_advanced
                        .as_ref()
                        .is_some_and(|sa| {
                            let r = sa.read(cx);
                            r.proxy_type == ProxyType::Http && r.proxy_port == proxy_port
                        });
                    ui_9201
                        && s.client
                            .http_client
                            .proxy()
                            .is_some_and(|p| p.port == proxy_port)
                        && s.client
                            .ws_transport
                            .proxy()
                            .is_some_and(|p| p.port == proxy_port)
                })
                .unwrap_or(false)
        })
        .await,
        "SelectProxy(9201) did not converge"
    );
    hold_probe.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Http);
            assert_eq!(sa.proxy_host, "127.0.0.1");
            assert_eq!(sa.proxy_port, proxy_port);
            assert_eq!(s.client.http_client.proxy().unwrap().port, proxy_port);
            assert_eq!(s.client.ws_transport.proxy().unwrap().port, proxy_port);
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(!disk.state.proxies.iter().any(|p| p.id == 9208));
            assert!(disk.state.proxies.iter().any(|p| p.id == 9201 && p.enabled));
        })
        .unwrap();

    // Sub-case E: URL metacharacters, host:port, scheme://, whitespace, CRLF, and IPv6 ::1
    let bad_hosts = [
        "evil.com@127.0.0.1",
        "127.0.0.1/path",
        "127.0.0.1?q=1",
        "127.0.0.1#frag",
        "http://127.0.0.1",
        "127.0.0.1:8080",
        " 127.0.0.1 ",
        "127.0.0.1\r\n",
    ];
    for (idx, bad_host) in bad_hosts.iter().enumerate() {
        let bad_rec = ProxyEndpointRecord {
            id: 9300 + idx as u32,
            kind: "HTTP".into(),
            host: (*bad_host).to_string(),
            port: 1080,
            secret_or_user: String::new(),
            username: None,
            password: None,
            ping_ms: 0,
            enabled: true,
        };
        shell
            .update(&mut app.clone(), |s, cx| {
                s.window_runtime.controller.clear_all_toasts();
                s.handle_modal_outcome(ModalActionOutcome::AddProxy(bad_rec), cx);
                let sa = s
                    .settings_router
                    .read(cx)
                    .settings_advanced
                    .as_ref()
                    .unwrap()
                    .read(cx);
                assert_eq!(sa.proxy_type, ProxyType::Http);
                assert_eq!(sa.proxy_host, "127.0.0.1");
                assert_eq!(sa.proxy_port, proxy_port);
                let toast_msg = s
                    .window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                assert!(
                    toast_msg.contains("Proxy error:"),
                    "bad host {bad_host:?} must immediately surface Proxy error toast, got: {toast_msg:?}"
                );
            })
            .unwrap();
    }
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    shell
        .read_with(&app, |s, _| {
            assert_eq!(s.client.http_client.proxy().unwrap().port, proxy_port);
            assert_eq!(s.client.ws_transport.proxy().unwrap().port, proxy_port);
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(
                !disk.state.proxies.iter().any(|p| p.id >= 9300 && p.id < 9399),
                "bad hosts must not persist to disk"
            );
        })
        .unwrap();

    let ep_e = shell.read_with(&app, |s, _| s.client.session_epoch()).unwrap();
    let cl_e = shell.read_with(&app, |s, _| s.client.clone()).unwrap();
    for (idx, bad_host) in ["127.0.0.1\r\n", "evil.com@127.0.0.1"].iter().enumerate() {
        let rec = ProxyEndpointRecord {
            id: 9350 + idx as u32,
            kind: "HTTP".into(),
            host: (*bad_host).to_string(),
            port: 1080,
            secret_or_user: String::new(),
            username: None,
            password: None,
            ping_ms: 0,
            enabled: true,
        };
        assert!(
            cl_e.upsert_proxy_for_session(Some("tok-a"), ep_e, rec)
                .await
                .is_err(),
            "upsert_proxy_for_session must reject {bad_host:?}"
        );
    }

    // Positive control: IPv6 "::1" disabled proxy persists cleanly
    let ipv6_rec = ProxyEndpointRecord {
        id: 9399,
        kind: "HTTP".into(),
        host: "::1".into(),
        port: 1080,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: false,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(ipv6_rec), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            SessionPersistenceBridge::new(srv_url.clone())
                .state
                .proxies
                .iter()
                .any(|p| p.id == 9399 && p.host == "::1")
        })
        .await,
        "valid IPv6 ::1 proxy must persist cleanly"
    );

    // Sub-case F: Direct User Switch + uninitialized settings_advanced MTProto fallback
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9205 }, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            SessionPersistenceBridge::new(srv_url.clone())
                .state
                .proxies
                .iter()
                .any(|p| p.id == 9205 && p.enabled)
                && shell
                    .read_with(&app, |s, _| s.client.http_client.proxy().is_none())
                    .unwrap_or(false)
        })
        .await,
        "MTProto 9205 did not re-activate for Sub-case F"
    );

    // Uninitialized settings_advanced + uncached SelectProxy(9999) leaves UI None & MTProto active
    shell
        .update(&mut app.clone(), |s, cx| {
            s.settings_router.update(cx, |r, _| r.settings_advanced = None);
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9999 }, cx);
        })
        .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    shell
        .read_with(&app, |s, cx| {
            assert!(s.settings_router.read(cx).settings_advanced.is_none());
            assert!(SessionPersistenceBridge::new(srv_url.clone())
                .state
                .proxies
                .iter()
                .any(|p| p.id == 9205 && p.enabled));
        })
        .unwrap();

    // Start held AddProxy(9401) (materializes settings_advanced optimistically to HTTP 9401),
    // then immediately perform direct user switch (no logout!) -> settings_advanced must sync
    // synchronously to active disk MTProto 9205, and late 9401 must be fenced.
    hold_probe.store(true, Ordering::SeqCst);
    fail_probe.store(false, Ordering::SeqCst);
    probe_arrived.store(0, Ordering::SeqCst);
    let held_9401 = ProxyEndpointRecord {
        id: 9401,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: held_proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(held_9401), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || probe_arrived.load(Ordering::SeqCst) >= 1).await,
        "held probe for 9401 did not arrive"
    );

    let auth_b = AuthResponse {
        user_id: 2002,
        username: "bob".into(),
        session_id: "sess-b".into(),
        token: "tok-b".into(),
        first_name: None,
        created_at: None,
        expires_at: None,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_b.clone(), cx);
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .expect("settings_advanced present")
                .read(cx);
            assert_eq!(
                sa.proxy_type,
                ProxyType::Mtproto,
                "direct user switch must sync settings_advanced back to active MTProto proxy"
            );
            assert_eq!(sa.proxy_host, "mt.telegram.example");
            assert_eq!(sa.proxy_port, 443);
        })
        .unwrap();

    hold_probe.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Mtproto);
            assert_eq!(sa.proxy_host, "mt.telegram.example");
            assert_eq!(sa.proxy_port, 443);
            assert!(s.client.http_client.proxy().is_none());
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(!disk.state.proxies.iter().any(|p| p.id == 9401));
            assert!(disk.state.proxies.iter().any(|p| p.id == 9205 && p.enabled));
        })
        .unwrap();

    // Sub-case G: Optimistic AddProxy(enabled: true) superseded by [P8], [P9], [P10], [P5]
    // plus uniform host metacharacter rejection across HTTP, SOCKS5, and MTProto.
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 0 }, cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    let ui_direct = s
                        .settings_router
                        .read(cx)
                        .settings_advanced
                        .as_ref()
                        .is_some_and(|sa| sa.read(cx).proxy_type == ProxyType::Direct);
                    ui_direct
                        && s.client.http_client.proxy().is_none()
                        && s.client.ws_transport.proxy().is_none()
                        && !SessionPersistenceBridge::new(srv_url.clone())
                            .state
                            .proxies
                            .iter()
                            .any(|p| p.enabled)
                })
                .unwrap_or(false)
        })
        .await,
        "reset to Direct before Sub-case G did not converge"
    );

    // [P8]: Held AddProxy(9501, enabled: true) superseded by AddProxy(9502, enabled: false)
    hold_probe.store(true, Ordering::SeqCst);
    fail_probe.store(false, Ordering::SeqCst);
    probe_arrived.store(0, Ordering::SeqCst);
    let held_9501 = ProxyEndpointRecord {
        id: 9501,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: held_proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(held_9501), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || probe_arrived.load(Ordering::SeqCst) >= 1).await,
        "[P8] held probe for 9501 did not arrive"
    );
    let disabled_9502 = ProxyEndpointRecord {
        id: 9502,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: false,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(disabled_9502), cx);
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .expect("settings_advanced present")
                .read(cx);
            assert_eq!(
                sa.proxy_type,
                ProxyType::Direct,
                "[P8] AddProxy(enabled: false) must synchronously revert settings_advanced to Direct"
            );
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            SessionPersistenceBridge::new(srv_url.clone())
                .state
                .proxies
                .iter()
                .any(|p| p.id == 9502 && !p.enabled)
        })
        .await,
        "[P8] disabled proxy 9502 did not persist"
    );
    hold_probe.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Direct);
            assert!(s.client.http_client.proxy().is_none());
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(!disk.state.proxies.iter().any(|p| p.id == 9501));
            assert!(disk.state.proxies.iter().any(|p| p.id == 9502 && !p.enabled));
        })
        .unwrap();

    // [P9]: Held AddProxy(9503, enabled: true) superseded by uncached SelectProxy(9999)
    hold_probe.store(true, Ordering::SeqCst);
    probe_arrived.store(0, Ordering::SeqCst);
    let held_9503 = ProxyEndpointRecord {
        id: 9503,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: held_proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(held_9503), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || probe_arrived.load(Ordering::SeqCst) >= 1).await,
        "[P9] held probe for 9503 did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.modal_container.update(cx, |m, _| m.dialog.proxies.clear());
            s.handle_modal_outcome(ModalActionOutcome::SelectProxy { proxy_id: 9999 }, cx);
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(
                sa.proxy_type,
                ProxyType::Direct,
                "[P9] uncached SelectProxy(9999) must synchronously revert settings_advanced to Direct"
            );
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| {
                    s.window_runtime
                        .controller
                        .current_toast
                        .as_ref()
                        .is_some_and(|t| {
                            t.text.contains("Selected proxy not found")
                                || t.text.contains("未找到所选代理节点")
                        })
                })
                .unwrap_or(false)
        })
        .await,
        "[P9] uncached SelectProxy(9999) NotFound toast did not appear"
    );
    hold_probe.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Direct);
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(!disk.state.proxies.iter().any(|p| p.id == 9503));
        })
        .unwrap();

    // [P10]: Held AddProxy(9504, enabled: true) superseded by same-user apply_authenticated_session
    hold_probe.store(true, Ordering::SeqCst);
    probe_arrived.store(0, Ordering::SeqCst);
    let held_9504 = ProxyEndpointRecord {
        id: 9504,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: held_proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(held_9504), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || probe_arrived.load(Ordering::SeqCst) >= 1).await,
        "[P10] held probe for 9504 did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_b.clone(), cx);
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(
                sa.proxy_type,
                ProxyType::Direct,
                "[P10] same-user apply_authenticated_session must synchronously revert settings_advanced to Direct"
            );
        })
        .unwrap();
    hold_probe.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Direct);
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(!disk.state.proxies.iter().any(|p| p.id == 9504));
        })
        .unwrap();

    // [P5]: Held AddProxy(9505, enabled: true) superseded by AuthWizardEvent::ServerUrlApplied
    hold_probe.store(true, Ordering::SeqCst);
    probe_arrived.store(0, Ordering::SeqCst);
    let held_9505 = ProxyEndpointRecord {
        id: 9505,
        kind: "HTTP".into(),
        host: "127.0.0.1".into(),
        port: held_proxy_port,
        secret_or_user: String::new(),
        username: None,
        password: None,
        ping_ms: 0,
        enabled: true,
    };
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_modal_outcome(ModalActionOutcome::AddProxy(held_9505), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || probe_arrived.load(Ordering::SeqCst) >= 1).await,
        "[P5] held probe for 9505 did not arrive"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(srv_url.clone()), cx);
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(
                sa.proxy_type,
                ProxyType::Direct,
                "[P5] ServerUrlApplied must synchronously revert settings_advanced to Direct"
            );
        })
        .unwrap();
    hold_probe.store(false, Ordering::SeqCst);
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    shell
        .read_with(&app, |s, cx| {
            let sa = s
                .settings_router
                .read(cx)
                .settings_advanced
                .as_ref()
                .unwrap()
                .read(cx);
            assert_eq!(sa.proxy_type, ProxyType::Direct);
            assert!(s.client.http_client.proxy().is_none());
            assert!(s.client.ws_transport.proxy().is_none());
            let disk = SessionPersistenceBridge::new(srv_url.clone());
            assert!(!disk.state.proxies.iter().any(|p| p.id == 9505));
        })
        .unwrap();

    // Uniform host metacharacter rejection across HTTP, SOCKS5, and MTProto via AddProxy
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_b.clone(), cx);
        })
        .unwrap();
    let uniform_bad_hosts = [
        "127.0.0.1%2f",
        "evil.com%40127.0.0.1",
        "127.0.0.1%0d%0a",
        "127.0.0.1,evil.com",
        "127.0.0.1;evil.com",
        "127.0.0.1&port=1",
        "127.0.0.1=evil",
    ];
    let mut reject_id = 9510u32;
    for kind in ["HTTP", "SOCKS5", "MTProto"] {
        for bad_host in uniform_bad_hosts {
            let rec_id = reject_id;
            reject_id += 1;
            let rec = ProxyEndpointRecord {
                id: rec_id,
                kind: kind.into(),
                host: bad_host.into(),
                port: 1080,
                secret_or_user: String::new(),
                username: None,
                password: None,
                ping_ms: 0,
                enabled: true,
            };
            shell
                .update(&mut app.clone(), |s, cx| {
                    s.window_runtime.controller.clear_all_toasts();
                    s.handle_modal_outcome(ModalActionOutcome::AddProxy(rec), cx);
                    let sa = s
                        .settings_router
                        .read(cx)
                        .settings_advanced
                        .as_ref()
                        .unwrap()
                        .read(cx);
                    assert_eq!(sa.proxy_type, ProxyType::Direct);
                    let toast_msg = s
                        .window_runtime
                        .controller
                        .current_toast
                        .as_ref()
                        .map(|t| t.text.clone())
                        .unwrap_or_default();
                    assert!(
                        toast_msg.contains("Proxy error:"),
                        "uniform rejection failed for kind={kind} host={bad_host:?}: toast={toast_msg:?}"
                    );
                })
                .unwrap();
        }
    }
    let disk_after_uniform = SessionPersistenceBridge::new(srv_url.clone());
    assert!(
        !disk_after_uniform
            .state
            .proxies
            .iter()
            .any(|p| p.id >= 9510 && p.id < reject_id),
        "uniformly rejected hosts must not persist to disk"
    );

    // Sub-case H: Reject invalid MTProto secret_or_user injection vectors via AddProxy
    let bad_mtproto_secrets = ["u&port=99", "u\r\nx", "u@a/b?c#d"];
    let mut bad_secret_id = 9600u32;
    for bad_secret in bad_mtproto_secrets {
        let rec_id = bad_secret_id;
        bad_secret_id += 1;
        let rec = ProxyEndpointRecord {
            id: rec_id,
            kind: "MTProto".into(),
            host: "mt.telegram.example".into(),
            port: 443,
            secret_or_user: bad_secret.into(),
            username: None,
            password: None,
            ping_ms: 0,
            enabled: true,
        };
        shell
            .update(&mut app.clone(), |s, cx| {
                s.window_runtime.controller.clear_all_toasts();
                s.handle_modal_outcome(ModalActionOutcome::AddProxy(rec), cx);
                let sa = s
                    .settings_router
                    .read(cx)
                    .settings_advanced
                    .as_ref()
                    .unwrap()
                    .read(cx);
                assert_eq!(sa.proxy_type, ProxyType::Direct);
                let toast_msg = s
                    .window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                assert!(
                    toast_msg.contains("Proxy error:"),
                    "MTProto invalid secret {bad_secret:?} must surface Proxy error toast: toast={toast_msg:?}"
                );
            })
            .unwrap();
    }
    let disk_after_bad_secret = SessionPersistenceBridge::new(srv_url.clone());
    assert!(
        !disk_after_bad_secret
            .state
            .proxies
            .iter()
            .any(|p| p.id >= 9600 && p.id < bad_secret_id),
        "rejected MTProto secret records must not persist to disk"
    );
}

// [T5-54] [D4] Non-composer outgoing sends preserve unsent composer draft, and reconcile_chat_scoped_toasts clears stale chat toasts.
#[test]
fn t5_54_non_composer_sends_preserve_draft_and_toast_reconciliation() {
    run_in_headless_subprocess(
        "t5_54_non_composer_sends_preserve_draft_and_toast_reconciliation",
    );
}

async fn t5_54_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::app_shell::shell_truth::select_authoritative_dialog;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let recorded_reqs: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_srv = Arc::clone(&recorded_reqs);
    let stored_draft: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stored_draft_srv = Arc::clone(&stored_draft);
    let next_msg_id = Arc::new(AtomicUsize::new(9001));
    let next_msg_id_srv = Arc::clone(&next_msg_id);

    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind api srv");
    let srv_url = format!("http://{}", api_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            let recorded_srv = Arc::clone(&recorded_srv);
            let stored_draft_srv = Arc::clone(&stored_draft_srv);
            let next_msg_id_srv = Arc::clone(&next_msg_id_srv);
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let hend = pos + 4;
                            header_end = Some(hend);
                            let hdr_str = String::from_utf8_lossy(&raw[..hend]);
                            for line in hdr_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(hend) = header_end {
                        if raw.len() >= hend + content_len {
                            break;
                        }
                    }
                }
                let hend = header_end.unwrap_or(raw.len());
                let hdr_str = String::from_utf8_lossy(&raw[..hend]).to_string();
                let body_str = String::from_utf8_lossy(&raw[hend..raw.len().min(hend + content_len)])
                    .to_string();
                let first_line = hdr_str.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();

                recorded_srv
                    .lock()
                    .unwrap()
                    .push((method.clone(), path.clone(), body_str.clone()));

                let resp_body = if method == "POST" && path == "/api/v1/messages/send" {
                    let mid = next_msg_id_srv.fetch_add(1, Ordering::SeqCst) as i64;
                    let text_val = serde_json::from_str::<serde_json::Value>(&body_str)
                        .ok()
                        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                        .unwrap_or_default();
                    serde_json::json!({
                        "message_id": mid,
                        "id": mid,
                        "pts": mid,
                        "pts_count": 1,
                        "duplicate": false,
                        "date_ms": 1700000000000i64,
                        "date": "2026-11-24T00:00:00Z",
                        "sender_id": UID_VIEWER,
                        "chat_id": 101,
                        "text": text_val,
                        "status": "sent"
                    })
                    .to_string()
                } else if method == "PUT" && path == "/api/v1/chats/101/draft" {
                    let stored = if body_str.trim().is_empty() {
                        r#"{"chat_id":101,"text":"","entities":[],"date_ms":1700000000000}"#.to_string()
                    } else {
                        body_str
                    };
                    *stored_draft_srv.lock().unwrap() = Some(stored.clone());
                    stored
                } else if method == "GET" && path == "/api/v1/chats/101/draft" {
                    match stored_draft_srv.lock().unwrap().clone() {
                        Some(d) => format!(r#"{{"draft":{d}}}"#),
                        None => r#"{"draft":null}"#.to_string(),
                    }
                } else if path == "/api/v1/chats" {
                    r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#.to_string()
                } else if path == "/api/v1/user-vault/envelope" {
                    r#"{"envelope":null}"#.to_string()
                } else if path.contains("/messages") || path.contains("/history") || path.contains("/members") {
                    "[]".to_string()
                } else {
                    "{}".to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    let client = new_isolated_client(srv_url);
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |s, cx| {
            s.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            s.stage.update(cx, |st, _| {
                st.is_bot_chat = true;
                st.show_bot_start_bar = false;
            });
        })
        .unwrap();
    pump_until_settled(&app, 40, || recorded_reqs.lock().unwrap().len()).await;

    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.set_text("unsent user draft");
                ccx.notify();
            });
            s.flush_pending_draft_for_chat(CID_FABUSHI, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains("unsent user draft")
            })
        })
        .await,
        "initial draft save did not reach mock server"
    );

    // Sub-case A: BotKeyboard press [D4]
    let kb_rows = vec![vec![BotKeyboardButton::new(
        "Quick Reply",
        BotKeyboardButtonKind::Default,
    )]];
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.set_bot_reply_keyboard(kb_rows, kb_flags::RESIZE, Some("Choose".into()));
                c.pending_bot_keyboard_press =
                    Some(BotKeyboardButton::new("Quick Reply", BotKeyboardButtonKind::Default));
                ccx.notify();
            });
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            let req_ok = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "POST" && p == "/api/v1/messages/send" && b.contains("Quick Reply")
            });
            let msg_ok = shell
                .read_with(&app, |s, cx| {
                    s.stage.read(cx).messages.iter().any(|m| {
                        matches!(&m.content, MessageContent::Text { text, .. } if text == "Quick Reply")
                            && m.delivery_status == DeliveryStatus::Sent
                    })
                })
                .unwrap_or(false);
            req_ok && msg_ok
        })
        .await,
        "[D4 Sub-case A] Quick Reply send did not complete"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.composer.read(cx).text(), "unsent user draft");
            assert_eq!(
                s.session_sync
                    .last_draft_text_by_chat
                    .get(&CID_FABUSHI)
                    .map(String::as_str),
                Some("unsent user draft")
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&CID_FABUSHI).map(String::as_str),
                Some("unsent user draft")
            );
            assert_eq!(
                s.client
                    .local_store
                    .get_draft(ChatId(CID_FABUSHI as i64))
                    .map(|d| d.text),
                Some("unsent user draft".to_string())
            );
        })
        .unwrap();
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "[D4 Sub-case A] BotKeyboard send must NOT issue empty PUT /draft"
    );

    // Sub-case B: Sticker + Inspector RunBotCommand [D4]
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_send_sticker_emoji = Some("🔥".into());
                ccx.notify();
            });
            s.handle_inspector_action(InspectorAction::RunBotCommand("status".into()), cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            let sticker_ok = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "POST" && p == "/api/v1/messages/send" && b.contains("🔥")
            });
            let cmd_ok = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "POST" && p == "/api/v1/messages/send" && b.contains("/status")
            });
            let stage_ok = shell
                .read_with(&app, |s, cx| {
                    let msgs = &s.stage.read(cx).messages;
                    let has_sticker = msgs.iter().any(|m| {
                        matches!(&m.content, MessageContent::Text { text, .. } if text == "🔥")
                            && m.delivery_status == DeliveryStatus::Sent
                    });
                    let has_cmd = msgs.iter().any(|m| {
                        matches!(&m.content, MessageContent::Text { text, .. } if text == "/status")
                            && m.delivery_status == DeliveryStatus::Sent
                    });
                    has_sticker && has_cmd
                })
                .unwrap_or(false);
            sticker_ok && cmd_ok && stage_ok
        })
        .await,
        "[D4 Sub-case B] Sticker + RunBotCommand sends did not complete"
    );
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;

    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.composer.read(cx).text(), "unsent user draft");
            assert_eq!(
                s.session_sync
                    .last_draft_text_by_chat
                    .get(&CID_FABUSHI)
                    .map(String::as_str),
                Some("unsent user draft")
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&CID_FABUSHI).map(String::as_str),
                Some("unsent user draft")
            );
            assert_eq!(
                s.client
                    .local_store
                    .get_draft(ChatId(CID_FABUSHI as i64))
                    .map(|d| d.text),
                Some("unsent user draft".to_string())
            );
        })
        .unwrap();
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "[D4 Sub-case B] Sticker/Inspector sends must NOT issue empty PUT /draft"
    );

    // Sub-case C: Real composer send DOES clear draft
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.submitted_message = Some("unsent user draft".into());
                ccx.notify();
            });
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            let empty_put = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
            });
            let cleared_local = shell
                .read_with(&app, |s, cx| {
                    s.composer.read(cx).text().is_empty()
                        && s.client.local_store.get_draft(ChatId(CID_FABUSHI as i64)).is_none()
                })
                .unwrap_or(false);
            empty_put && cleared_local
        })
        .await,
        "[D4 Sub-case C] real composer send must clear local and server draft"
    );

    // Sub-case D: reconcile_chat_scoped_toasts clears chat 101 toast on direct select_authoritative_dialog while preserving global toast
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.show_chat_toast(CID_FABUSHI, "scoped toast", 3000, cx);
            s.show_non_chat_toast("global toast", 3000, cx);

            let sb_entity = s.sidebar.clone();
            let st_entity = s.stage.clone();
            let insp_entity = s.inspector.clone();
            sb_entity.update(cx, |sb, sb_cx| {
                st_entity.update(sb_cx, |st, st_cx| {
                    insp_entity.update(st_cx, |insp, _| {
                        assert!(select_authoritative_dialog(sb, st, insp, 102));
                    });
                });
            });

            s.reconcile_chat_scoped_toasts(cx);

            let cur = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("global toast must promote into current_toast");
            assert_eq!(cur.text, "global toast");
            assert_eq!(cur.chat_id, None);
            assert!(
                s.window_runtime.controller.toast_queue.is_empty(),
                "chat 101 scoped toast must be purged from toast_queue"
            );
            assert_eq!(s.last_observed_toast_chat_id, Some(102));
        })
        .unwrap();
}

// [T5-55] Panel/lockscreen chat-scoped toast reconciliation + SendSilent/ScheduleMessage failure toast & draft preservation.
#[test]
fn t5_55_panel_lock_toast_reconciliation_and_silent_schedule_send_errors() {
    run_in_headless_subprocess(
        "t5_55_panel_lock_toast_reconciliation_and_silent_schedule_send_errors",
    );
}

async fn t5_55_body(app: AsyncApp) {
    use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    let recorded_reqs: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_srv = Arc::clone(&recorded_reqs);
    let send_500 = Arc::new(AtomicBool::new(true));
    let send_500_srv = Arc::clone(&send_500);
    let stored_draft: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let stored_draft_srv = Arc::clone(&stored_draft);
    let next_msg_id = Arc::new(AtomicUsize::new(9501));
    let next_msg_id_srv = Arc::clone(&next_msg_id);

    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind api srv");
    let srv_url = format!("http://{}", api_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            let recorded_srv = Arc::clone(&recorded_srv);
            let send_500_srv = Arc::clone(&send_500_srv);
            let stored_draft_srv = Arc::clone(&stored_draft_srv);
            let next_msg_id_srv = Arc::clone(&next_msg_id_srv);
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let hend = pos + 4;
                            header_end = Some(hend);
                            let hdr_str = String::from_utf8_lossy(&raw[..hend]);
                            for line in hdr_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(hend) = header_end {
                        if raw.len() >= hend + content_len {
                            break;
                        }
                    }
                }
                let hend = header_end.unwrap_or(raw.len());
                let hdr_str = String::from_utf8_lossy(&raw[..hend]).to_string();
                let body_str = String::from_utf8_lossy(&raw[hend..raw.len().min(hend + content_len)])
                    .to_string();
                let first_line = hdr_str.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();

                recorded_srv
                    .lock()
                    .unwrap()
                    .push((method.clone(), path.clone(), body_str.clone()));

                let (status_line, resp_body) = if method == "POST" && path == "/api/v1/messages/send" {
                    if send_500_srv.load(Ordering::SeqCst) {
                        (
                            "500 Internal Server Error",
                            r#"{"error":"simulated send 500"}"#.to_string(),
                        )
                    } else {
                        let mid = next_msg_id_srv.fetch_add(1, Ordering::SeqCst) as i64;
                        let text_val = serde_json::from_str::<serde_json::Value>(&body_str)
                            .ok()
                            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                            .unwrap_or_default();
                        (
                            "200 OK",
                            serde_json::json!({
                                "message_id": mid,
                                "id": mid,
                                "pts": mid,
                                "pts_count": 1,
                                "duplicate": false,
                                "date_ms": 1700000000000i64,
                                "date": "2026-11-24T00:00:00Z",
                                "sender_id": UID_VIEWER,
                                "chat_id": 101,
                                "text": text_val,
                                "status": "sent"
                            })
                            .to_string(),
                        )
                    }
                } else if method == "PUT" && path == "/api/v1/chats/101/draft" {
                    let stored = if body_str.trim().is_empty() {
                        r#"{"chat_id":101,"text":"","entities":[],"date_ms":1700000000000}"#.to_string()
                    } else {
                        body_str
                    };
                    *stored_draft_srv.lock().unwrap() = Some(stored.clone());
                    ("200 OK", stored)
                } else if method == "GET" && path == "/api/v1/chats/101/draft" {
                    let body = match stored_draft_srv.lock().unwrap().clone() {
                        Some(d) => format!(r#"{{"draft":{d}}}"#),
                        None => r#"{"draft":null}"#.to_string(),
                    };
                    ("200 OK", body)
                } else if path == "/api/v1/chats" {
                    (
                        "200 OK",
                        r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#.to_string(),
                    )
                } else if path == "/api/v1/user-vault/envelope" {
                    ("200 OK", r#"{"envelope":null}"#.to_string())
                } else if path.contains("/messages") || path.contains("/history") || path.contains("/members") {
                    ("200 OK", "[]".to_string())
                } else {
                    ("200 OK", "{}".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    let client = new_isolated_client(srv_url);
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Select Chat 101 cleanly
    shell
        .update(&mut app.clone(), |s, cx| {
            s.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    // Sub-case A: Non-chat panels clear chat-scoped toasts & preserve global toasts
    shell
        .update(&mut app.clone(), |s, cx| {
            let assert_panel_clears_chat_toast =
                |s: &mut AppShell, label: &str, switch_fn: &dyn Fn(&mut AppShell, &mut gpui::Context<AppShell>), cx: &mut gpui::Context<AppShell>| {
                    s.activate_nav_destination(NavDestination::Chats, cx);
                    s.window_runtime.controller.clear_all_toasts();
                    s.show_chat_toast(CID_FABUSHI, format!("chat 101 error ({label})"), 4000, cx);
                    s.show_non_chat_toast(format!("global notice ({label})"), 4000, cx);

                    switch_fn(s, cx);

                    assert!(
                        !s.is_chat_visible_for_toast(CID_FABUSHI, cx),
                        "{label}: chat 101 must not be toast-visible"
                    );
                    let cur = s
                        .window_runtime
                        .controller
                        .current_toast
                        .as_ref()
                        .expect("global toast must promote into current_toast");
                    assert_eq!(cur.text, format!("global notice ({label})"));
                    assert_eq!(cur.chat_id, None);
                    assert!(!cur.allow_chat_auto_scope);
                    assert!(
                        s.window_runtime.controller.toast_queue.is_empty(),
                        "{label}: chat-scoped toast must be purged from queue"
                    );
                    let rendered = s
                        .window_runtime
                        .controller
                        .current_toast
                        .clone()
                        .filter(|t| t.chat_id.is_none_or(|cid| s.is_chat_visible_for_toast(cid, cx)));
                    assert_eq!(
                        rendered.as_ref().map(|t| t.text.as_str()),
                        Some(format!("global notice ({label})").as_str())
                    );
                };

            assert_panel_clears_chat_toast(s, "nav_settings", &|s, cx| {
                s.activate_nav_destination(NavDestination::Settings, cx);
            }, cx);
            assert_panel_clears_chat_toast(s, "nav_calls", &|s, cx| {
                s.activate_nav_destination(NavDestination::Calls, cx);
            }, cx);
            assert_panel_clears_chat_toast(s, "nav_tasks", &|s, cx| {
                s.activate_nav_destination(NavDestination::TaskCenter, cx);
            }, cx);
            assert_panel_clears_chat_toast(s, "nav_mcp", &|s, cx| {
                s.activate_nav_destination(NavDestination::McpTools, cx);
            }, cx);
            assert_panel_clears_chat_toast(s, "navigate_settings_to", &|s, cx| {
                s.navigate_settings_to(SettingsSectionType::Main, cx);
            }, cx);
            assert_panel_clears_chat_toast(s, "main_menu_calls", &|s, cx| {
                s.handle_main_menu_command(MainMenuCommand::Calls, cx);
            }, cx);

            // Direct active_panel mutation + reconcile_chat_scoped_toasts + switch back to Chats (no zombie toast)
            s.activate_nav_destination(NavDestination::Chats, cx);
            s.window_runtime.controller.clear_all_toasts();
            s.show_chat_toast(CID_FABUSHI, "chat 101 direct panel error", 4000, cx);
            s.active_panel = ActiveWorkspacePanel::Settings;
            s.reconcile_chat_scoped_toasts(cx);
            assert!(s.window_runtime.controller.current_toast.is_none());
            s.active_panel = ActiveWorkspacePanel::Chats;
            s.reconcile_chat_scoped_toasts(cx);
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "purged toast must not resurrect when switching back to Chats"
            );
        })
        .unwrap();

    // Sub-case B: PasscodeLock via lock_app_now clears chat-scoped toasts & preserves global toasts
    shell
        .update(&mut app.clone(), |s, cx| {
            s.activate_nav_destination(NavDestination::Chats, cx);
            s.handle_modal_outcome(ModalActionOutcome::SetLocalPasscode(Some("2580".into())), cx);
            s.window_runtime.controller.clear_all_toasts();
            s.show_chat_toast(CID_FABUSHI, "sensitive chat toast", 4000, cx);
            s.show_non_chat_toast("global lock toast", 4000, cx);

            assert!(s.lock_app_now(cx));
            assert!(s.overlays.is_passcode_locked);
            assert!(!s.is_chat_visible_for_toast(CID_FABUSHI, cx));
            let cur = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("global lock toast must remain visible");
            assert_eq!(cur.text, "global lock toast");
            assert_eq!(cur.chat_id, None);
            assert!(s.window_runtime.controller.toast_queue.is_empty());

            assert!(s.try_unlock_with_passcode("2580", cx));
            assert!(!s.overlays.is_passcode_locked);
            s.window_runtime.controller.clear_all_toasts();
            s.show_chat_toast(CID_FABUSHI, "sensitive only toast", 4000, cx);
            assert!(s.lock_app_now(cx));
            assert!(s.window_runtime.controller.current_toast.is_none());
            assert!(s.try_unlock_with_passcode("2580", cx));
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "purged chat toast must not resurrect after unlocking"
            );
        })
        .unwrap();

    // Sub-case C: SendSilent & ScheduleMessage on HTTP 500 vs 200 OK
    send_500.store(true, Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.activate_nav_destination(NavDestination::Chats, cx);
            s.window_runtime.controller.clear_all_toasts();
            s.composer.update(cx, |c, ccx| {
                c.set_text("silent draft".to_string());
                ccx.notify();
            });
            s.flush_pending_draft_for_chat(CID_FABUSHI, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains("silent draft")
            })
        })
        .await,
        "initial silent draft PUT did not arrive"
    );

    recorded_reqs.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("silent draft".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || {
            let sent_silent = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "POST" && p == "/api/v1/messages/send" && b.contains(r#""silent":true"#)
            });
            let has_toast = shell
                .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                .unwrap_or(false);
            sent_silent && has_toast
        })
        .await,
        "SendSilent HTTP 500 must surface error toast"
    );

    shell
        .read_with(&app, |s, cx| {
            let toast = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected error toast on SendSilent 500");
            assert_eq!(toast.chat_id, Some(CID_FABUSHI));
            assert!(
                toast.text.contains("发送失败") || toast.text.contains("Message send failed"),
                "unexpected SendSilent 500 toast: {}",
                toast.text
            );
            assert_eq!(s.composer.read(cx).text(), "silent draft");
            assert_eq!(
                s.session_sync.last_draft_text_by_chat.get(&CID_FABUSHI).map(String::as_str),
                Some("silent draft")
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&CID_FABUSHI).map(String::as_str),
                Some("silent draft")
            );
            assert_eq!(
                s.client.local_store.get_draft(ChatId(CID_FABUSHI as i64)).map(|d| d.text),
                Some("silent draft".to_string())
            );
        })
        .unwrap();
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "SendSilent 500 must NOT emit empty PUT /draft"
    );

    // Repeat for ScheduleMessage on HTTP 500
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.composer.update(cx, |c, ccx| {
                c.set_text("sched draft".to_string());
                ccx.notify();
            });
            s.flush_pending_draft_for_chat(CID_FABUSHI, cx);
        })
        .unwrap();

    assert!(
        pump_until(&app, 3000, || {
            recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains("sched draft")
            })
        })
        .await,
        "initial sched draft PUT did not arrive"
    );

    recorded_reqs.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_schedule_message = Some("sched draft".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || {
            let sent_sched = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "POST"
                    && p == "/api/v1/messages/send"
                    && b.contains(r#""scheduled_at_ms":"#)
                    && !b.contains(r#""scheduled_at_ms":null"#)
            });
            let has_toast = shell
                .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                .unwrap_or(false);
            sent_sched && has_toast
        })
        .await,
        "ScheduleMessage HTTP 500 must surface error toast"
    );

    shell
        .read_with(&app, |s, cx| {
            let toast = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("expected error toast on ScheduleMessage 500");
            assert_eq!(toast.chat_id, Some(CID_FABUSHI));
            assert!(
                toast.text.contains("发送失败") || toast.text.contains("Message send failed"),
                "unexpected ScheduleMessage 500 toast: {}",
                toast.text
            );
            assert_eq!(s.composer.read(cx).text(), "sched draft");
            assert_eq!(
                s.session_sync.last_draft_text_by_chat.get(&CID_FABUSHI).map(String::as_str),
                Some("sched draft")
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&CID_FABUSHI).map(String::as_str),
                Some("sched draft")
            );
            assert_eq!(
                s.client.local_store.get_draft(ChatId(CID_FABUSHI as i64)).map(|d| d.text),
                Some("sched draft".to_string())
            );
        })
        .unwrap();
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "ScheduleMessage 500 must NOT emit empty PUT /draft"
    );

    // HTTP 200 OK path clears draft cleanly and emits no error toast
    send_500.store(false, Ordering::SeqCst);
    recorded_reqs.lock().unwrap().clear();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("sched draft".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();

    assert!(
        pump_until(&app, 4000, || {
            let sent_200 = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "POST" && p == "/api/v1/messages/send" && b.contains(r#""silent":true"#)
            });
            let cleared_put = recorded_reqs.lock().unwrap().iter().any(|(m, p, b)| {
                m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
            });
            let cleared_local = shell
                .read_with(&app, |s, cx| {
                    s.composer.read(cx).text().is_empty()
                        && s.client.local_store.get_draft(ChatId(CID_FABUSHI as i64)).is_none()
                })
                .unwrap_or(false);
            sent_200 && cleared_put && cleared_local
        })
        .await,
        "SendSilent 200 OK must clear server and local draft"
    );
    shell
        .read_with(&app, |s, cx| {
            assert!(s.composer.read(cx).text().is_empty());
            assert!(s.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();
}

// [T5-58] Part A: Unify has_blocking_overlay & synchronous toast reconciliation across all blocking overlays and non-creation modals.
#[test]
fn t5_58_blocking_overlays_and_modals_toast_reconciliation() {
    run_in_headless_subprocess("t5_58_blocking_overlays_and_modals_toast_reconciliation");
}

async fn t5_58_body(app: AsyncApp) {
    use gpui_telegram_desktop::components::ui::layers::LayerStackOption;
    use gpui_telegram_proto::miniapp_contracts::MiniAppWebMcpCommand;
    use std::io::{Read, Write};

    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind api srv");
    let srv_url = format!("http://{}", api_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let hend = pos + 4;
                            header_end = Some(hend);
                            let hdr_str = String::from_utf8_lossy(&raw[..hend]);
                            for line in hdr_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(hend) = header_end {
                        if raw.len() >= hend + content_len {
                            break;
                        }
                    }
                }
                let hend = header_end.unwrap_or(raw.len());
                let hdr_str = String::from_utf8_lossy(&raw[..hend]).to_string();
                let first_line = hdr_str.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let _method = parts.next().unwrap_or("");
                let path = parts.next().unwrap_or("");

                let resp_body = if path == "/api/v1/chats" {
                    r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#.to_string()
                } else if path == "/api/v1/user-vault/envelope" {
                    r#"{"envelope":null}"#.to_string()
                } else if path.contains("/miniapps/") && path.ends_with("/launch") {
                    serde_json::json!({
                        "app": {
                            "app_id": "fabushi_main_app",
                            "bot_user_id": UID_FABUSHI,
                            "bot_username": "fabushi_bot",
                            "bot_display_name": "全球法布施",
                            "short_name": "fabushi",
                            "title": "全球法布施",
                            "description": "全球法布施小程序",
                            "category": "lifestyle",
                            "icon_emoji": "🪷",
                            "web_url": "/miniapps/fabushi_bot/fabushi",
                            "ui_schema": {
                                "title": "全球法布施",
                                "subtitle": "净心禅室",
                                "theme": {},
                                "widgets": [],
                                "main_button": null,
                                "secondary_button": null,
                                "back_button_visible": false,
                                "settings_button_visible": false,
                                "copilot_suggestions": []
                            },
                            "peer_types": ["users", "bots"],
                            "is_installed": true,
                            "is_main_app": true,
                            "direct_chat_id": CID_FABUSHI
                        },
                        "launch_url": "/miniapps/fabushi_bot/fabushi",
                        "web_app_data": null
                    })
                    .to_string()
                } else if path.contains("/messages") || path.contains("/history") || path.contains("/members") {
                    "[]".to_string()
                } else {
                    "{}".to_string()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    let client = new_isolated_client(srv_url);
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |s, cx| {
            s.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        })
        .unwrap();

    let assert_overlay_gates_toast = |s: &mut AppShell,
                                      label: &str,
                                      open_fn: &dyn Fn(&mut AppShell, &mut gpui::Context<AppShell>),
                                      close_fn: &dyn Fn(&mut AppShell, &mut gpui::Context<AppShell>),
                                      cx: &mut gpui::Context<AppShell>| {
        // Phase 1: chat toast + queued global toast -> open overlay -> global promotes & chat purges
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.window_runtime.controller.clear_all_toasts();
        assert!(
            s.is_chat_visible_for_toast(CID_FABUSHI, cx),
            "{label}: chat 101 must initially be toast-visible"
        );
        assert!(
            !s.has_blocking_overlay(cx),
            "{label}: no blocking overlay initially"
        );
        s.show_chat_toast(CID_FABUSHI, format!("chat101-err-{label}"), 4000, cx);
        s.show_non_chat_toast(format!("global-ok-{label}"), 4000, cx);

        open_fn(s, cx);

        assert!(
            s.has_blocking_overlay(cx),
            "{label}: has_blocking_overlay must be true while open"
        );
        assert!(
            !s.is_chat_visible_for_toast(CID_FABUSHI, cx),
            "{label}: is_chat_visible_for_toast must be false while open"
        );
        let cur = s
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .unwrap_or_else(|| panic!("{label}: global toast must promote immediately"));
        assert_eq!(cur.text, format!("global-ok-{label}"));
        assert_eq!(cur.chat_id, None);
        assert!(!cur.allow_chat_auto_scope);
        assert!(
            s.window_runtime.controller.toast_queue.is_empty(),
            "{label}: chat-scoped toast must be purged from toast_queue"
        );
        let rendered = s
            .window_runtime
            .controller
            .current_toast
            .clone()
            .filter(|t| t.chat_id.is_none_or(|cid| s.is_chat_visible_for_toast(cid, cx)));
        assert_eq!(
            rendered.as_ref().map(|t| t.text.as_str()),
            Some(format!("global-ok-{label}").as_str())
        );

        // show_chat_toast while open must drop; show_toast must fall back to non-chat (chat_id == None)
        s.show_chat_toast(CID_FABUSHI, format!("while-open-{label}"), 4000, cx);
        assert!(
            s.window_runtime.controller.toast_queue.is_empty(),
            "{label}: show_chat_toast while open must be dropped"
        );
        s.show_toast(format!("fallback-while-open-{label}"), 4000, cx);
        assert_eq!(
            s.window_runtime.controller.toast_queue.len(),
            1,
            "{label}: show_toast while open should enqueue as non-chat fallback"
        );
        assert_eq!(
            s.window_runtime.controller.toast_queue[0].chat_id, None,
            "{label}: show_toast fallback while open must NOT stamp chat_id"
        );

        close_fn(s, cx);
        assert!(
            !s.has_blocking_overlay(cx),
            "{label}: has_blocking_overlay must be false after close"
        );

        // Phase 2: chat-only toast -> open -> cleared immediately -> close -> must NOT resurrect
        s.window_runtime.controller.clear_all_toasts();
        s.show_chat_toast(CID_FABUSHI, format!("only-chat-{label}"), 4000, cx);
        open_fn(s, cx);
        assert!(
            s.window_runtime.controller.current_toast.is_none(),
            "{label}: chat-only toast must clear immediately on open"
        );
        close_fn(s, cx);
        assert!(
            s.is_chat_visible_for_toast(CID_FABUSHI, cx),
            "{label}: chat must be visible again after close"
        );
        assert!(
            s.window_runtime.controller.current_toast.is_none(),
            "{label}: purged chat toast must NOT resurrect after close"
        );
    };

    shell
        .update(&mut app.clone(), |s, cx| {
            // 1. TalkMode via NavDestination::TalkMode
            assert_overlay_gates_toast(
                s,
                "nav_talk_mode",
                &|s, cx| s.activate_nav_destination(NavDestination::TalkMode, cx),
                &|s, cx| s.hangup_active_call(cx),
                cx,
            );
            // 2. TalkMode via direct show_talk_mode
            assert_overlay_gates_toast(
                s,
                "show_talk_mode",
                &|s, cx| s.show_talk_mode(cx),
                &|s, cx| s.hide_talk_mode(cx),
                cx,
            );
            // 3. ComputerWorkbench via show_remote_computer
            assert_overlay_gates_toast(
                s,
                "show_remote_computer",
                &|s, cx| s.show_remote_computer(cx),
                &|s, cx| s.hide_remote_computer(cx),
                cx,
            );
            // 4. MediaViewer via show_media_viewer
            assert_overlay_gates_toast(
                s,
                "show_media_viewer",
                &|s, cx| s.show_media_viewer("https://example.com/a.png", false, cx),
                &|s, _| s.hide_media_viewer(),
                cx,
            );
            // 5. IvViewer via stage.pending_iv_url + handle_stage_pending_actions
            assert_overlay_gates_toast(
                s,
                "iv_viewer",
                &|s, cx| {
                    s.stage.update(cx, |st, _| {
                        st.pending_iv_url = Some((
                            "https://example.com/iv".to_string(),
                            "Instant View".to_string(),
                        ));
                    });
                    s.handle_stage_pending_actions(cx);
                },
                &|s, _| {
                    s.overlays.iv_viewer_overlay = None;
                },
                cx,
            );
            // 6. BotProfileBox via open_bot_profile_popup (pre-seeded in bot_profiles_cache)
            assert_overlay_gates_toast(
                s,
                "bot_profile_box",
                &|s, cx| s.open_bot_profile_popup("fabushi_bot", cx),
                &|s, _| {
                    s.miniapps.bot_profile_box = None;
                },
                cx,
            );
            // 7. pending_miniapp_confirm via request_launch_miniapp_with_confirm (unaccepted bot)
            assert_overlay_gates_toast(
                s,
                "pending_miniapp_confirm",
                &|s, cx| {
                    s.request_launch_miniapp_with_confirm(
                        "thirdparty_bot",
                        "wallet",
                        Some(CID_FABUSHI),
                        MiniAppLaunchMode::MenuButton,
                        None,
                        cx,
                    );
                },
                &|s, _| {
                    s.miniapps.pending_miniapp_confirm = None;
                },
                cx,
            );
            // 8. miniapp_tos_modal_open via InspectorAction::OpenMiniAppTos
            assert_overlay_gates_toast(
                s,
                "miniapp_tos_modal",
                &|s, cx| s.handle_inspector_action(InspectorAction::OpenMiniAppTos, cx),
                &|s, _| {
                    s.miniapps.miniapp_tos_modal_open = false;
                },
                cx,
            );
            // 9. Expanded MiniApp overlay + minimize to PiP (no resurrection) + chat toast while minimized + expand clears
            assert_overlay_gates_toast(
                s,
                "miniapp_overlay_expanded",
                &|s, cx| {
                    s.launch_miniapp(
                        "fabushi_bot",
                        "fabushi",
                        Some(CID_FABUSHI),
                        MiniAppLaunchMode::DirectLink,
                        None,
                        cx,
                    );
                },
                &|s, _| {
                    s.miniapps.bump_launch_generation();
                    s.miniapps.is_miniapp_minimized = false;
                    s.miniapps.miniapp_overlay = None;
                },
                cx,
            );

            // Specific Mini App minimize <-> expand lifecycle check
            s.window_runtime.controller.clear_all_toasts();
            s.show_chat_toast(CID_FABUSHI, "pre-miniapp-chat-toast", 4000, cx);
            s.launch_miniapp(
                "fabushi_bot",
                "fabushi",
                Some(CID_FABUSHI),
                MiniAppLaunchMode::DirectLink,
                None,
                cx,
            );
            assert!(s.has_blocking_overlay(cx));
            assert!(s.window_runtime.controller.current_toast.is_none());

            let ov = s.miniapps.miniapp_overlay.clone().expect("miniapp overlay");
            ov.update(cx, |w, _| {
                w.minimize();
                w.pending_actions.push_back(BotWebViewPendingAction::Minimize);
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(!ov.read(cx).is_open);
            assert!(
                !s.has_blocking_overlay(cx),
                "minimized Mini App PiP must NOT be a blocking overlay"
            );
            assert!(
                s.is_chat_visible_for_toast(CID_FABUSHI, cx),
                "chat must remain toast-visible behind minimized Mini App PiP"
            );
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "purged toast must not resurrect on minimize"
            );

            s.show_chat_toast(CID_FABUSHI, "while-minimized-pip", 4000, cx);
            assert_eq!(
                s.window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| t.text.as_str()),
                Some("while-minimized-pip")
            );
            ov.update(cx, |w, _| {
                w.pending_actions
                    .push_back(BotWebViewPendingAction::ExpandFromFloatingPip);
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(s.has_blocking_overlay(cx));
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "expanding Mini App from PiP must synchronously clear chat toast"
            );
            s.miniapps.bump_launch_generation();
            s.miniapps.is_miniapp_minimized = false;
            s.miniapps.miniapp_overlay = None;

            // Background !want_takeover WebMCP PiP launch must NOT purge active chat toast
            s.window_runtime.controller.clear_all_toasts();
            s.show_chat_toast(CID_FABUSHI, "active-chat-under-webmcp-pip", 4000, cx);
            s.launch_miniapp_for_webmcp(
                MiniAppWebMcpCommand {
                    request_id: "req_pip_1".to_string(),
                    bot_username: "fabushi_bot".to_string(),
                    short_name: "fabushi".to_string(),
                    chat_id: CID_FABUSHI as i64,
                    bot_user_id: UID_FABUSHI as i64,
                    app_title: "全球法布施".to_string(),
                    operation: "call_tool".to_string(),
                    tool_name: Some("get_dharma".to_string()),
                    arguments: serde_json::json!({}),
                    status_message: None,
                },
                cx,
            );
            assert!(!s.has_blocking_overlay(cx));
            assert!(s.is_chat_visible_for_toast(CID_FABUSHI, cx));
            assert_eq!(
                s.window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| t.text.as_str()),
                Some("active-chat-under-webmcp-pip")
            );
            s.miniapps.bump_launch_generation();
            s.miniapps.is_miniapp_minimized = false;
            s.miniapps.floating_pip_status = None;
            s.miniapps.miniapp_overlay = None;

            // 10. Non-creation modal_container dialogs via open_modal_dialog & handle_main_menu_command
            let modal_cases: Vec<(&str, ModalDialogKind)> = vec![
                ("modal_create_poll", ModalDialogKind::CreatePoll),
                ("modal_connection_proxy", ModalDialogKind::ConnectionProxy),
                (
                    "modal_export_data",
                    ModalDialogKind::ExportData {
                        chat_title: "Chat 101".to_string(),
                    },
                ),
                ("modal_shortcuts_legend", ModalDialogKind::ShortcutsLegend),
                ("modal_about_telegram", ModalDialogKind::AboutTelegram),
                (
                    "modal_clear_history",
                    ModalDialogKind::ClearChatHistory {
                        peer_name: "Fabushi".to_string(),
                    },
                ),
                (
                    "modal_leave_chat",
                    ModalDialogKind::LeaveOrDeleteChat {
                        peer_name: "Engineering Group".to_string(),
                        is_group: true,
                    },
                ),
                ("modal_passcode_setup", ModalDialogKind::PasscodeSetup),
            ];
            for (label, kind) in modal_cases {
                assert_overlay_gates_toast(
                    s,
                    label,
                    &move |s, cx| {
                        s.open_modal_dialog(label, kind.clone(), 420.0, 320.0, cx);
                    },
                    &|s, cx| {
                        s.modal_container.update(cx, |m, mcx| {
                            m.dismiss_top();
                            mcx.notify();
                        });
                    },
                    cx,
                );
            }
            assert_overlay_gates_toast(
                s,
                "main_menu_about",
                &|s, cx| s.handle_main_menu_command(MainMenuCommand::AboutTelegram, cx),
                &|s, cx| {
                    s.modal_container.update(cx, |m, mcx| {
                        m.dismiss_top();
                        mcx.notify();
                    });
                },
                cx,
            );

            // Non-blocking floating overlays must keep chat toast-visible
            s.window_runtime.controller.clear_all_toasts();
            s.sidebar.update(cx, |sb, _| sb.is_account_popover_open = true);
            s.bot_browser.first_creation_banner_open = true;
            assert!(!s.has_blocking_overlay(cx));
            assert!(s.is_chat_visible_for_toast(CID_FABUSHI, cx));
            s.show_chat_toast(CID_FABUSHI, "visible-under-nonblocking-popover", 4000, cx);
            assert_eq!(
                s.window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| t.text.as_str()),
                Some("visible-under-nonblocking-popover")
            );
            s.sidebar.update(cx, |sb, _| sb.is_account_popover_open = false);
            s.bot_browser.first_creation_banner_open = false;
        })
        .unwrap();

    // 11. Direct modal_container.show_modal(...) -> observe_drawer_and_modal_entities fires asynchronously and reconciles
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.show_chat_toast(CID_FABUSHI, "before-direct-show-modal", 4000, cx);
            s.modal_container.update(cx, |m, mcx| {
                let _ = m.show_modal("DirectModal", 400.0, 300.0, LayerStackOption::Normal);
                mcx.notify();
            });
        })
        .unwrap();

    assert!(
        pump_until(&app, 2000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.has_blocking_overlay(cx)
                        && !s.is_chat_visible_for_toast(CID_FABUSHI, cx)
                        && s.window_runtime.controller.current_toast.is_none()
                })
                .unwrap_or(false)
        })
        .await,
        "direct modal_container.show_modal must trigger observer reconciliation and clear chat toast"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            s.modal_container.update(cx, |m, mcx| {
                m.dismiss_top();
                mcx.notify();
            });
            assert!(s.is_chat_visible_for_toast(CID_FABUSHI, cx));
            assert!(s.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();
}

// [T5-58] Part B: SendSilent & ScheduleMessage mid-flight chat switch, switch-back, typing, rapid 2-send, + compose, user-switch, and logout draft isolation.
#[test]
fn t5_58b_send_silent_schedule_midflight_switch_user_and_logout_draft_isolation() {
    run_in_headless_subprocess(
        "t5_58b_send_silent_schedule_midflight_switch_user_and_logout_draft_isolation",
    );
}

async fn t5_58b_body(app: AsyncApp) {
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};

    const TOK_U1: &str = "tok_u1_draft_iso";
    const TOK_U2: &str = "tok_u2_draft_iso";

    let recorded_reqs: Arc<Mutex<Vec<(String, String, String, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let recorded_srv = Arc::clone(&recorded_reqs);
    let server_drafts: Arc<Mutex<HashMap<(String, i64), String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let server_drafts_srv = Arc::clone(&server_drafts);
    let send_barriers: Arc<Mutex<VecDeque<mpsc::Sender<(u16, String)>>>> =
        Arc::new(Mutex::new(VecDeque::new()));
    let send_barriers_srv = Arc::clone(&send_barriers);
    let seen_send_nonces: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let seen_send_nonces_srv = Arc::clone(&seen_send_nonces);
    let send_arrived = Arc::new(AtomicUsize::new(0));
    let send_arrived_srv = Arc::clone(&send_arrived);
    let next_msg_id = Arc::new(AtomicUsize::new(9801));
    let next_msg_id_srv = Arc::clone(&next_msg_id);

    fn sha1_digest(data: &[u8]) -> [u8; 20] {
        let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
        let bit_len = (data.len() as u64) * 8;
        let mut msg = data.to_vec();
        msg.push(0x80);
        while (msg.len() % 64) != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bit_len.to_be_bytes());
        for chunk in msg.chunks_exact(64) {
            let mut w = [0u32; 80];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([
                    chunk[4 * i],
                    chunk[4 * i + 1],
                    chunk[4 * i + 2],
                    chunk[4 * i + 3],
                ]);
            }
            for i in 16..80 {
                w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            }
            let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
            for (i, &wi) in w.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                    20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                    _ => (b ^ c ^ d, 0xCA62C1D6u32),
                };
                let temp = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(wi);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = temp;
            }
            h[0] = h[0].wrapping_add(a);
            h[1] = h[1].wrapping_add(b);
            h[2] = h[2].wrapping_add(c);
            h[3] = h[3].wrapping_add(d);
            h[4] = h[4].wrapping_add(e);
        }
        let mut out = [0u8; 20];
        for (i, val) in h.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&val.to_be_bytes());
        }
        out
    }

    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind api srv");
    let srv_url = format!("http://{}", api_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            let recorded_srv = Arc::clone(&recorded_srv);
            let server_drafts_srv = Arc::clone(&server_drafts_srv);
            let send_barriers_srv = Arc::clone(&send_barriers_srv);
            let seen_send_nonces_srv = Arc::clone(&seen_send_nonces_srv);
            let send_arrived_srv = Arc::clone(&send_arrived_srv);
            let next_msg_id_srv = Arc::clone(&next_msg_id_srv);
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let hend = pos + 4;
                            header_end = Some(hend);
                            let hdr_str = String::from_utf8_lossy(&raw[..hend]);
                            for line in hdr_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(hend) = header_end {
                        if raw.len() >= hend + content_len {
                            break;
                        }
                    }
                }
                let hend = header_end.unwrap_or(raw.len());
                let hdr_str = String::from_utf8_lossy(&raw[..hend]).to_string();
                let body_str = String::from_utf8_lossy(&raw[hend..raw.len().min(hend + content_len)])
                    .to_string();
                let first_line = hdr_str.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();

                if path.starts_with("/api/v1/ws") {
                    if let Some(ws_key) = hdr_str.lines().find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.trim()
                            .eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| v.trim().to_string())
                    }) {
                        use base64::Engine as _;
                        let digest =
                            sha1_digest(format!("{ws_key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes());
                        let accept = base64::engine::general_purpose::STANDARD.encode(digest);
                        let hs = format!(
                            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                        );
                        let _ = stream.write_all(hs.as_bytes());
                        let _ = stream.flush();
                        let mut tmp = [0u8; 256];
                        while let Ok(n) = stream.read(&mut tmp) {
                            if n == 0 {
                                break;
                            }
                        }
                        return;
                    }
                }
                let bearer_tok = hdr_str
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        if k.trim().eq_ignore_ascii_case("authorization") {
                            let val = v.trim();
                            val.strip_prefix("Bearer ")
                                .or_else(|| val.strip_prefix("bearer "))
                                .map(|t| t.trim().to_string())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();

                recorded_srv.lock().unwrap().push((
                    bearer_tok.clone(),
                    method.clone(),
                    path.clone(),
                    body_str.clone(),
                ));

                let (status_line, resp_body) = if method == "POST" && path == "/api/v1/messages/send" {
                    let parsed_req = serde_json::from_str::<serde_json::Value>(&body_str)
                        .unwrap_or(serde_json::Value::Null);
                    let nonce = parsed_req
                        .get("client_nonce")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let is_first_attempt = nonce == 0
                        || seen_send_nonces_srv.lock().unwrap().insert(nonce);
                    if !is_first_attempt {
                        (
                            "500 Internal Server Error".to_string(),
                            r#"{"error":"outbox background retry"}"#.to_string(),
                        )
                    } else {
                        let (tx, rx) = mpsc::channel::<(u16, String)>();
                        send_barriers_srv.lock().unwrap().push_back(tx);
                        send_arrived_srv.fetch_add(1, Ordering::SeqCst);
                        let (code, custom_body) = rx
                            .recv_timeout(Duration::from_secs(10))
                            .unwrap_or((500, r#"{"error":"barrier timeout"}"#.to_string()));
                        match code {
                            200 => {
                                let mid = next_msg_id_srv.fetch_add(1, Ordering::SeqCst) as i64;
                                let text_val = parsed_req
                                    .get("text")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let cid_val = parsed_req
                                    .get("chat_id")
                                    .and_then(|c| c.as_i64())
                                    .unwrap_or(101);
                                (
                                    "200 OK".to_string(),
                                    serde_json::json!({
                                        "message_id": mid,
                                        "id": mid,
                                        "pts": mid,
                                        "pts_count": 1,
                                        "duplicate": false,
                                        "date_ms": 1700000000000i64,
                                        "date": "2026-11-24T00:00:00Z",
                                        "sender_id": UID_VIEWER,
                                        "chat_id": cid_val,
                                        "text": text_val,
                                        "status": "sent"
                                    })
                                    .to_string(),
                                )
                            }
                            401 => ("401 Unauthorized".to_string(), custom_body),
                            _ => ("500 Internal Server Error".to_string(), custom_body),
                        }
                    }
                } else if path.starts_with("/api/v1/chats/") && path.ends_with("/draft") {
                    let cid: i64 = path
                        .trim_start_matches("/api/v1/chats/")
                        .trim_end_matches("/draft")
                        .parse()
                        .unwrap_or(0);
                    if method == "PUT" {
                        let text_val = serde_json::from_str::<serde_json::Value>(&body_str)
                            .ok()
                            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                            .unwrap_or_default();
                        let dto_json = serde_json::json!({
                            "chat_id": cid,
                            "text": text_val,
                            "entities": [],
                            "date_ms": 1700000000000i64
                        })
                        .to_string();
                        if text_val.trim().is_empty() {
                            server_drafts_srv.lock().unwrap().remove(&(bearer_tok, cid));
                        } else {
                            server_drafts_srv
                                .lock()
                                .unwrap()
                                .insert((bearer_tok, cid), text_val);
                        }
                        ("200 OK".to_string(), dto_json)
                    } else {
                        let body = match server_drafts_srv.lock().unwrap().get(&(bearer_tok, cid)).cloned() {
                            Some(t) => serde_json::json!({
                                "draft": {
                                    "chat_id": cid,
                                    "text": t,
                                    "entities": [],
                                    "date_ms": 1700000000000i64
                                }
                            })
                            .to_string(),
                            None => r#"{"draft":null}"#.to_string(),
                        };
                        ("200 OK".to_string(), body)
                    }
                } else if path == "/api/v1/chats" {
                    (
                        "200 OK".to_string(),
                        r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#.to_string(),
                    )
                } else if path == "/api/v1/user-vault/envelope" {
                    ("200 OK".to_string(), r#"{"envelope":null}"#.to_string())
                } else if path.contains("/messages") || path.contains("/history") || path.contains("/members") {
                    ("200 OK".to_string(), "[]".to_string())
                } else {
                    ("200 OK".to_string(), "{}".to_string())
                };

                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    let mut cfg = ClientConfig::default();
    cfg.server_url = srv_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let auth_u1 = AuthResponse {
        user_id: UID_VIEWER,
        username: "u1_draft".to_string(),
        session_id: "sess_u1".to_string(),
        token: TOK_U1.to_string(),
        first_name: Some("U1".to_string()),
        created_at: None,
        expires_at: None,
    };
    let auth_u2 = AuthResponse {
        user_id: UID_VIEWER + 1,
        username: "u2_draft".to_string(),
        session_id: "sess_u2".to_string(),
        token: TOK_U2.to_string(),
        first_name: Some("U2".to_string()),
        created_at: None,
        expires_at: None,
    };
    client.set_auth_session(auth_u1.clone());
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| {
            let s = setup_authenticated_shell(client, cx);
            s.update(cx, |sh, sh_cx| {
                sh.apply_authenticated_session(auth_u1.clone(), sh_cx);
            });
            s
        })
        .unwrap();

    // Ensure Chat 101 & 102 loaded and WS/network is online before starting Sub-case 1
    assert!(
        pump_until(&app, 4000, || {
            shell
                .read_with(&app, |s, cx| {
                    let sb = s.sidebar.read(cx);
                    s.client.network_listener.state().is_online()
                        && sb.dialogs.iter().any(|d| d.id == 101)
                        && sb.dialogs.iter().any(|d| d.id == 102)
                })
                .unwrap_or(false)
        })
        .await
    );

    let select_chat = |cid: u64, app: &AsyncApp| {
        shell
            .update(&mut app.clone(), |s, cx| {
                s.sidebar.update(cx, |sb, _| sb.select_dialog(cid));
                s.sync_selected_chat_inspector_and_stage(cid, cx);
            })
            .unwrap();
    };
    let save_chat101_draft = |text: &str, app: &AsyncApp| {
        let t = text.to_string();
        shell
            .update(&mut app.clone(), |s, cx| {
                s.window_runtime.controller.clear_all_toasts();
                s.composer.update(cx, |c, ccx| {
                    c.set_text(t);
                    ccx.notify();
                });
                s.flush_pending_draft_for_chat(101, cx);
            })
            .unwrap();
    };
    let pop_send_barrier = || -> mpsc::Sender<(u16, String)> {
        send_barriers
            .lock()
            .unwrap()
            .pop_front()
            .expect("expected barriered POST /api/v1/messages/send")
    };

    // Helper: set up Chat 101 with saved draft and wait for PUT /draft
    select_chat(101, &app);

    // === Sub-case 1a: 101 -> 102 mid-flight (SendSilent + HTTP 500) ===
    recorded_reqs.lock().unwrap().clear();
    let req_idx_1a = recorded_reqs.lock().unwrap().len();
    save_chat101_draft("chat101_silent_switch", &app);
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str)
                == Some("chat101_silent_switch")
                && recorded_reqs.lock().unwrap()[req_idx_1a..].iter().any(|(tok, m, p, b)| {
                    tok == TOK_U1
                        && m == "PUT"
                        && p == "/api/v1/chats/101/draft"
                        && b.contains("chat101_silent_switch")
                })
                && shell
                    .read_with(&app, |s, _| {
                        s.session_sync
                            .last_saved_draft_by_chat
                            .get(&101)
                            .map(String::as_str)
                            == Some("chat101_silent_switch")
                    })
                    .unwrap_or(false)
        })
        .await
    );
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("chat101_silent_switch".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await,
        "1a: POST must arrive at barrier"
    );
    // Switch to 102 while POST is mid-flight
    select_chat(102, &app);
    pump_until_settled(&app, 5, || recorded_reqs.lock().unwrap().len()).await;
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(_, m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "1a: switching 101->102 mid-flight must NOT emit empty PUT /draft"
    );
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
        Some("chat101_silent_switch")
    );
    // Release 500
    pop_send_barrier()
        .send((500, r#"{"error":"midflight 500"}"#.into()))
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| {
                    !s.session_sync.has_unmodified_in_flight_send_draft(101)
                })
                .unwrap_or(false)
        })
        .await
    );
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(
                s.client.local_store.get_draft(ChatId(101)).map(|d| d.text),
                Some("chat101_silent_switch".to_string())
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&101).map(String::as_str),
                Some("chat101_silent_switch")
            );
            assert_eq!(
                s.session_sync.last_draft_text_by_chat.get(&101).map(String::as_str),
                Some("chat101_silent_switch")
            );
            assert!(
                s.composer.read(cx).text().is_empty(),
                "1a: Chat 102 composer must stay clean"
            );
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "1a: Chat 101 error toast must NOT leak onto Chat 102"
            );
        })
        .unwrap();
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
        Some("chat101_silent_switch")
    );
    select_chat(101, &app);
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.composer.read(cx).text(), "chat101_silent_switch");
        })
        .unwrap();

    // === Sub-case 1b: 101 -> 102 mid-flight (ScheduleMessage + HTTP 401) ===
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_schedule_message = Some("chat101_silent_switch".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    select_chat(102, &app);
    pop_send_barrier()
        .send((401, r#"{"error":"unauthorized"}"#.into()))
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| {
                    !s.session_sync.has_unmodified_in_flight_send_draft(101)
                })
                .unwrap_or(false)
        })
        .await
    );
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(_, m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "1b: 401 mid-flight switch must NOT emit empty PUT /draft"
    );
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
        Some("chat101_silent_switch")
    );
    shell
        .read_with(&app, |s, _| {
            assert_eq!(
                s.client.local_store.get_draft(ChatId(101)).map(|d| d.text),
                Some("chat101_silent_switch".to_string())
            );
        })
        .unwrap();

    // === Sub-case 1c: 101 -> 102 mid-flight (ScheduleMessage + HTTP 200 OK) ===
    select_chat(101, &app);
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_schedule_message = Some("chat101_silent_switch".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    select_chat(102, &app);
    pump_until_settled(&app, 4, || recorded_reqs.lock().unwrap().len()).await;
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
        Some("chat101_silent_switch"),
        "1c: server draft must stay intact while POST is still in-flight"
    );
    pop_send_barrier().send((200, "{}".into())).unwrap();
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).is_none()
                && shell
                    .read_with(&app, |s, _| {
                        !s.session_sync.has_unmodified_in_flight_send_draft(101)
                            && s.client.local_store.get_draft(ChatId(101)).is_none()
                    })
                    .unwrap_or(false)
        })
        .await,
        "1c: 200 OK must clear server & local draft for 101"
    );
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&101).map(String::as_str).unwrap_or(""),
                ""
            );
            assert!(s.composer.read(cx).text().is_empty());
        })
        .unwrap();

    // === Sub-case 2a & 2b: 101 -> 102 -> 101 switch back without typing (500 then 200 OK) ===
    select_chat(101, &app);
    let req_idx_2a = recorded_reqs.lock().unwrap().len();
    save_chat101_draft("old_silent_2a", &app);
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str)
                == Some("old_silent_2a")
                && recorded_reqs.lock().unwrap()[req_idx_2a..].iter().any(|(tok, m, p, b)| {
                    tok == TOK_U1
                        && m == "PUT"
                        && p == "/api/v1/chats/101/draft"
                        && b.contains("old_silent_2a")
                })
                && shell
                    .read_with(&app, |s, _| {
                        s.session_sync
                            .last_saved_draft_by_chat
                            .get(&101)
                            .map(String::as_str)
                            == Some("old_silent_2a")
                    })
                    .unwrap_or(false)
        })
        .await
    );
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("old_silent_2a".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    select_chat(102, &app);
    select_chat(101, &app);
    // Pump so GET /api/v1/chats/101/draft completes while POST is still in-flight
    assert!(
        pump_until(&app, 3000, || {
            recorded_reqs
                .lock()
                .unwrap()
                .iter()
                .any(|(_, m, p, _)| m == "GET" && p == "/api/v1/chats/101/draft")
        })
        .await
    );
    pump_until_settled(&app, 4, || recorded_reqs.lock().unwrap().len()).await;
    shell
        .read_with(&app, |s, cx| {
            assert!(
                s.composer.read(cx).text().is_empty(),
                "2a: switching back mid-flight must NOT resurrect pre-send draft in composer"
            );
        })
        .unwrap();
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(_, m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "2a: must NOT emit empty PUT /draft mid-flight"
    );
    pop_send_barrier()
        .send((500, r#"{"error":"fail 2a"}"#.into()))
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.composer.read(cx).text() == "old_silent_2a"
                        && s.window_runtime.controller.current_toast.is_some()
                })
                .unwrap_or(false)
        })
        .await,
        "2a: 500 after switch-back must restore draft into Chat 101 composer"
    );

    // 2b: repeat with ScheduleMessage + 200 OK
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.composer.update(cx, |c, ccx| {
                c.pending_schedule_message = Some("old_silent_2a".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    select_chat(102, &app);
    select_chat(101, &app);
    assert!(
        pump_until(&app, 3000, || {
            recorded_reqs
                .lock()
                .unwrap()
                .iter()
                .any(|(_, m, p, _)| m == "GET" && p == "/api/v1/chats/101/draft")
        })
        .await
    );
    pump_until_settled(&app, 4, || recorded_reqs.lock().unwrap().len()).await;
    shell
        .read_with(&app, |s, cx| assert!(s.composer.read(cx).text().is_empty()))
        .unwrap();
    pop_send_barrier().send((200, "{}".into())).unwrap();
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).is_none()
                && shell
                    .read_with(&app, |s, cx| {
                        s.composer.read(cx).text().is_empty()
                            && s.client.local_store.get_draft(ChatId(101)).is_none()
                            && !s.session_sync.has_unmodified_in_flight_send_draft(101)
                    })
                    .unwrap_or(false)
        })
        .await,
        "2b: 200 OK after 101->102->101 must clear server & local draft"
    );

    // === Sub-case 2c: enter_compose_or_modal_isolation mid-flight ===
    let req_idx_2c = recorded_reqs.lock().unwrap().len();
    save_chat101_draft("compose_iso_draft", &app);
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str)
                == Some("compose_iso_draft")
                && recorded_reqs.lock().unwrap()[req_idx_2c..].iter().any(|(tok, m, p, b)| {
                    tok == TOK_U1
                        && m == "PUT"
                        && p == "/api/v1/chats/101/draft"
                        && b.contains("compose_iso_draft")
                })
                && shell
                    .read_with(&app, |s, _| {
                        s.session_sync
                            .last_saved_draft_by_chat
                            .get(&101)
                            .map(String::as_str)
                            == Some("compose_iso_draft")
                    })
                    .unwrap_or(false)
        })
        .await
    );
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("compose_iso_draft".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    shell
        .update(&mut app.clone(), |s, cx| {
            assert!(s.handle_shortcut_key("n", false, cx));
            assert!(s.composer.read(cx).is_compose_isolated);
        })
        .unwrap();
    pump_until_settled(&app, 4, || recorded_reqs.lock().unwrap().len()).await;
    assert!(
        !recorded_reqs.lock().unwrap().iter().any(|(_, m, p, b)| {
            m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
        }),
        "2c: enter_compose_or_modal_isolation mid-flight must NOT emit empty PUT /draft"
    );
    pop_send_barrier()
        .send((500, r#"{"error":"fail 2c"}"#.into()))
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| {
                    !s.session_sync.has_unmodified_in_flight_send_draft(101)
                        && s.client.local_store.get_draft(ChatId(101)).map(|d| d.text)
                            == Some("compose_iso_draft".to_string())
                })
                .unwrap_or(false)
        })
        .await
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            // First Escape closes compose recipient dropdown if open; second exits compose isolation
            assert!(s.handle_non_char_key_with_alt("escape", false, false, cx));
            if s.composer.read(cx).is_compose_isolated {
                assert!(s.handle_non_char_key_with_alt("escape", false, false, cx));
            }
            assert!(!s.composer.read(cx).is_compose_isolated);
            assert_eq!(s.composer.read(cx).text(), "compose_iso_draft");
        })
        .unwrap();
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
        Some("compose_iso_draft")
    );

    // === Sub-case 3a: 101 -> 102 -> 101 + type "new_text_500" mid-flight -> 500 ===
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.window_runtime.controller.clear_all_toasts();
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("compose_iso_draft".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    select_chat(102, &app);
    select_chat(101, &app);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.set_text("new_text_500");
                ccx.notify();
            });
        })
        .unwrap();
    pop_send_barrier()
        .send((500, r#"{"error":"fail 3a"}"#.into()))
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, _| s.window_runtime.controller.current_toast.is_some())
                .unwrap_or(false)
        })
        .await
    );
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.composer.read(cx).text(), "new_text_500");
            assert_eq!(
                s.client.local_store.get_draft(ChatId(101)).map(|d| d.text),
                Some("new_text_500".to_string())
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&101).map(String::as_str),
                Some("new_text_500")
            );
            assert_eq!(
                s.session_sync.last_draft_text_by_chat.get(&101).map(String::as_str),
                Some("new_text_500")
            );
        })
        .unwrap();

    // === Sub-case 3b: ScheduleMessage + type "new_text_200" mid-flight -> 200 OK ===
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_schedule_message = Some("new_text_500".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.set_text("new_text_200");
                ccx.notify();
            });
        })
        .unwrap();
    let req_idx_after_type = recorded_reqs.lock().unwrap().len();
    pop_send_barrier().send((200, "{}".into())).unwrap();
    pump_until_settled(&app, 5, || recorded_reqs.lock().unwrap().len()).await;
    assert!(
        !recorded_reqs.lock().unwrap()[req_idx_after_type..]
            .iter()
            .any(|(_, m, p, b)| {
                m == "PUT" && p == "/api/v1/chats/101/draft" && b.contains(r#""text":"""#)
            }),
        "3b: 200 OK must NOT emit empty PUT /draft when user typed new_text_200"
    );
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.composer.read(cx).text(), "new_text_200");
            assert_eq!(
                s.client.local_store.get_draft(ChatId(101)).map(|d| d.text),
                Some("new_text_200".to_string())
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&101).map(String::as_str),
                Some("new_text_200")
            );
        })
        .unwrap();

    // === Sub-case 3c: 2 rapid sends S1 ("first_rapid") -> type "second_rapid" -> S2 ("second_rapid") -> switch 101->102 ===
    let req_idx_3c = recorded_reqs.lock().unwrap().len();
    save_chat101_draft("first_rapid", &app);
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str)
                == Some("first_rapid")
                && recorded_reqs.lock().unwrap()[req_idx_3c..].iter().any(|(tok, m, p, b)| {
                    tok == TOK_U1
                        && m == "PUT"
                        && p == "/api/v1/chats/101/draft"
                        && b.contains("first_rapid")
                })
                && shell
                    .read_with(&app, |s, _| {
                        s.session_sync
                            .last_saved_draft_by_chat
                            .get(&101)
                            .map(String::as_str)
                            == Some("first_rapid")
                    })
                    .unwrap_or(false)
        })
        .await,
        "3c: first_rapid PUT /draft must complete before triggering S1"
    );
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("first_rapid".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    let s1_barrier = pop_send_barrier();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.set_text("second_rapid");
                ccx.notify();
            });
        })
        .unwrap();
    let base_arrived2 = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_schedule_message = Some("second_rapid".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived2).await);
    let s2_barrier = pop_send_barrier();
    select_chat(102, &app);

    // Release S1 with 500 -> must NOT revert local_store[101] to "first_rapid"
    s1_barrier
        .send((500, r#"{"error":"s1 fail"}"#.into()))
        .unwrap();
    pump_until_settled(&app, 5, || recorded_reqs.lock().unwrap().len()).await;
    shell
        .read_with(&app, |s, _| {
            assert_ne!(
                s.client.local_store.get_draft(ChatId(101)).map(|d| d.text).as_deref(),
                Some("first_rapid"),
                "3c: stale S1 failure must NOT overwrite newer S2 draft"
            );
            assert!(
                s.session_sync.has_unmodified_in_flight_send_draft(101),
                "3c: S2 in-flight send draft must remain protected after S1 settles"
            );
        })
        .unwrap();
    // Release S2 with 200 OK -> clears local & server draft cleanly
    s2_barrier.send((200, "{}".into())).unwrap();
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).is_none()
                && shell
                    .read_with(&app, |s, _| {
                        !s.session_sync.has_unmodified_in_flight_send_draft(101)
                            && s.client.local_store.get_draft(ChatId(101)).is_none()
                    })
                    .unwrap_or(false)
        })
        .await,
        "3c: S2 200 OK must clear local & server draft cleanly"
    );

    // === Sub-case 4: User switch apply_authenticated_session(U2) mid-flight (500 & 200 OK) ===
    server_drafts
        .lock()
        .unwrap()
        .insert((TOK_U2.into(), 101), "u2_private_draft".to_string());
    select_chat(101, &app);
    let req_idx_4 = recorded_reqs.lock().unwrap().len();
    save_chat101_draft("u1_secret_draft", &app);
    assert!(
        pump_until(&app, 3000, || {
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str)
                == Some("u1_secret_draft")
                && recorded_reqs.lock().unwrap()[req_idx_4..].iter().any(|(tok, m, p, b)| {
                    tok == TOK_U1
                        && m == "PUT"
                        && p == "/api/v1/chats/101/draft"
                        && b.contains("u1_secret_draft")
                })
                && shell
                    .read_with(&app, |s, _| {
                        s.session_sync
                            .last_saved_draft_by_chat
                            .get(&101)
                            .map(String::as_str)
                            == Some("u1_secret_draft")
                    })
                    .unwrap_or(false)
        })
        .await
    );
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_send_silent = Some("u1_secret_draft".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    let u1_barrier = pop_send_barrier();

    // Switch to U2 mid-flight
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_u2.clone(), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 4000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.client.network_listener.state().is_online()
                        && s.session_sync.user_id == Some(auth_u2.user_id)
                        && s.sidebar.read(cx).dialogs.iter().any(|d| d.id == 101)
                })
                .unwrap_or(false)
        })
        .await
    );
    select_chat(101, &app);
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| s.composer.read(cx).text() == "u2_private_draft")
                .unwrap_or(false)
        })
        .await,
        "4: U2 must load u2_private_draft"
    );

    // Release U1's POST with 500 -> zero effect on U2
    u1_barrier.send((500, r#"{"error":"u1 fail"}"#.into())).unwrap();
    pump_until_settled(&app, 5, || recorded_reqs.lock().unwrap().len()).await;
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.composer.read(cx).text(), "u2_private_draft");
            assert_eq!(
                s.client.local_store.get_draft(ChatId(101)).map(|d| d.text),
                Some("u2_private_draft".to_string())
            );
            assert_eq!(
                s.sidebar.read(cx).drafts.get(&101).map(String::as_str),
                Some("u2_private_draft")
            );
            assert!(s.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
        Some("u1_secret_draft")
    );
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U2.into(), 101)).map(String::as_str),
        Some("u2_private_draft")
    );

    // Repeat: U2 ScheduleMessage("u2_sched") -> switch to U1 -> U2 POST 200 OK -> zero tok_u2 PUT /draft
    recorded_reqs.lock().unwrap().clear();
    let base_arrived = send_arrived.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.pending_schedule_message = Some("u2_sched".into());
                c.clear();
                ccx.notify();
            });
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
    let u2_barrier = pop_send_barrier();
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_u1.clone(), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 4000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.client.network_listener.state().is_online()
                        && s.session_sync.user_id == Some(auth_u1.user_id)
                        && s.sidebar.read(cx).dialogs.iter().any(|d| d.id == 101)
                })
                .unwrap_or(false)
        })
        .await
    );
    select_chat(101, &app);
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| s.composer.read(cx).text() == "u1_secret_draft")
                .unwrap_or(false)
        })
        .await
    );
    let req_idx_u2 = recorded_reqs.lock().unwrap().len();
    u2_barrier.send((200, "{}".into())).unwrap();
    pump_until_settled(&app, 5, || recorded_reqs.lock().unwrap().len()).await;
    assert!(
        !recorded_reqs.lock().unwrap()[req_idx_u2..]
            .iter()
            .any(|(tok, m, p, _)| tok == TOK_U2 && m == "PUT" && p == "/api/v1/chats/101/draft"),
        "4: stale U2 200 OK after switching to U1 must NOT emit PUT /draft with tok_u2"
    );
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.composer.read(cx).text(), "u1_secret_draft");
            assert_eq!(
                s.client.local_store.get_draft(ChatId(101)).map(|d| d.text),
                Some("u1_secret_draft".to_string())
            );
        })
        .unwrap();
    assert_eq!(
        server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
        Some("u1_secret_draft")
    );

    // === Sub-case 5: logout() mid-flight (500 / 200 OK / 401) + clean re-login ===
    for (idx, (is_schedule, status_code)) in [(false, 500u16), (true, 200u16), (false, 401u16)]
        .into_iter()
        .enumerate()
    {
        if idx > 0 {
            shell
                .update(&mut app.clone(), |s, cx| {
                    s.apply_authenticated_session(auth_u1.clone(), cx);
                })
                .unwrap();
            assert!(
                pump_until(&app, 4000, || {
                    shell
                        .read_with(&app, |s, cx| {
                            s.client.network_listener.state().is_online()
                                && s.is_authenticated
                                && s.sidebar.read(cx).dialogs.iter().any(|d| d.id == 101)
                        })
                        .unwrap_or(false)
                })
                .await
            );
            select_chat(101, &app);
        }
        let draft_txt = format!("u1_before_logout_{idx}");
        let req_idx_draft = recorded_reqs.lock().unwrap().len();
        save_chat101_draft(&draft_txt, &app);
        assert!(
            pump_until(&app, 3000, || {
                server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str)
                    == Some(draft_txt.as_str())
                    && recorded_reqs.lock().unwrap()[req_idx_draft..].iter().any(
                        |(tok, m, p, b)| {
                            tok == TOK_U1
                                && m == "PUT"
                                && p == "/api/v1/chats/101/draft"
                                && b.contains(&draft_txt)
                        },
                    )
                    && shell
                        .read_with(&app, |s, _| {
                            s.session_sync
                                .last_saved_draft_by_chat
                                .get(&101)
                                .map(String::as_str)
                                == Some(draft_txt.as_str())
                        })
                        .unwrap_or(false)
            })
            .await,
            "5 (idx={idx}): PUT /draft for {draft_txt} must complete before triggering send"
        );
        let base_arrived = send_arrived.load(Ordering::SeqCst);
        shell
            .update(&mut app.clone(), |s, cx| {
                s.composer.update(cx, |c, ccx| {
                    if is_schedule {
                        c.pending_schedule_message = Some(draft_txt.clone());
                    } else {
                        c.pending_send_silent = Some(draft_txt.clone());
                    }
                    c.clear();
                    ccx.notify();
                });
            })
            .unwrap();
        assert!(pump_until(&app, 3000, || send_arrived.load(Ordering::SeqCst) > base_arrived).await);
        let logout_barrier = pop_send_barrier();

        shell.update(&mut app.clone(), |s, cx| s.logout(cx)).unwrap();
        let req_idx_logout = recorded_reqs.lock().unwrap().len();
        logout_barrier.send((status_code, "{}".into())).unwrap();
        pump_until_settled(&app, 5, || recorded_reqs.lock().unwrap().len()).await;

        assert!(
            !recorded_reqs.lock().unwrap()[req_idx_logout..]
                .iter()
                .any(|(_, m, p, _)| m == "PUT" && p == "/api/v1/chats/101/draft"),
            "5 (status={status_code}): post-logout POST settle must NOT emit PUT /draft"
        );
        shell
            .read_with(&app, |s, cx| {
                assert!(!s.is_authenticated);
                assert!(s.session_sync.in_flight_send_draft_gen_by_chat.is_empty());
                assert!(s.client.local_store.get_draft(ChatId(101)).is_none());
                assert!(s.composer.read(cx).text().is_empty());
                assert!(s.window_runtime.controller.current_toast.is_none());
            })
            .unwrap();
        assert_eq!(
            server_drafts.lock().unwrap().get(&(TOK_U1.into(), 101)).map(String::as_str),
            Some(draft_txt.as_str()),
            "5 (status={status_code}): logout mid-flight must leave server draft untouched"
        );
    }

    // Final recovery check after logout
    shell
        .update(&mut app.clone(), |s, cx| {
            s.apply_authenticated_session(auth_u1.clone(), cx);
        })
        .unwrap();
    assert!(
        pump_until(&app, 4000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.is_authenticated && s.sidebar.read(cx).dialogs.iter().any(|d| d.id == 101)
                })
                .unwrap_or(false)
        })
        .await
    );
    select_chat(101, &app);
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| s.composer.read(cx).text() == "u1_before_logout_2")
                .unwrap_or(false)
        })
        .await,
        "5: re-login after logout mid-flight must recover cleanly"
    );
}

// [T5-59] OpenTelegramLink / OpenCompanionChat / OpenBotProfile / OpenMiniAppTos & has_blocking_overlay Mini App sync.
#[test]
fn t5_59_open_telegram_link_and_companion_chat_miniapp_minimize_sync() {
    run_in_headless_subprocess("t5_59_open_telegram_link_and_companion_chat_miniapp_minimize_sync");
}

async fn t5_59_body(app: AsyncApp) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let profile_hits = Arc::new(AtomicUsize::new(0));
    let profile_hits_srv = profile_hits.clone();
    let api_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind api srv");
    let srv_url = format!("http://{}", api_listener.local_addr().unwrap());
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = api_listener.accept() {
            let profile_hits_srv = profile_hits_srv.clone();
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let hend = pos + 4;
                            header_end = Some(hend);
                            let hdr_str = String::from_utf8_lossy(&raw[..hend]);
                            for line in hdr_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(hend) = header_end {
                        if raw.len() >= hend + content_len {
                            break;
                        }
                    }
                }
                let hend = header_end.unwrap_or(raw.len());
                let hdr_str = String::from_utf8_lossy(&raw[..hend]).to_string();
                let first_line = hdr_str.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let _method = parts.next().unwrap_or("");
                let path = parts.next().unwrap_or("");

                let (status_line, resp_body) = if path == "/api/v1/chats" {
                    (
                        "200 OK".to_string(),
                        r#"[{"chat_id":101,"title":"全球法布施 · Fabushi","chat_type":4,"is_group":false,"is_bot":true,"updated_at":1700000005000},{"chat_id":102,"title":"Engineering Group","chat_type":1,"is_group":true,"is_bot":false,"updated_at":1700000004000}]"#.to_string(),
                    )
                } else if path == "/api/v1/user-vault/envelope" {
                    ("200 OK".to_string(), r#"{"envelope":null}"#.to_string())
                } else if path == "/api/v1/bots/profile/missing_500_bot" {
                    profile_hits_srv.fetch_add(1, Ordering::SeqCst);
                    (
                        "500 Internal Server Error".to_string(),
                        r#"{"error":"profile unavailable"}"#.to_string(),
                    )
                } else if path == "/api/v1/bots/profile/uncached_ok_bot"
                    || path == "/api/v1/bots/profile/cached_helper_bot"
                {
                    profile_hits_srv.fetch_add(1, Ordering::SeqCst);
                    let uname = if path.contains("uncached_ok_bot") {
                        "uncached_ok_bot"
                    } else {
                        "cached_helper_bot"
                    };
                    let uid = if uname == "uncached_ok_bot" { 9091i64 } else { 9092i64 };
                    let dto = BotPublicProfileDto {
                        bot: BotRegistryEntry {
                            bot_user_id: uid,
                            username: uname.to_string(),
                            display_name: format!("@{uname}"),
                            description: "Helper bot profile".to_string(),
                            about: "Helper bot about".to_string(),
                            allows_write_to_pm: true,
                            ..Default::default()
                        },
                        allows_write_to_pm: true,
                        app_tos_accepted: true,
                        ..Default::default()
                    };
                    (
                        "200 OK".to_string(),
                        serde_json::to_string(&dto).unwrap(),
                    )
                } else if path.contains("/miniapps/") && path.ends_with("/launch") {
                    let ctx = gpui_telegram_proto::miniapp_contracts::MiniAppLaunchContext {
                        app: MiniAppEntry {
                            app_id: "fabushi_main_app".to_string(),
                            bot_user_id: UID_FABUSHI as i64,
                            bot_username: "fabushi_bot".to_string(),
                            short_name: "fabushi".to_string(),
                            title: "全球法布施".to_string(),
                            description: "全球法布施小程序".to_string(),
                            category: "lifestyle".to_string(),
                            icon_emoji: "🪷".to_string(),
                            web_url: "/miniapps/fabushi_bot/fabushi".to_string(),
                            app_tos_accepted: true,
                            is_installed: true,
                            is_main_app: true,
                            direct_chat_id: Some(CID_FABUSHI as i64),
                            ..Default::default()
                        },
                        companion_bot: BotRegistryEntry {
                            bot_user_id: UID_FABUSHI as i64,
                            username: "fabushi_bot".to_string(),
                            display_name: "全球法布施".to_string(),
                            allows_write_to_pm: true,
                            has_main_app: true,
                            ..Default::default()
                        },
                        companion_chat_id: CID_FABUSHI as i64,
                        source_chat_id: Some(CID_FABUSHI as i64),
                        launch_mode: MiniAppLaunchMode::DirectLink,
                        ..Default::default()
                    };
                    (
                        "200 OK".to_string(),
                        serde_json::to_string(&ctx).unwrap(),
                    )
                } else if path.contains("/messages") || path.contains("/history") || path.contains("/members") {
                    ("200 OK".to_string(), "[]".to_string())
                } else {
                    ("200 OK".to_string(), "{}".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    let client = new_isolated_client(srv_url);
    let _rt = client.runtime().enter();
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    shell
        .update(&mut app.clone(), |s, cx| {
            s.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            s.miniapps.bot_profiles_cache.insert(
                "cached_helper_bot".to_string(),
                BotPublicProfileDto {
                    bot: BotRegistryEntry {
                        bot_user_id: 9092,
                        username: "cached_helper_bot".to_string(),
                        display_name: "Cached Helper".to_string(),
                        description: "Cached bot".to_string(),
                        allows_write_to_pm: true,
                        ..Default::default()
                    },
                    allows_write_to_pm: true,
                    app_tos_accepted: true,
                    ..Default::default()
                },
            );
        })
        .unwrap();

    let launch_fullscreen = |app: &AsyncApp| {
        shell
            .update(&mut app.clone(), |s, cx| {
                s.window_runtime.controller.clear_all_toasts();
                s.miniapps.bot_profile_box = None;
                s.miniapps.miniapp_tos_modal_open = false;
                s.launch_miniapp(
                    "fabushi_bot",
                    "fabushi",
                    Some(CID_FABUSHI),
                    MiniAppLaunchMode::DirectLink,
                    None,
                    cx,
                );
                let ov = s.miniapps.miniapp_overlay.clone().expect("miniapp overlay");
                assert!(ov.read(cx).is_open, "miniapp overlay must be open");
                assert!(!s.miniapps.is_miniapp_minimized);
                assert!(s.has_blocking_overlay(cx));
                ov
            })
            .unwrap()
    };

    // (c) Invariant: if ov.is_open is ever true, has_blocking_overlay(cx) MUST be true regardless of is_miniapp_minimized
    let ov = launch_fullscreen(&app);
    shell
        .update(&mut app.clone(), |s, cx| {
            s.miniapps.is_miniapp_minimized = true;
            assert!(ov.read(cx).is_open);
            assert!(
                s.has_blocking_overlay(cx),
                "has_blocking_overlay must match render_miniapp_overlays whenever ov.is_open == true"
            );
            assert!(
                !s.is_chat_visible_for_toast(CID_FABUSHI, cx),
                "chat must NOT be toast-visible while fullscreen miniapp ov.is_open == true"
            );
            s.miniapps.is_miniapp_minimized = false;
        })
        .unwrap();

    // (a1) OpenTelegramLink (non-miniapp, uncached & 500-failing /profile)
    let base_hits_a1 = profile_hits.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::OpenTelegramLink(
                    "https://t.me/missing_500_bot".to_string(),
                ));
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(
                !ov.read(cx).is_open,
                "OpenTelegramLink(500 bot) must minimize miniapp_overlay synchronously"
            );
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(!s.has_blocking_overlay(cx));
            assert!(s.is_chat_visible_for_toast(CID_FABUSHI, cx));
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            profile_hits.load(Ordering::SeqCst) > base_hits_a1
                && shell
                    .read_with(&app, |s, _| {
                        s.window_runtime
                            .controller
                            .current_toast
                            .as_ref()
                            .is_some_and(|t| t.text.contains("Failed to load bot info"))
                    })
                    .unwrap_or(false)
        })
        .await,
        "500 /profile must surface error toast on visible chat stage"
    );
    shell
        .read_with(&app, |s, cx| {
            assert!(!ov.read(cx).is_open);
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(s.miniapps.bot_profile_box.is_none());
            assert!(!s.has_blocking_overlay(cx));
            assert!(s.is_chat_visible_for_toast(CID_FABUSHI, cx));
        })
        .unwrap();

    // (a2) OpenTelegramLink (non-miniapp, cached bot profile with query string)
    let ov = launch_fullscreen(&app);
    let base_hits = profile_hits.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::OpenTelegramLink(
                    "https://t.me/cached_helper_bot?start=ref1".to_string(),
                ));
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(!ov.read(cx).is_open);
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(s.miniapps.bot_profile_box.as_ref().is_some_and(|b| b.read(cx).is_visible));
            assert!(s.has_blocking_overlay(cx));
            assert!(!s.is_chat_visible_for_toast(CID_FABUSHI, cx));
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || profile_hits.load(Ordering::SeqCst) > base_hits).await);
    pump_until_settled(&app, 3, || profile_hits.load(Ordering::SeqCst)).await;
    shell
        .update(&mut app.clone(), |s, cx| {
            s.miniapps.bot_profile_box = None;
            assert!(!ov.read(cx).is_open);
            assert!(!s.has_blocking_overlay(cx));
        })
        .unwrap();

    // (a3) OpenTelegramLink (non-miniapp, uncached 200-succeeding /profile via tg://resolve)
    let ov = launch_fullscreen(&app);
    shell
        .update(&mut app.clone(), |s, cx| {
            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::OpenTelegramLink(
                    "tg://resolve?domain=uncached_ok_bot".to_string(),
                ));
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(!ov.read(cx).is_open);
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(!s.has_blocking_overlay(cx));
        })
        .unwrap();
    assert!(
        pump_until(&app, 3000, || {
            shell
                .read_with(&app, |s, cx| {
                    s.miniapps
                        .bot_profile_box
                        .as_ref()
                        .is_some_and(|b| b.read(cx).is_visible)
                })
                .unwrap_or(false)
        })
        .await,
        "uncached 200 /profile must open BotProfileBox"
    );
    shell
        .update(&mut app.clone(), |s, cx| {
            assert!(!ov.read(cx).is_open);
            assert!(s.has_blocking_overlay(cx));
            assert!(!s.is_chat_visible_for_toast(CID_FABUSHI, cx));
            s.miniapps.bot_profile_box = None;
            assert!(!s.has_blocking_overlay(cx));
        })
        .unwrap();

    // (a4) OpenCompanionChat minimizes miniapp_overlay and opens companion chat with toast visibility
    let ov = launch_fullscreen(&app);
    shell
        .update(&mut app.clone(), |s, cx| {
            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::OpenCompanionChat {
                    companion_chat_id: 102,
                    bot_username: "companion_bot".to_string(),
                });
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(
                !ov.read(cx).is_open,
                "OpenCompanionChat must minimize miniapp_overlay synchronously"
            );
            assert!(s.miniapps.is_miniapp_minimized);
            assert_eq!(s.stage.read(cx).active_chat_id, Some(102));
            assert!(!s.has_blocking_overlay(cx));
            assert!(s.is_chat_visible_for_toast(102, cx));
            s.show_chat_toast(102, "companion-chat-toast", 3000, cx);
            assert_eq!(
                s.window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| t.text.as_str()),
                Some("companion-chat-toast")
            );
        })
        .unwrap();

    // (b) OpenBotProfile and OpenMiniAppTos keep ov.is_open == false & has_blocking_overlay == true while modal is open
    let ov = launch_fullscreen(&app);
    let base_hits_b = profile_hits.load(Ordering::SeqCst);
    shell
        .update(&mut app.clone(), |s, cx| {
            // Note: do NOT pre-call w.minimize() — verify drain_miniapp_overlay_actions syncs ov.is_open
            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::OpenBotProfile(
                    "cached_helper_bot".to_string(),
                ));
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(
                !ov.read(cx).is_open,
                "OpenBotProfile must keep ov.is_open == false so fullscreen miniapp-overlay-host never renders"
            );
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(s.miniapps.bot_profile_box.as_ref().is_some_and(|b| b.read(cx).is_visible));
            assert!(s.has_blocking_overlay(cx));
            assert!(!s.is_chat_visible_for_toast(102, cx));
            s.show_chat_toast(102, "blocked-by-bot-profile", 3000, cx);
            assert!(s.window_runtime.controller.current_toast.is_none());
        })
        .unwrap();
    assert!(pump_until(&app, 3000, || profile_hits.load(Ordering::SeqCst) > base_hits_b).await);
    pump_until_settled(&app, 3, || profile_hits.load(Ordering::SeqCst)).await;
    shell
        .update(&mut app.clone(), |s, cx| {
            s.miniapps.bot_profile_box = None;
            assert!(!ov.read(cx).is_open);
            assert!(!s.has_blocking_overlay(cx));
        })
        .unwrap();

    let ov = launch_fullscreen(&app);
    shell
        .update(&mut app.clone(), |s, cx| {
            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::OpenMiniAppTos);
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(
                !ov.read(cx).is_open,
                "OpenMiniAppTos must keep ov.is_open == false so fullscreen miniapp-overlay-host never renders"
            );
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(s.miniapps.miniapp_tos_modal_open);
            assert!(s.has_blocking_overlay(cx));
            assert!(!s.is_chat_visible_for_toast(102, cx));
            s.show_chat_toast(102, "blocked-by-miniapp-tos", 3000, cx);
            assert!(s.window_runtime.controller.current_toast.is_none());
            s.miniapps.miniapp_tos_modal_open = false;
            assert!(!ov.read(cx).is_open);
            assert!(!s.has_blocking_overlay(cx));
            assert!(s.is_chat_visible_for_toast(102, cx));
        })
        .unwrap();
}
