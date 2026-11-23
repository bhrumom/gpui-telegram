//! Phase 2 Tier 5 Adversarial Coverage Hardening Tests (`[T5-01]` .. `[T5-17]`).
//!
//! Uses isolated headless GPUI subprocess execution so each GPUI test owns an independent
//! `Application::headless()` instance and `GPUI_TELEGRAM_CONFIG_DIR`.

use futures::FutureExt;
use gpui::{App, AppContext, Application, AsyncApp, Entity};
use gpui_telegram_client::net::http_client::ChatSummary;
use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_desktop::components::bots::bot_attach_web_view::BotWebViewPendingAction;
use gpui_telegram_desktop::theme::{TdesktopThemePalette, TelegramAssetSource};
use gpui_telegram_desktop::views::app_shell::shell_truth::apply_authoritative_chat_list;
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
use gpui_telegram_desktop::views::stage::cards::{BotAssistCardAction, MessageItemModel};
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
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(test_fn_name);
    }

    let tmp_dir = tempfile::Builder::new()
        .prefix("gpui_tg_tier5_")
        .tempdir()
        .expect("create isolated tempdir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp_dir.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture")
        .output()
        .expect("spawn headless gpui child");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "tier5 child test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
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
        .timer(Duration::from_millis(140))
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
                                member_ids: Vec::new(),
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
                assert!(shell.handle_non_char_key("escape", false, cx));
                assert!(!shell.sidebar.read(cx).is_plus_menu_open);
                let comp = shell.composer.read(cx);
                assert!(!comp.is_compose_isolated);
                assert_eq!(
                    comp.text(),
                    draft_text,
                    "Esc cancel must restore un-flushed draft for entry path #{idx}"
                );
                assert_eq!(
                    comp.replying_to.as_ref().map(|r| r.msg_id),
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

    // 2) Switch back to Chat 101, enter + compose, then Cmd+, (Settings) synchronously exits compose & isolation
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
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
    let shell = app
        .update(|cx| setup_authenticated_shell(client, cx))
        .unwrap();

    // Part 1: persist_bot_computer_profile async completion after switching 101 -> 102 must NOT emit stale toast or mutate 102's inspector
    shell
        .update(&mut app.clone(), |shell, cx| {
            shell.sidebar.update(cx, |sb, _| sb.select_dialog(CID_FABUSHI));
            shell.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
            shell.inspector.update(cx, |insp, _| {
                insp.subpage = InspectorSubpage::BotComputers;
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
                insp.subpage = InspectorSubpage::Members;
            });
        })
        .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(140))
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
                InspectorSubpage::Members,
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
                        sender_name: "Fabushi".into(),
                        sender_avatar_initials: "FB".into(),
                        sender_avatar_color: 0x8F59F9,
                        timestamp: "10:00".into(),
                        content: "cached bot msg".into(),
                        is_outgoing: false,
                        ..Default::default()
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
