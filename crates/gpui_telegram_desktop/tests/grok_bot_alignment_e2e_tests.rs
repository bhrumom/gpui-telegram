//! Opaque-box 4-tier E2E test suite for Telegram `@GrokAI` & Bot/Mini App alignment,
//! Profile isolation, Copy/Window toast lifecycle, External MenuButton, `+` compose suppression,
//! non-overlapping `PROVIDER_ERROR`, Local Fabushi per-user hydration, and WebMCP bridge/guards.
//!
//! Covers Features 1–12 of `PROJECT.md` & `TEST_INFRA.md`:
//! - Tier 1: 60 feature coverage cases (`[T1-F01-01]` .. `[T1-F12-05]`)
//! - Tier 2: 60 boundary & corner cases (`[T2-F01-01]` .. `[T2-F12-05]`)
//! - Tier 3: 12 cross-feature pairwise cases (`[T3-01]` .. `[T3-12]`)
//! - Tier 4: 6 real-world application scenarios (`[T4-01]` .. `[T4-06]`)

use bot_browser::e2ee_sync::AiOauthVaultBundle;
use futures::FutureExt;
use gpui::{px, App, AppContext, Application, AsyncApp, Entity, IntoElement};
use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
use gpui_telegram_agent::hermes::providers::ProviderRouter;
use gpui_telegram_agent::hermes::toolsets::BoxFuture;
use gpui_telegram_agent::{
    HermesEngine, HermesEngineConfig, HermesEvent, HermesToolContext, HermesToolExecutor,
    HermesTurnRequest, PlatformImBridge, PlatformImCaller, ProviderConfig,
};
use gpui_telegram_client::net::hermes_client::{HermesInlineButtonDto, HermesRichChatDto};
use gpui_telegram_client::net::http_client::{ChatMemberItem, ChatSummary};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_core::types::{ChatId, MessageFlags, MessageId, PeerId, UserId};
use gpui_telegram_desktop::app::session::send_as_peers::SendAsPeer;
use gpui_telegram_desktop::components::bots::bot_attach_web_view::{
    peer_types, AttachBotEntry, BotAttachWebView, BotWebViewPendingAction,
};
#[cfg(target_os = "macos")]
use gpui_telegram_desktop::components::bots::macos_wkwebview::TELEGRAM_WEBVIEW_PROXY_BOOT_JS;
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette, TelegramAssetSource};
use gpui_telegram_desktop::views::app_shell::local_subscription_bridge::{
    has_usable_local_fabushi_provider, has_usable_local_fabushi_provider_for_user,
    hydrate_ai_oauth_vault_bundle_into_fabushi, hydrate_local_subscriptions_into_fabushi,
    hydrate_local_subscriptions_into_fabushi_for_user,
};
use gpui_telegram_desktop::views::app_shell::shell_stage_actions::build_mention_candidates;
use gpui_telegram_desktop::views::app_shell::shell_truth::{
    apply_authoritative_chat_list, decode_inline_keyboard_json, hermes_keyboard_to_stage,
    select_authoritative_dialog,
};
use gpui_telegram_desktop::views::boxes::peers::BotProfileBox;
use gpui_telegram_desktop::views::chat_sidebar::ComposeRecipientChip;
use gpui_telegram_desktop::views::composer::bot_keyboard::{
    flags as kb_flags, BotKeyboard, BotKeyboardButton, BotKeyboardButtonKind,
};
use gpui_telegram_desktop::views::composer::composer_ui::{
    render_text_input_strip_with_bot_menu, render_text_input_strip_with_options, ComposerBarAction,
};
use gpui_telegram_desktop::views::drawer::bot_computer_inspector::{
    format_bot_mau_subtitle, render_bot_profile_main,
};
use gpui_telegram_desktop::views::drawer::inspector_panels::{
    build_contact_profile_layout, render_contact_profile_panel,
};
use gpui_telegram_desktop::views::drawer::inspector_tabs::{
    InspectorAction, InspectorSubpage, InspectorTab, MemberItem,
};
use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
use gpui_telegram_desktop::views::shell::ActiveWorkspacePanel;
use gpui_telegram_desktop::views::stage::cards::{
    DeliveryStatus, InlineButtonType, MessageContent, MessageDirection, MessageItemModel,
    ReplyReference,
};
use gpui_telegram_desktop::views::stage::entities::parse_rich_blocks;
use gpui_telegram_desktop::views::stage::view::controls::HistoryViewBotMenuButtonViewControls;
use gpui_telegram_desktop::views::{
    AppShell, CanonicalComposer, ChatSidebar, ContextInspector, ConversationStage,
};
use gpui_telegram_desktop::window::WindowSubsystemRuntime;
use gpui_telegram_proto::contracts::auth::ContactDto;
use gpui_telegram_proto::message::{
    InlineKeyboardButton as ProtoInlineKeyboardButton, MessageContent as ProtoMessageContent,
    MessageItem as ProtoMessageItem,
};
use gpui_telegram_proto::miniapp_contracts::{
    BotCommandSpec, BotPublicProfileDto, BotRegistryEntry, FabushiBotSummaryDto,
    MiniAppControlMode, MiniAppEntry, MiniAppInitDataDto, MiniAppLaunchContext,
    MiniAppLaunchMode, MiniAppThemeParams, MiniAppUiSchema, MiniAppWebMcpCommand,
    BOTFATHER_USER_ID,
};
use gpui_telegram_proto::{UpdateEvent, WsClientFrame, WsServerFrame};
use magpie_gateway::{
    ClientSubscriptionSyncItem, ProtocolKind, SubscriptionAccount, SubscriptionKind,
    VertexAdcCredentials,
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CHILD_ENV: &str = "GPUI_GROK_ALIGNMENT_E2E_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_GROK_ALIGNMENT_E2E_TARGET";

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
        "tier1_f01_profile_state_isolation_on_chat_switch" => {
            Runner::Async(|app| Box::pin(t1_f01_body(app)))
        }
        "tier1_f02_group_vs_bot_vs_user_profile_branching" => {
            Runner::Async(|app| Box::pin(t1_f02_body(app)))
        }
        "tier1_f03_copy_feedback_auto_dismiss_and_switch_clear" => {
            Runner::Async(|app| Box::pin(t1_f03_body(app)))
        }
        "tier1_f04_stage_toast_banner_lifecycle_and_isolation" => {
            Runner::Async(|app| Box::pin(t1_f04_body(app)))
        }
        "tier1_f05_grok_bot_and_miniapp_spec_parity" => Runner::Sync(t1_f05_body),
        "tier1_f06_miniapp_menu_button_outside_input_box" => {
            Runner::Async(|app| Box::pin(t1_f06_body(app)))
        }
        "tier1_f07_plus_new_chat_composer_suppression" => {
            Runner::Async(|app| Box::pin(t1_f07_body(app)))
        }
        "tier1_f08_bot_conversation_start_bar_and_keyboard" => {
            Runner::Async(|app| Box::pin(t1_f08_body(app)))
        }
        "tier1_f09_window_toast_auto_dismiss_and_non_overlapping_banner" => {
            Runner::Async(|app| Box::pin(t1_f09_body(app)))
        }
        "tier1_f10_local_fabushi_per_user_provider_and_inline_error" => {
            Runner::Async(|app| Box::pin(t1_f10_body(app)))
        }
        "tier1_f11_webmcp_cross_frame_iframe_bridge_and_readiness" => {
            Runner::Async(|app| Box::pin(t1_f11_body(app)))
        }
        "tier1_f12_webmcp_call_tool_step_trace_and_failure_guard" => {
            Runner::Async(|app| Box::pin(t1_f12_body(app)))
        }
        "tier2_f01_profile_isolation_boundaries" => {
            Runner::Async(|app| Box::pin(t2_f01_body(app)))
        }
        "tier2_f02_profile_branching_boundaries" => Runner::Sync(t2_f02_body),
        "tier2_f03_copy_feedback_timer_and_fence_boundaries" => {
            Runner::Async(|app| Box::pin(t2_f03_body(app)))
        }
        "tier2_f04_stage_toast_isolation_boundaries" => Runner::Sync(t2_f04_body),
        "tier2_f05_grok_bot_edge_cases" => Runner::Sync(t2_f05_body),
        "tier2_f06_miniapp_menu_button_layout_boundaries" => {
            Runner::Async(|app| Box::pin(t2_f06_body(app)))
        }
        "tier2_f07_plus_compose_residual_state_boundaries" => {
            Runner::Async(|app| Box::pin(t2_f07_body(app)))
        }
        "tier2_f08_start_bar_and_bot_keyboard_flag_boundaries" => Runner::Sync(t2_f08_body),
        "tier2_f09_window_toast_wrapping_and_fence_boundaries" => {
            Runner::Async(|app| Box::pin(t2_f09_body(app)))
        }
        "tier2_f10_local_fabushi_provider_isolation_boundaries" => {
            Runner::Async(|app| Box::pin(t2_f10_body(app)))
        }
        "tier2_f11_webmcp_bridge_and_popup_boundaries" => Runner::Sync(t2_f11_body),
        "tier2_f12_webmcp_failure_guard_boundaries" => {
            Runner::Async(|app| Box::pin(t2_f12_body(app)))
        }
        "tier3_pairwise_miniapp_slash_typing_and_plus_compose" => {
            Runner::Async(|app| Box::pin(t3_pair_a_body(app)))
        }
        "tier3_pairwise_copy_toast_inspector_and_chat_switch" => {
            Runner::Async(|app| Box::pin(t3_pair_b_body(app)))
        }
        "tier3_pairwise_provider_error_toast_and_menu_hit_test" => {
            Runner::Async(|app| Box::pin(t3_pair_c_body(app)))
        }
        "tier3_pairwise_webmcp_miniapp_and_failure_guard" => {
            Runner::Async(|app| Box::pin(t3_pair_d_body(app)))
        }
        "tier4_scenario_01_fabushi_cold_start_menu_miniapp_webmcp" => {
            Runner::Async(|app| Box::pin(t4_s01_body(app)))
        }
        "tier4_scenario_02_fabushi_copy_switch_group_zero_leakage" => {
            Runner::Async(|app| Box::pin(t4_s02_body(app)))
        }
        "tier4_scenario_03_fabushi_miniapp_plus_compose_cancel_restore" => {
            Runner::Async(|app| Box::pin(t4_s03_body(app)))
        }
        "tier4_scenario_04_grok_provider_error_banner_cta_and_recovery" => {
            Runner::Async(|app| Box::pin(t4_s04_body(app)))
        }
        "tier4_scenario_05_multi_peer_rapid_hopping_under_active_toasts" => {
            Runner::Async(|app| Box::pin(t4_s05_body(app)))
        }
        "tier4_scenario_06_full_desktop_alignment_end_to_end_journey" => {
            Runner::Async(|app| Box::pin(t4_s06_body(app)))
        }
        other => {
            eprintln!("unknown headless gpui test target: {other}");
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
                            eprintln!("[headless child panic] {target_owned}: {msg}");
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
                                eprintln!("[headless async child panic] {t}: {msg}");
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
        eprintln!("[headless outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = macos_mod_init_headless_entry;

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
        .prefix("gpui_tg_grok_e2e_")
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
            "headless child test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn new_isolated_client(server_url: String) -> Arc<TelegramClient> {
    let mut cfg = ClientConfig::default();
    cfg.server_url = server_url;
    let client = Arc::new(TelegramClient::new(cfg));
    client.set_token(Some("tok_e2e_alignment".to_string()));
    client
}

const CID_FABUSHI: u64 = 101;
const CID_GROUP: u64 = 102;
const CID_ALICE: u64 = 103;
const CID_GROK: u64 = 104;
const CID_CHANNEL: u64 = 105;

const UID_VIEWER: u64 = 100_001;
const UID_FABUSHI: u64 = 700_101;
const UID_ALICE: u64 = 200_103;
const UID_GROK: u64 = 700_104;

#[derive(Clone, Default)]
struct MockPeerRecord {
    chat_id: u64,
    title: String,
    chat_type: u8,
    is_group: bool,
    is_channel: bool,
    is_bot: bool,
    peer_user_id: Option<u64>,
    peer_username: Option<String>,
    peer_phone: Option<String>,
    bio: Option<String>,
    members: Vec<serde_json::Value>,
    invite_link: Option<String>,
    menu_button_text: Option<String>,
    menu_button_url: Option<String>,
    delay_ms: u64,
}

#[derive(Clone, Default)]
struct MockServerData {
    peers: HashMap<u64, MockPeerRecord>,
    bot_profiles: HashMap<String, BotPublicProfileDto>,
    #[allow(dead_code)]
    requests: Vec<(String, String)>,
    miniapp_launch_fail: bool,
}

fn build_fabushi_bot_profile(chat_id: Option<i64>, has_started: bool) -> BotPublicProfileDto {
    let bot = BotRegistryEntry {
        bot_user_id: UID_FABUSHI as i64,
        owner_user_id: UID_VIEWER as i64,
        username: "fabushi_bot".to_string(),
        display_name: "全球法布施 · Fabushi".to_string(),
        description: "全球法布施与净心禅室 AI 助手，支持小程序联动交互。".to_string(),
        about: "全球法布施与净心禅室，7x24 小时智慧问答与布施助手。".to_string(),
        ai_persona: "Fabushi Dharma Guide".to_string(),
        menu_button_text: "全球法布施".to_string(),
        menu_button_url: "/miniapps/fabushi_bot/fabushi".to_string(),
        is_official: true,
        is_inline: true,
        allow_attach_menu: true,
        attach_peer_types: vec!["users".into(), "bots".into()],
        commands: vec![
            BotCommandSpec {
                command: "start".to_string(),
                description: "开启法布施对话".to_string(),
            },
            BotCommandSpec {
                command: "dharma".to_string(),
                description: "发送今日法布施".to_string(),
            },
        ],
        created_at: 1_700_000_000_000,
        is_fabushi_bot: true,
        menu_button_type: "web_app".to_string(),
        has_main_app: true,
        main_app_short_name: "fabushi".to_string(),
        privacy_policy_url: "https://fabushi.example.org/privacy".to_string(),
        has_settings: true,
        can_join_groups: true,
        mau_count: 8_888,
        allows_write_to_pm: true,
        app_tos_accepted: true,
        is_blocked_by_viewer: false,
        coding_agent: None,
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
        install_count: 1,
        is_installed: true,
        has_write_access: true,
        direct_chat_id: chat_id,
        created_by_nl: false,
        updated_at: 1_700_000_000_000,
        mau_count: 8_888,
        is_main_app: true,
        is_recent_used: true,
        last_launched_at: 1_700_000_001_000,
        privacy_policy_url: "https://fabushi.example.org/privacy".to_string(),
        app_tos_accepted: true,
        header_color: None,
        background_color: None,
    };
    BotPublicProfileDto {
        bot,
        main_app: Some(app.clone()),
        apps: vec![app],
        existing_chat_id: chat_id,
        has_started_chat: has_started,
        allows_write_to_pm: true,
        app_tos_accepted: true,
        is_blocked: false,
        is_muted: false,
    }
}

fn build_grok_bot_profile(chat_id: Option<i64>, has_started: bool) -> BotPublicProfileDto {
    let bot = BotRegistryEntry {
        bot_user_id: UID_GROK as i64,
        owner_user_id: 999_001,
        username: "GrokAI".to_string(),
        display_name: "Grok".to_string(),
        description: "Official xAI Grok assistant on Telegram.".to_string(),
        about: "Ask Grok anything directly or mention @GrokAI in groups.".to_string(),
        ai_persona: String::new(),
        menu_button_text: String::new(),
        menu_button_url: String::new(),
        is_official: true,
        is_inline: true,
        allow_attach_menu: false,
        attach_peer_types: vec!["users".into(), "groups".into()],
        commands: vec![
            BotCommandSpec {
                command: "start".to_string(),
                description: "Start chatting with Grok".to_string(),
            },
            BotCommandSpec {
                command: "help".to_string(),
                description: "Show usage tips".to_string(),
            },
            BotCommandSpec {
                command: "clear".to_string(),
                description: "Reset conversation".to_string(),
            },
            BotCommandSpec {
                command: "model".to_string(),
                description: "Switch Grok model".to_string(),
            },
        ],
        created_at: 1_700_000_000_000,
        is_fabushi_bot: false,
        menu_button_type: "commands".to_string(),
        has_main_app: false,
        main_app_short_name: String::new(),
        privacy_policy_url: "https://x.ai/legal/privacy-policy".to_string(),
        has_settings: false,
        can_join_groups: true,
        mau_count: 1_250_000,
        allows_write_to_pm: true,
        app_tos_accepted: true,
        is_blocked_by_viewer: false,
        coding_agent: None,
        ..Default::default()
    };
    BotPublicProfileDto {
        bot,
        main_app: None,
        apps: Vec::new(),
        existing_chat_id: chat_id,
        has_started_chat: has_started,
        allows_write_to_pm: true,
        app_tos_accepted: true,
        is_blocked: false,
        is_muted: false,
    }
}

fn default_mock_server_data() -> MockServerData {
    let mut peers = HashMap::new();
    peers.insert(
        CID_FABUSHI,
        MockPeerRecord {
            chat_id: CID_FABUSHI,
            title: "全球法布施 · Fabushi".to_string(),
            chat_type: 4,
            is_group: false,
            is_channel: false,
            is_bot: true,
            peer_user_id: Some(UID_FABUSHI),
            peer_username: Some("fabushi_bot".to_string()),
            peer_phone: None,
            bio: Some("全球法布施与净心禅室，7x24 小时智慧问答与布施助手。".to_string()),
            members: Vec::new(),
            invite_link: None,
            menu_button_text: Some("全球法布施".to_string()),
            menu_button_url: Some("/miniapps/fabushi_bot/fabushi".to_string()),
            delay_ms: 0,
        },
    );
    peers.insert(
        CID_GROUP,
        MockPeerRecord {
            chat_id: CID_GROUP,
            title: "AA 记账分账助手、团队投票助手".to_string(),
            chat_type: 1,
            is_group: true,
            is_channel: false,
            is_bot: false,
            peer_user_id: None,
            peer_username: None,
            peer_phone: None,
            bio: None, // IMPORTANT: empty description! Must NEVER inherit @fabushi_bot bio
            members: vec![
                json!({ "user_id": UID_VIEWER, "role": "creator" }),
                json!({ "user_id": UID_ALICE, "role": "member" }),
                json!({ "user_id": UID_FABUSHI, "role": "member" }),
            ],
            invite_link: Some("https://t.me/+GroupInviteAA".to_string()),
            menu_button_text: None,
            menu_button_url: None,
            delay_ms: 0,
        },
    );
    peers.insert(
        CID_ALICE,
        MockPeerRecord {
            chat_id: CID_ALICE,
            title: "Alice Lin".to_string(),
            chat_type: 0,
            is_group: false,
            is_channel: false,
            is_bot: false,
            peer_user_id: Some(UID_ALICE),
            peer_username: Some("alice".to_string()),
            peer_phone: Some("+1-555-0199".to_string()),
            bio: Some("Design & Rust Systems".to_string()),
            members: Vec::new(),
            invite_link: None,
            menu_button_text: None,
            menu_button_url: None,
            delay_ms: 0,
        },
    );
    peers.insert(
        CID_GROK,
        MockPeerRecord {
            chat_id: CID_GROK,
            title: "Grok".to_string(),
            chat_type: 4,
            is_group: false,
            is_channel: false,
            is_bot: true,
            peer_user_id: Some(UID_GROK),
            peer_username: Some("GrokAI".to_string()),
            peer_phone: None,
            bio: Some("Ask Grok anything directly or mention @GrokAI in groups.".to_string()),
            members: Vec::new(),
            invite_link: None,
            menu_button_text: None,
            menu_button_url: None,
            delay_ms: 0,
        },
    );
    peers.insert(
        CID_CHANNEL,
        MockPeerRecord {
            chat_id: CID_CHANNEL,
            title: "Release Announcements".to_string(),
            chat_type: 3,
            is_group: false,
            is_channel: true,
            is_bot: false,
            peer_user_id: None,
            peer_username: Some("gpui_releases".to_string()),
            peer_phone: None,
            bio: None,
            members: vec![json!({ "user_id": UID_VIEWER, "role": "creator" })],
            invite_link: None,
            menu_button_text: None,
            menu_button_url: None,
            delay_ms: 0,
        },
    );

    let mut bot_profiles = HashMap::new();
    bot_profiles.insert(
        "fabushi_bot".to_string(),
        build_fabushi_bot_profile(Some(CID_FABUSHI as i64), true),
    );
    bot_profiles.insert(
        "grokai".to_string(),
        build_grok_bot_profile(Some(CID_GROK as i64), true),
    );

    MockServerData {
        peers,
        bot_profiles,
        requests: Vec::new(),
        miniapp_launch_fail: false,
    }
}

fn spawn_alignment_mock_http_server(
    client: &Arc<TelegramClient>,
    initial: MockServerData,
) -> (String, Arc<Mutex<MockServerData>>) {
    let state = Arc::new(Mutex::new(initial));
    let state_srv = state.clone();
    let base_url = client.runtime().block_on(async move {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind alignment mock listener");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let st_arc = state_srv.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 32 * 1024];
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    let req_str = String::from_utf8_lossy(&buf[..n]);
                    let req_line = req_str.split("\r\n").next().unwrap_or("");
                    let mut parts = req_line.split_whitespace();
                    let method = parts.next().unwrap_or("GET").to_string();
                    let path = parts.next().unwrap_or("/").to_string();
                    let clean_path = path.split('?').next().unwrap_or(&path).to_string();

                    let mut delay_ms = 0u64;
                    let mut status_line = "200 OK";
                    let json_body = {
                        let mut st = st_arc.lock().unwrap();
                        st.requests.push((method.clone(), clean_path.clone()));

                        if clean_path.starts_with("/api/v1/miniapps/")
                            && clean_path.ends_with("/launch")
                        {
                            if st.miniapp_launch_fail {
                                status_line = "404 Not Found";
                                json!({ "error": "Mini App launch not found" }).to_string()
                            } else {
                                let prof = build_fabushi_bot_profile(Some(CID_FABUSHI as i64), true);
                                let launch_ctx = MiniAppLaunchContext {
                                    app: prof.main_app.clone().unwrap(),
                                    companion_bot: prof.bot.clone(),
                                    companion_chat_id: CID_FABUSHI as i64,
                                    source_chat_id: Some(CID_FABUSHI as i64),
                                    launch_mode: MiniAppLaunchMode::CompactSheet,
                                    start_param: None,
                                    init_data: MiniAppInitDataDto::default(),
                                    cloud_storage: Vec::new(),
                                    state_json: json!({}),
                                };
                                serde_json::to_string(&launch_ctx).unwrap()
                            }
                        } else if clean_path == "/api/v1/chats" {
                            let mut chats: Vec<serde_json::Value> = st
                                .peers
                                .values()
                                .map(|p| {
                                    json!({
                                        "chat_id": p.chat_id,
                                        "title": p.title,
                                        "chat_type": p.chat_type,
                                        "last_message": p.bio.clone().unwrap_or_else(|| "Active".to_string()),
                                        "unread_count": 0,
                                        "is_group": p.is_group || p.is_channel,
                                        "is_bot": p.is_bot,
                                        "avatar_color": 0x2AABEE,
                                        "updated_at": 1_700_000_000_000_i64
                                    })
                                })
                                .collect();
                            chats.sort_by_key(|v| v["chat_id"].as_u64().unwrap_or(0));
                            serde_json::to_string(&json!({ "chats": chats })).unwrap()
                        } else if let Some(target) =
                            clean_path.strip_prefix("/api/v1/bots/profile/")
                        {
                            let key = target.trim().trim_start_matches('@').to_ascii_lowercase();
                            if let Some(prof) = st.bot_profiles.get(&key) {
                                serde_json::to_string(prof).unwrap()
                            } else if let Ok(uid) = key.parse::<i64>() {
                                if let Some(prof) =
                                    st.bot_profiles.values().find(|p| p.bot.bot_user_id == uid)
                                {
                                    serde_json::to_string(prof).unwrap()
                                } else {
                                    json!({ "error": "not found" }).to_string()
                                }
                            } else {
                                json!({ "error": "not found" }).to_string()
                            }
                        } else if let Some(rest) = clean_path.strip_prefix("/api/v1/hermes/chats/")
                        {
                            if let Some(id_str) = rest.strip_suffix("/rich") {
                                let cid = id_str.parse::<u64>().unwrap_or(0);
                                if let Some(p) = st.peers.get(&cid) {
                                    delay_ms = p.delay_ms;
                                    let dto = HermesRichChatDto {
                                        chat_id: p.chat_id as i64,
                                        title: p.title.clone(),
                                        about: p.bio.clone(),
                                        username: p.peer_username.clone(),
                                        chat_type: p.chat_type,
                                        kind: Some(if p.is_channel {
                                            "channel".to_string()
                                        } else if p.is_group {
                                            "group".to_string()
                                        } else if p.is_bot {
                                            "bot".to_string()
                                        } else {
                                            "user".to_string()
                                        }),
                                        is_group: p.is_group,
                                        is_channel: p.is_channel,
                                        is_bot: p.is_bot,
                                        peer_user_id: p.peer_user_id,
                                        peer_username: p.peer_username.clone(),
                                        peer_display_name: Some(p.title.clone()),
                                        peer_phone: p.peer_phone.clone(),
                                        peer_bio: p.bio.clone(),
                                        members_count: p.members.len(),
                                        menu_button_text: p.menu_button_text.clone(),
                                        menu_button_url: p.menu_button_url.clone(),
                                        has_mini_app: p.menu_button_url.is_some(),
                                        ..Default::default()
                                    };
                                    serde_json::to_string(&dto).unwrap()
                                } else {
                                    json!({}).to_string()
                                }
                            } else {
                                json!({ "messages": [] }).to_string()
                            }
                        } else if let Some(rest) = clean_path.strip_prefix("/api/v1/chats/") {
                            let segs: Vec<&str> = rest.split('/').collect();
                            let cid = segs.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                            if segs.len() == 1 {
                                if let Some(p) = st.peers.get(&cid) {
                                    delay_ms = p.delay_ms;
                                    json!({
                                        "chat_id": p.chat_id,
                                        "title": p.title,
                                        "about": p.bio,
                                        "username": p.peer_username,
                                        "chat_type": p.chat_type,
                                        "kind": if p.is_channel { "channel" } else if p.is_group { "group" } else if p.is_bot { "bot" } else { "user" },
                                        "creator_id": UID_VIEWER,
                                        "is_forum": false,
                                        "is_group": p.is_group,
                                        "is_channel": p.is_channel,
                                        "is_bot": p.is_bot,
                                        "peer_user_id": p.peer_user_id,
                                        "peer_username": p.peer_username,
                                        "peer_display_name": p.title,
                                        "peer_phone": p.peer_phone,
                                        "peer_bio": p.bio,
                                        "photo_file_id": if p.is_bot { Some(format!("bot_photo_{cid}")) } else { None },
                                        "members_count": p.members.len(),
                                        "menu_button_text": p.menu_button_text,
                                        "menu_button_url": p.menu_button_url,
                                        "has_mini_app": p.menu_button_url.is_some(),
                                        "pts": 1
                                    })
                                    .to_string()
                                } else {
                                    json!({}).to_string()
                                }
                            } else if segs.get(1) == Some(&"members") {
                                let members = st
                                    .peers
                                    .get(&cid)
                                    .map(|p| p.members.clone())
                                    .unwrap_or_default();
                                json!({ "members": members }).to_string()
                            } else if segs.get(1) == Some(&"invites") {
                                let invites: Vec<serde_json::Value> = st
                                    .peers
                                    .get(&cid)
                                    .and_then(|p| p.invite_link.as_ref())
                                    .map(|lnk| {
                                        vec![json!({
                                            "invite_id": "inv_1",
                                            "chat_id": cid as i64,
                                            "creator_id": UID_VIEWER,
                                            "link": lnk,
                                            "code": "GroupInviteAA",
                                            "title": null,
                                            "usage_limit": null,
                                            "usage_count": 0,
                                            "expire_date_ms": null,
                                            "request_needed": false,
                                            "is_revoked": false,
                                            "created_at_ms": 1_700_000_000_000_i64
                                        })]
                                    })
                                    .unwrap_or_default();
                                json!({ "invites": invites }).to_string()
                            } else if segs.get(1) == Some(&"messages") && method == "POST" {
                                json!({ "message_id": 9001, "pts": 2 }).to_string()
                            } else if segs.get(1) == Some(&"messages") {
                                json!({ "messages": [] }).to_string()
                            } else if segs.get(1) == Some(&"pins") {
                                json!({ "pins": [] }).to_string()
                            } else if segs.get(1) == Some(&"send-as") {
                                json!({ "peers": [] }).to_string()
                            } else {
                                json!({ "ok": true }).to_string()
                            }
                        } else if clean_path == "/api/v1/contacts/add" {
                            json!({
                                "user_id": UID_GROK,
                                "chat_id": CID_GROK,
                                "title": "Grok",
                                "username": "GrokAI",
                                "is_bot": true
                            })
                            .to_string()
                        } else {
                            json!({ "ok": true, "apps": [], "bots": [], "chats": [] }).to_string()
                        }
                    };

                    if delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }

                    let resp = format!(
                        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json_body.len(),
                        json_body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{}", addr)
    });
    (base_url, state)
}

fn setup_authenticated_shell(
    client: Arc<TelegramClient>,
    cx: &mut App,
) -> Entity<AppShell> {
    let fabushi_prof = build_fabushi_bot_profile(Some(CID_FABUSHI as i64), true);
    let grok_prof = build_grok_bot_profile(Some(CID_GROK as i64), true);

    cx.new(|cx| {
        let mut s = AppShell::new_with_client(client, cx);
        s.is_authenticated = true;
        s.session_sync.user_id = Some(UID_VIEWER);
        s.session_sync.session_token = Some("tok_e2e_alignment".to_string());
        s.miniapps
            .bot_profiles_cache
            .insert("fabushi_bot".to_string(), fabushi_prof.clone());
        s.miniapps
            .bot_profiles_cache
            .insert("grokai".to_string(), grok_prof.clone());
        s.miniapps.attach_menu_bots = vec![fabushi_prof.bot.clone(), grok_prof.bot.clone()];
        s.miniapps.installed_miniapps = fabushi_prof.apps.clone();
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

        s.session_sync.chat_peer_user_ids.insert(CID_FABUSHI, UID_FABUSHI);
        s.session_sync.chat_peer_user_ids.insert(CID_ALICE, UID_ALICE);
        s.session_sync.chat_peer_user_ids.insert(CID_GROK, UID_GROK);

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
                chat_id: CID_GROUP,
                title: "AA 记账分账助手、团队投票助手".to_string(),
                chat_type: 1,
                last_message: Some("Group created".to_string()),
                unread_count: 0,
                is_group: Some(true),
                is_bot: Some(false),
                avatar_color: Some(0x3A7AFE),
                updated_at: Some(1_700_000_004_000),
            },
            ChatSummary {
                chat_id: CID_ALICE,
                title: "Alice Lin".to_string(),
                chat_type: 0,
                last_message: Some("Hey!".to_string()),
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(false),
                avatar_color: Some(0x0A84FF),
                updated_at: Some(1_700_000_003_000),
            },
            ChatSummary {
                chat_id: CID_GROK,
                title: "Grok".to_string(),
                chat_type: 4,
                last_message: Some("Ask Grok".to_string()),
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: Some(0x111111),
                updated_at: Some(1_700_000_002_000),
            },
            ChatSummary {
                chat_id: CID_CHANNEL,
                title: "Release Announcements".to_string(),
                chat_type: 3,
                last_message: Some("v1.6.7".to_string()),
                unread_count: 0,
                is_group: Some(true),
                is_bot: Some(false),
                avatar_color: Some(0x34C759),
                updated_at: Some(1_700_000_001_000),
            },
        ];

        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    apply_authoritative_chat_list(sb, st, insp, &chats);
                });
            });
        });

        s
    })
}

async fn pump_until(
    app: &AsyncApp,
    timeout_ms: u64,
    mut cond: impl FnMut(&mut AsyncApp) -> bool,
) -> bool {
    let mut elapsed = 0u64;
    let step = 20u64;
    let mut app_mut = app.clone();
    loop {
        if cond(&mut app_mut) {
            return true;
        }
        if elapsed >= timeout_ms {
            return false;
        }
        app.background_executor()
            .timer(Duration::from_millis(step))
            .await;
        elapsed += step;
    }
}

fn sample_vertex_item() -> ClientSubscriptionSyncItem {
    let mut headers = BTreeMap::new();
    headers.insert("x-goog-user-project".to_string(), "proj-e2e-123".to_string());
    ClientSubscriptionSyncItem {
        account: SubscriptionAccount {
            id: "google_vertex:dev@e2e.test".to_string(),
            kind: SubscriptionKind::GoogleVertex,
            label: "Google Vertex AI (dev@e2e.test · proj-e2e-123)".to_string(),
            email: Some("dev@e2e.test".to_string()),
            account_id: Some("proj-e2e-123".to_string()),
            plan: Some("vertex_adc".to_string()),
            active: true,
            base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
            protocol: ProtocolKind::GeminiGenerateContent,
            default_model: Some("gemini-2.5-flash".to_string()),
            source_path: std::env::temp_dir()
                .join("nonexistent_adc_e2e.json")
                .to_string_lossy()
                .into_owned(),
            expires_at_ms: None,
            access_token: None,
            headers: headers.clone(),
        },
        access_token: None,
        raw_headers: headers,
        vertex_adc: Some(VertexAdcCredentials {
            client_id: "cid-e2e".to_string(),
            client_secret: "csec-e2e".to_string(),
            refresh_token: "rt-e2e".to_string(),
            quota_project_id: Some("proj-e2e-123".to_string()),
            account: Some("dev@e2e.test".to_string()),
        }),
        codex_oauth: None,
        region_id: Some("global".to_string()),
        available_models: Vec::new(),
    }
}

#[cfg(target_os = "macos")]
struct RestoreEnv {
    key: &'static str,
    prev: Option<String>,
}

#[cfg(target_os = "macos")]
impl RestoreEnv {
    fn temporarily_unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prev }
    }
}

#[cfg(target_os = "macos")]
impl Drop for RestoreEnv {
    fn drop(&mut self) {
        if let Some(ref val) = self.prev {
            unsafe {
                std::env::set_var(self.key, val);
            }
        } else {
            unsafe {
                std::env::remove_var(self.key);
            }
        }
    }
}

struct MockWebMcpImBridge {
    calls: Arc<Mutex<Vec<(String, serde_json::Value, Option<i64>)>>>,
}

impl PlatformImBridge for MockWebMcpImBridge {
    fn execute_im_tool<'a>(
        &'a self,
        tool_name: &'a str,
        args: &'a serde_json::Value,
        caller: PlatformImCaller,
    ) -> BoxFuture<'a, std::result::Result<serde_json::Value, String>> {
        let calls = self.calls.clone();
        let tn = tool_name.to_string();
        let args_cl = args.clone();
        Box::pin(async move {
            calls
                .lock()
                .unwrap()
                .push((tn.clone(), args_cl.clone(), caller.chat_id));
            if tn == "webmcp_call_tool"
                && args_cl.get("tool_name").and_then(|v| v.as_str()) == Some("fabushi_send_dharma")
            {
                return Err(
                    "未注册的 WebMCP 工具: fabushi_send_dharma | 请先在桌面端打开或刷新「全球法布施」小程序窗口后重试"
                        .to_string(),
                );
            }
            if tn == "webmcp_discover" {
                return Ok(json!({ "success": true, "tools": ["get_state", "page_inspect"] }));
            }
            Ok(json!({ "ok": true }))
        })
    }
}

async fn read_full_http_body(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let mut header_end: Option<usize> = None;
    let mut content_len: usize = 0;

    loop {
        let Ok(n) = stream.read(&mut buf).await else {
            break;
        };
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if header_end.is_none() {
            if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = Some(pos + 4);
                let headers_str = String::from_utf8_lossy(&raw[..pos]);
                for line in headers_str.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse::<usize>().unwrap_or(0);
                        }
                    }
                }
            }
        }
        if let Some(hend) = header_end {
            if raw.len() >= hend + content_len {
                return raw[hend..hend + content_len].to_vec();
            }
        }
    }
    if let Some(hend) = header_end {
        raw[hend..].to_vec()
    } else {
        Vec::new()
    }
}

async fn spawn_mock_openai_webmcp_server(
) -> (String, Arc<Mutex<bool>>, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock openai listener");
    let addr = listener.local_addr().expect("openai local_addr");
    let saw_tool_error = Arc::new(Mutex::new(false));
    let turn_calls = Arc::new(Mutex::new(0usize));
    let saw_err_srv = saw_tool_error.clone();
    let calls_srv = turn_calls.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let body_bytes = read_full_http_body(&mut stream).await;
            let body_json: serde_json::Value =
                serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({}));
            let step = {
                let mut g = calls_srv.lock().unwrap();
                *g += 1;
                *g
            };

            let resp_json = if step == 1 {
                json!({
                    "id": "chatcmpl-f12-1",
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call_webmcp_1",
                                "type": "function",
                                "function": {
                                    "name": "webmcp_call_tool",
                                    "arguments": "{\"bot_username\":\"fabushi_bot\",\"chat_id\":101,\"tool_name\":\"fabushi_send_dharma\",\"arguments\":{\"sutra\":\"金刚经\"}}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": { "prompt_tokens": 20, "completion_tokens": 12, "total_tokens": 32 }
                })
            } else {
                let has_err = body_json
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .is_some_and(|msgs| {
                        msgs.iter().any(|m| {
                            m.get("role").and_then(|r| r.as_str()) == Some("tool")
                                && m.get("content")
                                    .and_then(|c| c.as_str())
                                    .is_some_and(|s| s.contains("未注册的 WebMCP 工具: fabushi_send_dharma"))
                        })
                    });
                if has_err {
                    *saw_err_srv.lock().unwrap() = true;
                }
                json!({
                    "id": "chatcmpl-f12-2",
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "操作失败：未注册工具 fabushi_send_dharma，请先打开「全球法布施」小程序。"
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 30, "completion_tokens": 18, "total_tokens": 48 }
                })
            };

            let body_str = resp_json.to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body_str.len(),
                body_str
            );
            let _ = stream.write_all(resp.as_bytes()).await;
        }
    });

    (format!("http://{addr}/v1"), saw_tool_error, turn_calls)
}

fn make_proto_text_msg(
    msg_id: i64,
    chat_id: u64,
    sender_id: u64,
    text: &str,
    inline_keyboard: Option<Vec<Vec<ProtoInlineKeyboardButton>>>,
) -> ProtoMessageItem {
    let mut item = ProtoMessageItem::new(
        MessageId(msg_id),
        ChatId(chat_id as i64),
        UserId(sender_id),
        PeerId::Chat(ChatId(chat_id as i64)),
        ProtoMessageContent::Text {
            text: text.to_string(),
            entities: Vec::new(),
        },
    )
    .with_flags(MessageFlags::default());
    item.inline_keyboard = inline_keyboard;
    item
}

// ============================================================================
// TIER 1: FEATURE COVERAGE (60 cases across Features 1–12)
// ============================================================================

#[test]
fn tier1_f01_profile_state_isolation_on_chat_switch() {
    run_in_headless_subprocess("tier1_f01_profile_state_isolation_on_chat_switch");
}

async fn t1_f01_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();

    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T1-F01-01] Bot (@fabushi_bot) -> Group (empty bio): zero bot bio/command/miniapp leakage
    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| {
            insp.open();
            insp.set_tab(InspectorTab::SharedMedia);
            insp.subpage = InspectorSubpage::BotConfigEditor;
        });
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let ok_bot = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                let insp = s.inspector.read(cx);
                insp.profile.is_bot
                    && insp.profile.bio.contains("全球法布施")
                    && insp.profile.miniapp_url.is_some()
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_bot, "[T1-F01-01] @fabushi_bot profile hydration failed");

    // Switch to Group (empty about)
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
    });
    // Synchronous check before HTTP resolves
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(
            insp.profile.bio.is_empty(),
            "[T1-F01-01-sync] stale bio leaked immediately on switch to group: {:?}",
            insp.profile.bio
        );
        assert!(!insp.profile.is_bot, "[T1-F01-01-sync] stale is_bot leaked");
        assert!(
            insp.profile.bot_commands.is_empty(),
            "[T1-F01-01-sync] stale bot_commands leaked"
        );
        assert!(
            insp.profile.miniapp_url.is_none(),
            "[T1-F01-01-sync] stale miniapp_url leaked"
        );
        assert!(
            insp.active_fabushi_bot.is_none(),
            "[T1-F01-01-sync] stale active_fabushi_bot leaked"
        );
        // [T1-F01-05] Inspector subpage & active_tab reset on chat switch
        assert_eq!(
            insp.subpage,
            InspectorSubpage::Main,
            "[T1-F01-05] inspector subpage did not reset on chat switch"
        );
        assert_eq!(
            insp.active_tab,
            InspectorTab::Profile,
            "[T1-F01-05] inspector active_tab did not reset to Profile on chat switch"
        );
    });

    let ok_grp = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                let insp = s.inspector.read(cx);
                insp.members.is_group_or_channel && insp.members.members.len() == 3
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_grp, "[T1-F01-01] group members hydration timed out");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(
            insp.profile.bio.is_empty(),
            "[T1-F01-01-async] bot bio leaked after group hydration: {:?}",
            insp.profile.bio
        );
        assert!(!insp.profile.is_bot);
        assert!(insp.profile.username.is_empty());
        assert!(insp.profile.avatar_path.is_none());
    });

    // [T1-F01-02] Bot -> 1v1 User (@alice)
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.is_bot)
            .unwrap_or(false)
    })
    .await;
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
    });
    let ok_alice = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                let insp = s.inspector.read(cx);
                insp.profile.username.contains("alice") && !insp.profile.phone.is_empty()
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_alice, "[T1-F01-02] @alice hydration failed");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(!insp.profile.is_bot);
        assert_eq!(insp.profile.bio, "Design & Rust Systems");
        assert!(insp.profile.bot_commands.is_empty());
        assert!(insp.profile.avatar_path.is_none());
    });

    // [T1-F01-03] 1v1 User (has phone) -> Bot (@GrokAI, no phone)
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROK, cx);
    });
    let ok_grok = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                let insp = s.inspector.read(cx);
                insp.profile.is_bot && insp.profile.username.to_ascii_lowercase().contains("grokai")
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_grok, "[T1-F01-03] @GrokAI hydration failed");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(
            insp.profile.phone.is_empty(),
            "[T1-F01-03] stale 1v1 phone leaked into Bot profile: {:?}",
            insp.profile.phone
        );
        assert!(insp.profile.bio.contains("Ask Grok"));
    });

    // [T1-F01-04] Bot A (@fabushi_bot) -> Bot B (@GrokAI) isolation
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).active_fabushi_bot.is_some())
            .unwrap_or(false)
    })
    .await;
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROK, cx);
    });
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| {
                s.inspector
                    .read(cx)
                    .profile
                    .username
                    .to_ascii_lowercase()
                    .contains("grokai")
            })
            .unwrap_or(false)
    })
    .await;
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(
            insp.active_fabushi_bot.is_none(),
            "[T1-F01-04] Bot A active_fabushi_bot leaked to Bot B"
        );
        assert!(
            insp.profile.miniapp_url.is_none(),
            "[T1-F01-04] Bot A miniapp_url leaked to Bot B"
        );
        assert!(
            !insp.profile.bot_commands.iter().any(|(c, _)| c == "/dharma"),
            "[T1-F01-04] Bot A /dharma command leaked to Bot B"
        );
    });
}

#[test]
fn tier1_f02_group_vs_bot_vs_user_profile_branching() {
    run_in_headless_subprocess("tier1_f02_group_vs_bot_vs_user_profile_branching");
}

async fn t1_f02_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();

    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T1-F02-01] Group chat branching
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
    });
    let ok_grp = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                let insp = s.inspector.read(cx);
                insp.members.is_group_or_channel && insp.members.members.len() == 3
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_grp, "[T1-F02-01] group members state not set");
    let insp_ent = shell.read_with(&mut cx, |s, _| s.inspector.clone()).unwrap();
    let group_layout = insp_ent
        .update(&mut cx, |insp, icx| {
            insp.set_language("zh-CN");
            let _group_el = render_contact_profile_panel(
                &insp.profile,
                &insp.members,
                &insp.shared_media,
                &insp.collapsed_sections,
                insp.is_secret_chat_active,
                insp.is_contact_blocked,
                insp.copied_feedback.as_ref(),
                TdesktopThemePalette::default(),
                "zh-CN",
                icx,
            );
            build_contact_profile_layout(
                &insp.profile,
                &insp.members,
                TdesktopThemePalette::default(),
                "zh-CN",
            )
        })
        .unwrap();
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        let st = s.stage.read(cx);
        assert!(st.is_group_chat, "[T1-F02-01] stage.is_group_chat must be true");
        assert!(!insp.profile.is_bot);
        assert_eq!(insp.members.members.len(), 3);
        assert!(!insp.members.invite_link.is_empty());
    });
    assert!(group_layout.is_group_or_channel, "[T1-F02-04] group layout must set is_group_or_channel");
    assert!(!group_layout.is_channel);
    assert_eq!(group_layout.info_section_title, "群组信息");
    assert_eq!(group_layout.privacy_section_title, "通知与管理");
    assert!(!group_layout.show_secret_chat, "[T1-F02-04] group must hide Secret Chat");
    assert!(!group_layout.show_block_contact, "[T1-F02-04] group must hide Block Contact");
    assert!(group_layout.show_members_row);
    assert_eq!(group_layout.members_row_label.as_deref(), Some("查看群成员 (3)"));
    assert_eq!(group_layout.edit_row_label, "编辑群组信息");
    assert_eq!(group_layout.leave_row_label.as_deref(), Some("退出群组"));
    assert!(
        group_layout
            .info_rows
            .iter()
            .any(|r| r.subtitle.contains("邀请链接") && r.value == "https://t.me/+GroupInviteAA")
    );
    assert!(
        group_layout
            .info_rows
            .iter()
            .all(|r| r.subtitle != "简介" && r.subtitle != "群组简介"),
        "[T1-F02-04] empty group bio must not render stale bio row"
    );

    // [T1-F02-02] Bot chat branching
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let ok_bot = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.is_bot)
            .unwrap_or(false)
    })
    .await;
    assert!(ok_bot, "[T1-F02-02] bot profile branching failed");
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        let _bot_el = render_bot_profile_main(insp, TdesktopThemePalette::default(), "zh-CN", icx);
    });
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        let st = s.stage.read(cx);
        assert!(st.is_bot_chat);
        assert!(!st.is_group_chat);
        assert!(!insp.members.is_group_or_channel);
        assert!(insp.members.members.is_empty());
    });

    // [T1-F02-03] 1v1 User branching
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
    });
    let ok_user = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.username.contains("alice"))
            .unwrap_or(false)
    })
    .await;
    assert!(ok_user, "[T1-F02-03] 1v1 user profile failed");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        let st = s.stage.read(cx);
        assert!(!st.is_bot_chat && !st.is_group_chat);
        assert!(!insp.profile.is_bot && !insp.members.is_group_or_channel);
    });

    // [T1-F02-04] Render branching in inspector_panels & bot_computer_inspector across 1v1 User vs Group vs Bot
    let user_layout = insp_ent
        .update(&mut cx, |insp, icx| {
            insp.set_language("zh-CN");
            let _contact_el = render_contact_profile_panel(
                &insp.profile,
                &insp.members,
                &insp.shared_media,
                &insp.collapsed_sections,
                insp.is_secret_chat_active,
                insp.is_contact_blocked,
                insp.copied_feedback.as_ref(),
                TdesktopThemePalette::default(),
                "zh-CN",
                icx,
            );
            build_contact_profile_layout(
                &insp.profile,
                &insp.members,
                TdesktopThemePalette::default(),
                "zh-CN",
            )
        })
        .unwrap();
    assert!(!user_layout.is_group_or_channel);
    assert_eq!(user_layout.info_section_title, "联系信息");
    assert_eq!(user_layout.privacy_section_title, "通知与隐私");
    assert!(user_layout.show_secret_chat, "[T1-F02-04] 1v1 user must show Secret Chat");
    assert!(user_layout.show_block_contact, "[T1-F02-04] 1v1 user must show Block Contact");
    assert!(!user_layout.show_members_row);
    assert!(user_layout.members_row_label.is_none());
    assert_eq!(user_layout.edit_row_label, "编辑联系人信息");
    assert!(user_layout.leave_row_label.is_none());
    assert!(user_layout.info_rows.iter().any(|r| r.value == "+1-555-0199"));

    // [T1-F02-05] Third-party Bot (@GrokAI) hides owner Fabushi cards
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROK, cx);
    });
    let ok_grok = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.inspector
                    .read(cx)
                    .profile
                    .username
                    .to_ascii_lowercase()
                    .contains("grokai")
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_grok, "[T1-F02-05] @GrokAI profile load failed");
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        assert!(!insp.profile.is_viewer_owner);
        assert!(insp.active_fabushi_bot.is_none());
        let _bot_el = render_bot_profile_main(insp, TdesktopThemePalette::default(), "en", icx);
    });
}

#[test]
fn tier1_f03_copy_feedback_auto_dismiss_and_switch_clear() {
    run_in_headless_subprocess("tier1_f03_copy_feedback_auto_dismiss_and_switch_clear");
}

async fn t1_f03_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();

    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| insp.open());
        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
    });
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.username.contains("alice"))
            .unwrap_or(false)
    })
    .await;

    // [T1-F03-01] Trigger copy feedback with generation-fenced auto-dismiss timer
    let insp_ent = shell.read_with(&mut cx, |s, _| s.inspector.clone()).unwrap();
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        insp.show_copied_feedback("已复制: @alice", icx);
        insp.pending_action = Some(InspectorAction::CopyText("@alice".to_string()));
        icx.notify();
    });
    assert!(
        insp_ent
            .read_with(&mut cx, |insp, _| insp.copied_feedback.is_some())
            .unwrap_or(false),
        "[T1-F03-01] copied_feedback must be Some after copy"
    );

    // [T1-F03-02] Auto-dismiss within <= 3.0s without user interaction
    let auto_cleared = pump_until(&app, 3200, |app| {
        insp_ent
            .read_with(app, |insp, _| insp.copied_feedback.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(
        auto_cleared,
        "[T1-F03-02] copied_feedback must auto-dismiss within ~2-3s without user interaction"
    );

    // [T1-F03-03] Copy on @fabushi_bot -> immediate switch to Group -> cleared synchronously
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        insp.copied_feedback = Some("已复制: @fabushi_bot".to_string());
        icx.notify();
    });
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
    });
    let _ = insp_ent.read_with(&mut cx, |insp, _| {
        assert!(
            insp.copied_feedback.is_none(),
            "[T1-F03-03] copied_feedback leaked across chat switch to Group: {:?}",
            insp.copied_feedback
        );
    });

    // [T1-F03-04] Closing inspector (clear_peer) clears copied_feedback immediately
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        insp.open();
        insp.copied_feedback = Some("已复制: +1-555".to_string());
        insp.clear_peer();
        icx.notify();
    });
    let _ = insp_ent.read_with(&mut cx, |insp, _| {
        assert!(
            insp.copied_feedback.is_none(),
            "[T1-F03-04] clear_peer must clear copied_feedback"
        );
    });

    // [T1-F03-05] BotProfileBox generation-fenced copy timer + clear on set_profile & close
    let prof = build_fabushi_bot_profile(Some(CID_FABUSHI as i64), true);
    let box_ent = cx
        .update(|cx| {
            cx.new(|_| BotProfileBox::new(prof.clone(), TdesktopThemePalette::default(), "zh-CN"))
        })
        .unwrap();
    let _ = box_ent.update(&mut cx, |b, bcx| {
        b.show_copied_feedback_with_duration(
            "已复制: @fabushi_bot",
            Duration::from_millis(80),
            bcx,
        );
        assert_eq!(b.copied_feedback.as_deref(), Some("已复制: @fabushi_bot"));
    });
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    let _ = box_ent.update(&mut cx, |b, bcx| {
        b.show_copied_feedback_with_duration("已复制: @GrokAI", Duration::from_millis(240), bcx);
    });
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    let still_grok_copy = box_ent
        .read_with(&mut cx, |b, _| {
            b.copied_feedback.as_deref() == Some("已复制: @GrokAI")
        })
        .unwrap_or(false);
    assert!(
        still_grok_copy,
        "[T1-F03-05] BotProfileBox first copy timer prematurely cleared second copy feedback"
    );
    let box_auto_cleared = pump_until(&app, 800, |app| {
        box_ent
            .read_with(app, |b, _| b.copied_feedback.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(
        box_auto_cleared,
        "[T1-F03-05] BotProfileBox::show_copied_feedback_with_duration must auto-dismiss"
    );
    let _ = box_ent.update(&mut cx, |b, bcx| {
        b.show_copied_feedback_with_duration(
            "已复制: @fabushi_bot",
            Duration::from_millis(200),
            bcx,
        );
        b.set_profile(build_grok_bot_profile(Some(CID_GROK as i64), true));
        assert!(
            b.copied_feedback.is_none(),
            "[T1-F03-05] BotProfileBox::set_profile must clear copied_feedback"
        );
        b.show_copied_feedback_with_duration("已复制: @GrokAI", Duration::from_millis(200), bcx);
        b.close();
        assert!(b.copied_feedback.is_none(), "[T1-F03-05] BotProfileBox::close must clear copied_feedback");
        assert!(!b.is_visible);
        let _ = b.take_actions();
        bcx.notify();
    });
}

#[test]
fn tier1_f04_stage_toast_banner_lifecycle_and_isolation() {
    run_in_headless_subprocess("tier1_f04_stage_toast_banner_lifecycle_and_isolation");
}

async fn t1_f04_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T1-F04-01] Stage toast_banner set & cleared on chat switch
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                    st.toast_banner = Some("Copied message link".to_string());
                    st.active_fabushi_bot = Some(FabushiBotSummaryDto {
                        bot_user_id: UID_FABUSHI as i64,
                        username: "fabushi_bot".to_string(),
                        ..Default::default()
                    });
                    st.set_bot_activity("thinking", Some("Working".to_string()));
                    st.pending_reply = Some(ReplyReference {
                        message_id: 42,
                        sender_name: "Sender".to_string(),
                        snippet: "hi".to_string(),
                    });
                });
            });
        });
    });

    // [T1-F04-02], [T1-F04-03], [T1-F04-04] Switch chat via select_authoritative_dialog -> clears toast_banner, active_fabushi_bot, bot_activity, pending_reply
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_GROUP);
                });
            });
        });
    });
    let _ = shell.read_with(&mut cx, |s, cx| {
        let st = s.stage.read(cx);
        assert!(
            st.toast_banner.is_none(),
            "[T1-F04-02] ConversationStage::toast_banner leaked across chat switch: {:?}",
            st.toast_banner
        );
        assert!(
            st.active_fabushi_bot.is_none(),
            "[T1-F04-03] active_fabushi_bot leaked across chat switch"
        );
        assert_eq!(st.bot_activity_state, "idle");
        assert!(st.bot_activity_detail.is_none());
        assert!(st.pending_reply.is_none(), "[T1-F04-04] pending_reply leaked");
        assert!(st.room_agent_states.is_empty(), "[T1-F04-03] room_agent_states not isolated");
    });

    // [T1-F04-05] show_toast_banner_with_duration generation-fenced auto-dismiss + set_chat & clear_selection
    let stage_ent = shell.read_with(&mut cx, |s, _| s.stage.clone()).unwrap();
    let _ = stage_ent.update(&mut cx, |st, st_cx| {
        st.show_toast_banner_with_duration("Toast A", Duration::from_millis(80), st_cx);
        assert_eq!(st.toast_banner.as_deref(), Some("Toast A"));
    });
    app.background_executor()
        .timer(Duration::from_millis(40))
        .await;
    let _ = stage_ent.update(&mut cx, |st, st_cx| {
        st.show_toast_banner_with_duration("Toast B", Duration::from_millis(240), st_cx);
    });
    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    let toast_b_still = stage_ent
        .read_with(&mut cx, |st, _| st.toast_banner.as_deref() == Some("Toast B"))
        .unwrap_or(false);
    assert!(
        toast_b_still,
        "[T1-F04-05] Toast A timer must not dismiss generation-fenced Toast B"
    );
    let toast_auto_cleared = pump_until(&app, 800, |app| {
        stage_ent
            .read_with(app, |st, _| st.toast_banner.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(
        toast_auto_cleared,
        "[T1-F04-05] show_toast_banner_with_duration must auto-dismiss"
    );

    let _ = stage_ent.update(&mut cx, |st, st_cx| {
        st.show_toast_banner_with_duration("Toast C", Duration::from_millis(120), st_cx);
        let gen_before = st.toast_banner_gen;
        let next_cid = if st.active_chat_id == Some(CID_FABUSHI) {
            CID_ALICE
        } else {
            CID_FABUSHI
        };
        st.set_chat(next_cid, "Next Chat", "bot", "NC", 0x8F59F9);
        assert!(st.toast_banner.is_none(), "[T1-F04-05] set_chat must clear toast_banner");
        assert!(st.toast_banner_gen > gen_before, "[T1-F04-05] set_chat must bump toast_banner_gen");
        st.toast_banner = Some("Transient stage toast".to_string());
        st.clear_selection();
        assert!(st.toast_banner.is_none(), "[T1-F04-05] clear_selection must clear toast_banner");
    });
}

#[test]
fn tier1_f05_grok_bot_and_miniapp_spec_parity() {
    run_in_headless_subprocess("tier1_f05_grok_bot_and_miniapp_spec_parity");
}

fn t1_f05_body(cx: &mut App) {
    // [T1-F05-01] docs/grok_bot_interaction_spec.md exists and covers required domains
    let spec_path = workspace_root().join("docs/grok_bot_interaction_spec.md");
    let spec_bytes = std::fs::read(&spec_path).unwrap_or_else(|e| {
        panic!(
            "[T1-F05-01] Missing Grok Bot interaction spec doc at {}: {e}",
            spec_path.display()
        )
    });
    let spec_doc = String::from_utf8(spec_bytes).expect("[T1-F05-01] spec doc must be valid UTF-8");
    assert!(
        spec_doc.contains("GrokAI") && spec_doc.contains("MenuButton"),
        "[T1-F05-01] spec doc must document @GrokAI and MenuButton"
    );
    assert!(
        spec_doc.contains("START") || spec_doc.contains("What can this bot do"),
        "[T1-F05-01] spec doc must document Bot START bar"
    );
    assert!(
        spec_doc.contains("PROVIDER_ERROR") || spec_doc.contains("webmcp_call_tool"),
        "[T1-F05-01] spec doc must document error & tool feedback"
    );

    // [T1-F05-02] @GrokAI classification and slash commands
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            assert!(sb.bot_ids.contains(&CID_GROK));
            assert_eq!(
                sb.dialogs.iter().find(|d| d.id == CID_GROK).map(|d| d.folder.as_str()),
                Some("Bots"),
                "[T1-F05-02] @GrokAI must classify into Bots folder"
            );
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    assert!(select_authoritative_dialog(sb, st, insp, CID_GROK));
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        let comp = s.composer.read(cx);
        assert!(
            comp.bot_slash_commands
                .iter()
                .any(|c| c.command.trim_start_matches('/') == "start"),
            "[T1-F05-02] @GrokAI must expose /start slash command: {:?}",
            comp.bot_slash_commands
        );
        // [T1-F05-03] @GrokAI has no bespoke web app pill inside text box (menu_button_type = commands)
        assert!(
            !comp.bot_menu_controls.is_active,
            "[T1-F05-03] @GrokAI without WebApp must not activate web_app menu button"
        );
    });

    // [T1-F05-04] Group @GrokAI mention candidates + inline keyboard decode
    let cands = build_mention_candidates(
        true,
        &[
            ChatMemberItem {
                user_id: UID_ALICE,
                role: "member".into(),
                admin_title: None,
                admin_rights: 0,
                permissions_bitmask: 0,
                joined_at_ms: 0,
            },
            ChatMemberItem {
                user_id: UID_GROK,
                role: "member".into(),
                admin_title: None,
                admin_rights: 0,
                permissions_bitmask: 0,
                joined_at_ms: 0,
            },
        ],
        &[
            ContactDto {
                user_id: UID_ALICE,
                first_name: "Alice".into(),
                last_name: None,
                username: Some("alice".into()),
                phone: None,
                is_bot: false,
                is_mutual: true,
                chat_id: Some(CID_ALICE as i64),
            },
            ContactDto {
                user_id: UID_GROK,
                first_name: "Grok".into(),
                last_name: None,
                username: Some("GrokAI".into()),
                phone: None,
                is_bot: true,
                is_mutual: false,
                chat_id: Some(CID_GROK as i64),
            },
        ],
        &[],
        None,
    );
    assert!(
        cands.iter().any(|(row, _)| row.username == "GrokAI"),
        "[T1-F05-04] @GrokAI mention candidate missing"
    );
    let kb = decode_inline_keyboard_json(&json!([[
        { "text": "Retry", "callback_data": "grok:retry" },
        { "text": "xAI", "url": "https://x.ai" }
    ]]))
    .expect("[T1-F05-04] inline keyboard must decode");
    assert_eq!(kb[0][0].button_type, InlineButtonType::Callback("grok:retry".to_string()));
    assert_eq!(kb[0][1].button_type, InlineButtonType::Url("https://x.ai".to_string()));

    // [T1-F05-05] Streaming draft / typing activity lifecycle
    let mut stage = ConversationStage::new();
    stage.set_bot_activity("thinking", Some("Thinking…".to_string()));
    assert_eq!(stage.bot_activity_state, "thinking");
    stage.set_bot_activity("idle", None);
    assert_eq!(stage.bot_activity_state, "idle");
}

#[test]
fn tier1_f06_miniapp_menu_button_outside_input_box() {
    run_in_headless_subprocess("tier1_f06_miniapp_menu_button_outside_input_box");
}

async fn t1_f06_body(app: AsyncApp) {
    let mut cx = app.clone();
    // [T1-F06-01] & [T1-F06-02] Apple-style & Classic-style render_text_input_strip_with_options/with_bot_menu + MenuButton lifecycle
    let comp_ent = cx
        .update(|cx| cx.new(|_| CanonicalComposer::new()))
        .unwrap();
    let _ = comp_ent.update(&mut cx, |c, ccx| {
        c.palette = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        c.sync_bot_menu_with_cx(
            UID_FABUSHI,
            "fabushi_bot".to_string(),
            "全球法布施".to_string(),
            "/miniapps/fabushi_bot/fabushi".to_string(),
            ccx,
        );
        let menu_ent = c.ensure_bot_menu_entity(ccx);
        assert!(c.bot_menu_controls.is_active);
        assert!(!c.bot_menu_controls.is_small);
        assert!(menu_ent.read(ccx).is_active);

        let _apple_el = render_text_input_strip_with_options(
            c.text(),
            c.ime_machine.cursor_index,
            None,
            None,
            true,
            false,
            false,
            false,
            false,
            c.calculate_content_height(),
            false,
            Some(menu_ent.clone().into_any_element()),
            None,
            false,
            &c.palette,
            "zh-CN",
            ccx,
            |_this, _act: ComposerBarAction, _cx| {},
        );

        let classic_pal =
            TdesktopThemePalette::build(TdesktopThemeMode::Classic, 0x0A84FF, 14, 18);
        let _classic_el = render_text_input_strip_with_bot_menu(
            c.text(),
            c.ime_machine.cursor_index,
            None,
            None,
            true,
            false,
            false,
            false,
            false,
            c.calculate_content_height(),
            false,
            Some(menu_ent.clone().into_any_element()),
            &classic_pal,
            "zh-CN",
            ccx,
            |_this, _act: ComposerBarAction, _cx| {},
        );
        let _plain_el = render_text_input_strip_with_options(
            "",
            0,
            None,
            None,
            false,
            false,
            false,
            false,
            false,
            px(38.0),
            false,
            None,
            None,
            false,
            &c.palette,
            "zh-CN",
            ccx,
            |_this, _act: ComposerBarAction, _cx| {},
        );

        // Typing shrinks MenuButton to compact is_small = true (36px), clear() restores expanded pill
        c.set_text("hello");
        c.sync_bot_menu_compact_state(Some(ccx));
        assert!(
            c.bot_menu_controls.is_small && c.bot_menu_controls.width_px == 36.0,
            "[T1-F06-01] typing must shrink bot_menu_controls to 36px"
        );
        assert!(
            menu_ent.read(ccx).is_small && menu_ent.read(ccx).width_px == 36.0,
            "[T1-F06-02] typing must sync compact is_small to bot_menu_entity"
        );
        c.clear();
        c.sync_bot_menu_compact_state(Some(ccx));
        assert!(!c.bot_menu_controls.is_small && c.bot_menu_controls.width_px > 36.0);
        assert!(!menu_ent.read(ccx).is_small && menu_ent.read(ccx).width_px > 36.0);
    });

    // [T1-F06-04] External MenuButton entity binds launch URL and forwards launch via CanonicalComposer observer
    let menu_ent = comp_ent
        .update(&mut cx, |c, ccx| c.ensure_bot_menu_entity(ccx))
        .unwrap();
    let _ = menu_ent.read_with(&mut cx, |m, _| {
        assert!(m.is_active);
        assert!(m.width_px >= 48.0);
        assert_eq!(m.menu_url, "/miniapps/fabushi_bot/fabushi");
    });
    let _ = menu_ent.update(&mut cx, |m, mcx| {
        m.pending_launch = Some((
            UID_FABUSHI,
            "fabushi_bot".to_string(),
            "全球法布施".to_string(),
            "/miniapps/fabushi_bot/fabushi".to_string(),
        ));
        mcx.notify();
    });
    let launch_forwarded = pump_until(&app, 1000, |app| {
        comp_ent
            .read_with(app, |c, _| c.pending_launch_miniapp.is_some())
            .unwrap_or(false)
    })
    .await;
    assert!(
        launch_forwarded,
        "[T1-F06-04] CanonicalComposer must observe MenuButton launch click"
    );
    let taken = comp_ent
        .update(&mut cx, |c, _| c.take_launch_miniapp())
        .unwrap();
    assert_eq!(
        taken,
        Some((
            UID_FABUSHI,
            "fabushi_bot".to_string(),
            "全球法布施".to_string(),
            "/miniapps/fabushi_bot/fabushi".to_string()
        ))
    );

    // [T1-F06-03] & [T1-F06-05] AppShell Mini App MenuButton + / slash coexistence & switch clear
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        let comp = s.composer.read(cx);
        assert!(comp.bot_menu_controls.is_active, "[T1-F06-03] menu button must activate");
        assert_eq!(comp.bot_menu_controls.text, "全球法布施");
        assert!(
            !comp.bot_slash_commands.is_empty(),
            "[T1-F06-03] slash commands must coexist with external Mini App menu button"
        );
    });
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_GROUP);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        assert!(
            !s.composer.read(cx).bot_menu_controls.is_active,
            "[T1-F06-05] switching to group must deactivate bot_menu_controls"
        );
    });
    let _ = app;
}

#[test]
fn tier1_f07_plus_new_chat_composer_suppression() {
    run_in_headless_subprocess("tier1_f07_plus_new_chat_composer_suppression");
}

async fn t1_f07_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // Activate @fabushi_bot with menu button, slash commands, draft, reply
    let _ = shell.update(&mut cx, |s, cx| {
        s.theme_state.palette =
            TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        s.sync_theme_and_locale_to_views(cx);
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        s.composer.update(cx, |c, _| {
            c.set_text("stale fabushi draft");
            c.begin_reply(55, "Fabushi".to_string(), "hi".to_string());
            c.set_hermes_slash_enabled(true);
        });
    });

    // [T1-F07-01] Enter `+` compose mode -> clear bot_menu_controls, slash, reply, draft
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
        cx.notify();
    });

    let entered_plus = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.composer.read(cx).is_compose_isolated && s.should_hide_composer(cx)
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        entered_plus,
        "[T1-F07-01/02] observe_sidebar_entity must isolate composer in + compose mode"
    );

    // Verify AppShell::should_hide_composer & composer suppression
    let _ = shell.read_with(&mut cx, |s, cx| {
        assert!(
            s.should_hide_composer(cx) && !s.composer.read(cx).is_input_focused,
            "[T1-F07-02] composer must be hidden in + compose mode with 0 recipients"
        );
        let comp = s.composer.read(cx);
        assert!(
            !comp.bot_menu_controls.is_active,
            "[T1-F07-01] bot_menu_controls remained active after entering + compose"
        );
        assert!(
            comp.bot_slash_commands.is_empty(),
            "[T1-F07-01] bot_slash_commands remained populated in + compose"
        );
        assert!(
            !comp.hermes_slash_enabled,
            "[T1-F07-01] hermes_slash_enabled remained true in + compose"
        );
        assert!(
            comp.replying_to.is_none(),
            "[T1-F07-01] replying_to leaked into + compose"
        );
        assert!(
            comp.text().is_empty(),
            "[T1-F07-01] previous chat draft leaked into + compose: {:?}",
            comp.text()
        );
    });

    // [T1-F07-03] Adding >=1 recipient chip reveals composer via observer without restoring prior bot menu
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.add_compose_recipient(ComposeRecipientChip {
                user_id: UID_ALICE,
                chat_id: Some(CID_ALICE),
                username: Some("alice".to_string()),
                display_name: "Alice".to_string(),
                initials: "AL".to_string(),
                is_bot: false,
                bot_id: None,
                avatar_color: 0x0A84FF,
                avatar_shape: None,
            });
            scx.notify();
        });
    });
    let recipient_synced = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| {
                !s.should_hide_composer(cx)
                    && !s.sidebar.read(cx).compose_draft.selected_recipients.is_empty()
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        recipient_synced,
        "[T1-F07-03] adding 1 recipient must unhide composer"
    );
    let _ = shell.read_with(&mut cx, |s, cx| {
        assert!(!s.should_hide_composer(cx));
        assert!(s.is_compose_or_creation_modal_active(cx));
        assert!(
            !s.composer.read(cx).bot_menu_controls.is_active,
            "[T1-F07-03] adding recipient must never restore prior bot MenuButton"
        );
    });

    // [T1-F07-04] Cancel `+` compose mode -> observer restores @fabushi_bot menu & slash commands without manual sync
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    let restored = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| {
                !s.composer.read(cx).is_compose_isolated
                    && s.composer.read(cx).bot_menu_controls.is_active
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        restored,
        "[T1-F07-04] exiting + compose must restore active bot MenuButton via observe_sidebar_entity"
    );
    let _ = shell.read_with(&mut cx, |s, cx| {
        let comp = s.composer.read(cx);
        assert!(comp.bot_menu_controls.is_active);
        assert!(!comp.bot_slash_commands.is_empty());
        assert!(!s.should_hide_composer(cx));
    });

    // [T1-F07-05] Contacts modal open also clears/suppresses bot menu & hides composer
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.open_contacts_modal();
            scx.notify();
        });
        cx.notify();
    });
    let modal_isolated = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    assert!(
        modal_isolated,
        "[T1-F07-05] opening contacts modal must trigger compose/modal isolation"
    );
    let _ = shell.read_with(&mut cx, |s, cx| {
        assert!(s.is_compose_or_creation_modal_active(cx));
        assert!(s.should_hide_composer(cx));
        assert!(!s.composer.read(cx).bot_menu_controls.is_active);
    });
}

#[test]
fn tier1_f08_bot_conversation_start_bar_and_keyboard() {
    run_in_headless_subprocess("tier1_f08_bot_conversation_start_bar_and_keyboard");
}

async fn t1_f08_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T1-F08-01] Unstarted bot shows START bar and hides composer
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_GROK);
                    st.messages.clear();
                    st.set_bot_prestart_state(
                        Some("What can this bot do? Ask Grok anything.".to_string()),
                        true,
                        false,
                    );
                });
                st_cx.notify();
            });
        });
        assert!(s.stage.read(cx).show_bot_start_bar);
        assert!(s.stage.read(cx).bot_description_card.is_some());
        assert!(
            s.should_hide_composer(cx),
            "[T1-F08-01] AppShell::should_hide_composer must be true while show_bot_start_bar is active"
        );
    });

    // [T1-F08-02] Completing START clears show_bot_start_bar and restores composer
    let _ = shell.update(&mut cx, |s, cx| {
        s.stage.update(cx, |st, st_cx| {
            st.set_bot_prestart_state(None, false, false);
            st_cx.notify();
        });
        assert!(!s.stage.read(cx).show_bot_start_bar);
        assert!(
            !s.should_hide_composer(cx),
            "[T1-F08-02] AppShell::should_hide_composer must be false once START completes"
        );
    });

    // [T1-F08-03] BotKeyboard RESIZE flag + CanonicalComposer -> AppShell observer outgoing message & switch clear
    let mut kb = BotKeyboard::new();
    let rows = vec![
        vec![
            BotKeyboardButton::new("Ask Grok", BotKeyboardButtonKind::Default),
            BotKeyboardButton::new("Model", BotKeyboardButtonKind::Default),
        ],
        vec![BotKeyboardButton::new("WebApp", BotKeyboardButtonKind::WebApp("/app".into()))],
    ];
    kb.set_markup(rows.clone(), kb_flags::RESIZE, Some("Choose option".into()));
    assert!(kb.has_markup(), "[T1-F08-03] has_markup should be true");
    assert!(!kb.maximize_size, "[T1-F08-03] RESIZE should set maximize_size = false");
    assert!(kb.calculated_height() < kb.max_outer_height);

    let _ = shell.update(&mut cx, |s, cx| {
        s.composer.update(cx, |c, ccx| {
            c.set_bot_reply_keyboard(
                rows.clone(),
                kb_flags::RESIZE | kb_flags::SINGLE_USE,
                Some("Choose".into()),
            );
            assert!(c.bot_keyboard.has_markup() && c.is_bot_keyboard_open);
            c.toggle_bot_keyboard();
            assert!(!c.is_bot_keyboard_open);
            c.toggle_bot_keyboard();
            assert!(c.is_bot_keyboard_open);
            c.pending_bot_keyboard_press =
                Some(BotKeyboardButton::new("Ask Grok", BotKeyboardButtonKind::Default));
            if c.bot_keyboard.single_use {
                c.is_bot_keyboard_open = false;
            }
            ccx.notify();
        });
    });
    let sent_via_kb = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.stage.read(cx).messages.iter().any(|m| {
                    matches!(&m.content, MessageContent::Text { text, .. } if text == "Ask Grok")
                })
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        sent_via_kb,
        "[T1-F08-03] pending_bot_keyboard_press must dispatch outgoing message via AppShell observer"
    );
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
    });
    let kb_cleared_on_switch = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| !s.composer.read(cx).bot_keyboard.has_markup())
            .unwrap_or(false)
    })
    .await;
    assert!(
        kb_cleared_on_switch,
        "[T1-F08-03] switching away from bot chat must clear bot_keyboard via AppShell observer"
    );

    // [T1-F08-04] BotKeyboard SINGLE_USE + FORCE_REPLY
    kb.set_markup(
        rows,
        kb_flags::SINGLE_USE | kb_flags::FORCE_REPLY | kb_flags::RESIZE,
        Some("Reply here".into()),
    );
    assert!(kb.single_use);
    assert!(kb.force_reply);
    assert_eq!(kb.placeholder.as_deref(), Some("Reply here"));

    // [T1-F08-05] ReplyKeyboardRemove
    kb.set_markup(Vec::new(), 0, None);
    assert!(!kb.has_markup());
    assert_eq!(kb.natural_height(), 0.0);
    let _ = app;
}

#[test]
fn tier1_f09_window_toast_auto_dismiss_and_non_overlapping_banner() {
    run_in_headless_subprocess("tier1_f09_window_toast_auto_dismiss_and_non_overlapping_banner");
}

async fn t1_f09_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T1-F09-01] Toast auto-dismisses after duration_ms
    let _ = shell.update(&mut cx, |s, cx| {
        s.show_toast("PROVIDER_ERROR: No AI provider configured", 800, cx);
        assert!(s.window_runtime.controller.current_toast.is_some());
    });
    let cleared = pump_until(&app, 1800, |app| {
        shell
            .read_with(app, |s, _| s.window_runtime.controller.current_toast.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(
        cleared,
        "[T1-F09-01] #window-toast-banner must auto-dismiss after duration_ms without user click"
    );

    // [T1-F09-02] Non-overlapping PROVIDER_ERROR toast above tall 3-line composer
    let _ = shell.update(&mut cx, |s, cx| {
        s.window_runtime.apply_viewport_width(800.0);
        s.theme_state.palette =
            TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        s.sync_theme_and_locale_to_views(cx);
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        s.composer.update(cx, |c, ccx| {
            c.set_text("Line 1\nLine 2\nLine 3");
            assert_eq!(c.calculate_line_count(), 3);
            assert_eq!(c.calculate_content_height(), px(84.0));
            let menu_ent = c.ensure_bot_menu_entity(ccx);
            let _strip_el = render_text_input_strip_with_options(
                c.text(),
                c.ime_machine.cursor_index,
                None,
                None,
                true,
                false,
                false,
                false,
                false,
                c.calculate_content_height(),
                c.is_overflow_scroll_active(),
                Some(menu_ent.into_any_element()),
                None,
                c.compose_send_disabled,
                &c.palette,
                "zh-CN",
                ccx,
                |_this, _act: ComposerBarAction, _cx| {},
            );
            ccx.notify();
        });
        s.show_chat_toast(
            CID_FABUSHI,
            "PROVIDER_ERROR: Provider configuration error — please configure an AI provider in Settings -> AI Providers before running Fabushi.",
            4000,
            cx,
        );
    });

    let _ = shell.update(&mut cx, |s, cx| {
        assert!(!s.should_hide_composer(cx));
        let toast_id = {
            let t = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("[T1-F09-02] PROVIDER_ERROR toast should be active");
            assert_eq!(t.chat_id, Some(CID_FABUSHI));
            assert_eq!(t.duration_ms, 4000);
            assert_eq!(s.scheduled_toast_id, Some(t.id));
            t.id
        };
        // Composer remains interactive while toast is active
        s.composer.update(cx, |c, ccx| {
            assert!(!c.compose_send_disabled);
            c.ime_machine.insert_text(" + more");
            assert!(c.text().ends_with(" + more"));
            ccx.notify();
        });
        // ID fence rejects wrong toast id, dismiss_current_toast clears cleanly
        assert!(!s.window_runtime.controller.dismiss_toast_if_id(toast_id.wrapping_add(99)));
        assert!(s.window_runtime.controller.dismiss_toast_if_id(toast_id));
        s.schedule_current_toast_dismiss(cx);
        assert!(s.window_runtime.controller.current_toast.is_none());
        assert!(s.scheduled_toast_id.is_none());

        // Local Fabushi error card helper caps toast summary to <= 80 chars single-line
        s.apply_local_fabushi_turn_error_to_shell(
            CID_FABUSHI,
            UID_FABUSHI as i64,
            "全球法布施",
            "PROVIDER_ERROR: missing API key\nline2\nline3 very long detail",
            cx,
        );
        let summary_toast = s
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .expect("[T1-F09-02] apply_local_fabushi_turn_error_to_shell must enqueue concise toast");
        assert!(summary_toast.text.chars().count() <= 80);
        assert!(!summary_toast.text.contains('\n'));
        s.window_runtime.controller.dismiss_current_toast();

        // Re-arm chat-scoped toast for [T1-F09-03] chat-switch clear check
        s.show_chat_toast(CID_FABUSHI, "PROVIDER_ERROR: active chat error", 4000, cx);
    });

    // [T1-F09-03] Chat switch clears chat-scoped error toast
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
    });
    let _ = shell.read_with(&mut cx, |s, _| {
        assert!(
            s.window_runtime.controller.current_toast.is_none(),
            "[T1-F09-03] error toast leaked across chat switch"
        );
    });

    // [T1-F09-04] & [T1-F09-05] Queue ordering & explicit dismiss
    let _ = shell.update(&mut cx, |s, _| {
        s.window_runtime.enqueue_toast("Toast One", 3000);
        s.window_runtime.enqueue_toast("Toast Two", 3000);
        assert_eq!(
            s.window_runtime.controller.current_toast.as_ref().map(|t| t.text.as_str()),
            Some("Toast One")
        );
        s.window_runtime.controller.dismiss_current_toast();
        assert_eq!(
            s.window_runtime.controller.current_toast.as_ref().map(|t| t.text.as_str()),
            Some("Toast Two")
        );
        s.window_runtime.controller.dismiss_current_toast();
        assert!(s.window_runtime.controller.current_toast.is_none());
    });
}

#[test]
fn tier1_f10_local_fabushi_per_user_provider_and_inline_error() {
    run_in_headless_subprocess("tier1_f10_local_fabushi_per_user_provider_and_inline_error");
}

async fn t1_f10_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();

    // [T1-F10-01] Global hydration marks local fabushi usable
    let host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("create ephemeral host"),
    );
    assert!(!has_usable_local_fabushi_provider(&host));
    let count = hydrate_local_subscriptions_into_fabushi(&host, &[sample_vertex_item()]);
    assert_eq!(count, 1);
    assert!(
        has_usable_local_fabushi_provider(&host),
        "[T1-F10-01] host should have usable local fabushi provider after hydration"
    );

    // [T1-F10-02] Per-user provider hydration (user 100_001)
    let user_host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("create user host"),
    );
    assert!(!has_usable_local_fabushi_provider_for_user(&user_host, Some("100001")));
    let per_user_count = hydrate_local_subscriptions_into_fabushi_for_user(
        &user_host,
        &[sample_vertex_item()],
        Some("100001"),
    );
    assert_eq!(per_user_count, 1);
    assert!(
        has_usable_local_fabushi_provider_for_user(&user_host, Some("100001")),
        "[T1-F10-02] per-user wrapper must report usable after subscription hydration"
    );
    assert!(
        !has_usable_local_fabushi_provider_for_user(&user_host, Some("200002")),
        "[T1-F10-02] user 100001 hydration must stay isolated from user 200002"
    );

    // [T1-F10-03] Vault bundle hydration
    let bundle_host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("create bundle host"),
    );
    let bundle = AiOauthVaultBundle {
        providers_json: serde_json::Value::Null,
        subscriptions_json: serde_json::to_value(vec![sample_vertex_item()]).unwrap(),
        updated_at_ms: 1_700_000_000_000,
    };
    let n = hydrate_ai_oauth_vault_bundle_into_fabushi(&bundle_host, &bundle)
        .expect("hydrate vault bundle");
    assert_eq!(n, 1);
    assert!(has_usable_local_fabushi_provider(&bundle_host));

    // [T1-F10-04] Unconfigured host -> spawn_local_fabushi_turn emits structured inline PROVIDER_ERROR card with hermes:open_ai_providers CTA
    let empty_host = Arc::new(
        FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("create empty host"),
    );
    empty_host.core_engine().providers().set_allow_local_fallback(false);
    empty_host.core_engine().providers().set_offline_rule_bot_allowed(false);
    empty_host.sync_all_providers();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();
    let _ = shell.update(&mut cx, |s, cx| {
        s.attach_local_fabushi_host(empty_host);
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        let target = s
            .resolve_fabushi_bot_target(CID_FABUSHI, cx)
            .expect("resolve fabushi target");
        s.spawn_local_fabushi_turn(CID_FABUSHI, target, None, "你好".to_string(), cx);
    });

    let got_inline_cta = pump_until(&app, 4000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.stage.read(cx).messages.iter().any(|m| {
                    m.bot_keyboard.as_ref().is_some_and(|rows| {
                        rows.iter().flatten().any(|b| {
                            b.button_type
                                == InlineButtonType::Callback("hermes:open_ai_providers".into())
                        })
                    })
                })
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        got_inline_cta,
        "[T1-F10-04] spawn_local_fabushi_turn without provider must insert inline error card with hermes:open_ai_providers CTA"
    );

    // [T1-F10-05] Clicking/executing hermes:open_ai_providers navigates to Settings::AiProviders
    let _ = shell.update(&mut cx, |s, cx| {
        s.execute_hermes_inline_callback("hermes:open_ai_providers".to_string(), cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert_eq!(
            s.settings_router.read(cx).settings_section,
            SettingsSectionType::AiProviders
        );
    });
}

#[test]
fn tier1_f11_webmcp_cross_frame_iframe_bridge_and_readiness() {
    run_in_headless_subprocess("tier1_f11_webmcp_cross_frame_iframe_bridge_and_readiness");
}

async fn t1_f11_body(app: AsyncApp) {
    let mut cx = app.clone();

    // [T1-F11-01] Queue command before ready; web_app_ready and host_wrapper_only do NOT arm readiness;
    // __gpui_webmcp_page_ready flushes; gpui_webmcp_unready resets; gpui_webmcp_tools_changed re-arms.
    let mut wv = BotAttachWebView::new();
    wv.bot_username = "fabushi_bot".to_string();
    assert!(!wv.webmcp_page_ready);
    wv.pending_webmcp_queue.push_back(MiniAppWebMcpCommand {
        request_id: "req_1".to_string(),
        bot_username: "fabushi_bot".to_string(),
        short_name: "fabushi".to_string(),
        chat_id: CID_FABUSHI as i64,
        bot_user_id: UID_FABUSHI as i64,
        app_title: "全球法布施".to_string(),
        operation: "discover".to_string(),
        tool_name: None,
        arguments: json!({}),
        status_message: None,
    });
    wv.handle_bridge_event("web_app_ready", "{}");
    assert!(
        !wv.webmcp_page_ready && wv.pending_webmcp_queue.len() == 1,
        "[T1-F11-01] web_app_ready must not arm WebMCP readiness or drain queue"
    );
    wv.handle_bridge_event(
        "__gpui_webmcp_page_ready",
        r#"{"host_wrapper_only":true,"url":"http://127.0.0.1:8080/miniapps/fabushi_bot/fabushi"}"#,
    );
    assert!(
        !wv.webmcp_page_ready && wv.pending_webmcp_queue.len() == 1,
        "[T1-F11-01] host_wrapper_only ready event must be ignored"
    );
    wv.handle_bridge_event(
        "__gpui_webmcp_page_ready",
        &json!({
            "url": "http://127.0.0.1:18090/",
            "in_iframe": true,
            "tools": [{
                "name": "fabushi_send_dharma",
                "description": "Send Dharma",
                "input_schema": { "type": "object" },
                "read_only": false
            }],
            "state_summary": { "merit": 108 }
        })
        .to_string(),
    );
    assert!(wv.webmcp_page_ready && wv.webmcp_iframe_ready, "[T1-F11-01] ready flags must become true");
    assert!(wv.webmcp_discovered_tools.iter().any(|t| t.name == "fabushi_send_dharma"));
    assert!(
        wv.pending_webmcp_queue.is_empty(),
        "[T1-F11-01] queued commands must flush upon __gpui_webmcp_page_ready"
    );
    wv.handle_bridge_event("gpui_webmcp_unready", "{}");
    assert!(!wv.webmcp_page_ready && !wv.webmcp_iframe_ready);
    assert!(wv.webmcp_discovered_tools.is_empty());
    assert!(wv.webmcp_needs_rediscover);
    wv.handle_bridge_event(
        "gpui_webmcp_tools_changed",
        &json!({
            "tools": [{
                "name": "get_state",
                "description": "Get State",
                "input_schema": { "type": "object" },
                "read_only": true
            }]
        })
        .to_string(),
    );
    assert!(wv.webmcp_page_ready && wv.webmcp_iframe_ready);
    assert!(wv.webmcp_discovered_tools.iter().any(|t| t.name == "get_state"));

    // [T1-F11-02] webmcp_state_update object & string fallback
    wv.handle_bridge_event(
        "webmcp_state_update",
        r#"{"state_summary":{"merit":108,"screen":"home"}}"#,
    );
    assert_eq!(
        wv.last_webmcp_state_summary.as_ref().and_then(|v| v["merit"].as_i64()),
        Some(108)
    );
    wv.handle_bridge_event("webmcp_state_update", r#"{"state_summary":"禅室在线"}"#);
    assert_eq!(
        wv.last_webmcp_state_summary
            .as_ref()
            .and_then(|v| v["status_text"].as_str()),
        Some("禅室在线")
    );

    // [T1-F11-03] request_takeover switches control_mode & opens WebView
    wv.is_open = false;
    wv.is_floating_pip = true;
    wv.dispatch_webmcp_command(MiniAppWebMcpCommand {
        request_id: "req_takeover".to_string(),
        bot_username: "fabushi_bot".to_string(),
        short_name: "fabushi".to_string(),
        chat_id: CID_FABUSHI as i64,
        bot_user_id: UID_FABUSHI as i64,
        app_title: "全球法布施".to_string(),
        operation: "request_takeover".to_string(),
        tool_name: None,
        arguments: json!({}),
        status_message: Some("请完成登录验证".to_string()),
    });
    assert_eq!(wv.control_mode, MiniAppControlMode::WaitingForTakeover);
    assert!(wv.is_open && !wv.is_floating_pip);

    // [T1-F11-04] Part 1: Cross-frame TELEGRAM_WEBVIEW_PROXY_BOOT_JS runtime via Node VM (macOS)
    #[cfg(target_os = "macos")]
    if let Ok(ver) = std::process::Command::new("node").arg("--version").output() {
        if ver.status.success() {
            let boot_js_lit = serde_json::to_string(TELEGRAM_WEBVIEW_PROXY_BOOT_JS).unwrap();
            let driver = format!(
                r#"
const assert = require('assert');
const vm = require('vm');
const bootJs = {boot_js_lit};

function createEnv(isTop, hasTgAppFrame) {{
  const winListeners = {{}};
  const bridgeEvents = [];
  const parentMessages = [];
  const doc = {{
    readyState: 'loading',
    title: 'Mock',
    getElementById(id) {{ return (hasTgAppFrame && id === 'tg-app-frame') ? {{ id: 'tg-app-frame' }} : null; }},
    querySelector() {{ return null; }},
    querySelectorAll() {{ return []; }}
  }};
  const win = {{
    location: {{ href: isTop ? 'http://127.0.0.1:8080/miniapps/fabushi_bot/fabushi' : 'http://127.0.0.1:18090/' }},
    top: null,
    parent: null,
    document: doc,
    webkit: {{ messageHandlers: {{ telegramWebviewProxy: {{ postMessage(p) {{ bridgeEvents.push(p); }} }} }} }},
    addEventListener(type, fn) {{ (winListeners[type] = winListeners[type] || []).push(fn); }}
  }};
  win.top = isTop ? win : {{ isOuter: true }};
  win.parent = isTop ? win : {{ postMessage(msg, targetOrigin) {{ parentMessages.push({{ msg, targetOrigin }}); }} }};
  const ctx = vm.createContext({{
    window: win, document: doc, navigator: {{}}, Event: function(t) {{ this.type = t; }},
    setTimeout, clearTimeout, Promise, Object, Array, String, Number, Date, JSON
  }});
  ctx.window.navigator = ctx.navigator;
  vm.runInContext(bootJs, ctx);
  return {{ ctx, doc, winListeners, bridgeEvents, parentMessages }};
}}

(async () => {{
  const topEnv = createEnv(true, true);
  topEnv.doc.readyState = 'interactive';
  for (const fn of (topEnv.winListeners.DOMContentLoaded || [])) fn();
  assert(!topEnv.bridgeEvents.some(e => e.eventType === '__gpui_webmcp_page_ready'),
    'top frame with #tg-app-frame must suppress premature __gpui_webmcp_page_ready');

  const childEnv = createEnv(false, false);
  childEnv.doc.readyState = 'complete';
  for (const fn of (childEnv.winListeners.DOMContentLoaded || [])) fn();
  assert(childEnv.parentMessages.some(m => m.msg && m.msg.type === 'gpui_webmcp_ready'),
    'child frame must post gpui_webmcp_ready to window.parent');

  const replies = [];
  const src = {{ postMessage(msg, targetOrigin) {{ replies.push({{ msg, targetOrigin }}); }} }};
  for (const fn of (childEnv.winListeners.message || [])) {{
    fn({{
      source: src,
      data: {{ type: 'gpui_webmcp_invoke', request_id: 'req_vm', operation: 'call_tool', tool_name: 'fabushi_send_dharma', arguments: {{ sutra: '金刚经' }} }}
    }});
  }}
  setTimeout(() => {{
    childEnv.ctx.navigator.modelContext.registerTool({{
      name: 'fabushi_send_dharma', description: 'Send', inputSchema: {{ type: 'object' }},
      execute: (args) => ({{ accepted: true, sutra: args.sutra }})
    }});
  }}, 30);
  await new Promise(r => setTimeout(r, 120));
  assert.strictEqual(replies.length, 1);
  assert.strictEqual(replies[0].msg.type, 'gpui_webmcp_response');
  assert.strictEqual(replies[0].msg.payload.ok, true);
  assert.strictEqual(replies[0].msg.payload.result.sutra, '金刚经');
  assert(childEnv.parentMessages.some(m => m.msg && m.msg.type === 'gpui_webmcp_tools_changed'));
  process.exit(0);
}})().catch(err => {{ console.error(err); process.exit(1); }});
"#
            );
            let out = std::process::Command::new("node")
                .arg("-e")
                .arg(&driver)
                .output()
                .expect("run node boot js VM");
            assert!(
                out.status.success(),
                "[T1-F11-04] Node VM cross-frame WebMCP boot JS failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    // [T1-F11-04] Part 2: AppShell + WsChannel + BotAttachWebView live WS MiniAppWebMcpCommand -> MiniAppWebMcpResult round-trip
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let (client_tx, mut client_rx) = tokio::sync::mpsc::unbounded_channel::<WsClientFrame>();
    let (_server_tx, server_rx) = tokio::sync::mpsc::unbounded_channel::<WsServerFrame>();
    client.ws_channel.attach_channels(client_tx, server_rx);
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    let _ = shell.update(&mut cx, |s, cx| {
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::MiniAppWebMcpCommand(MiniAppWebMcpCommand {
                request_id: "req_f11_cmd".to_string(),
                bot_username: "fabushi_bot".to_string(),
                short_name: "fabushi".to_string(),
                chat_id: CID_FABUSHI as i64,
                bot_user_id: UID_FABUSHI as i64,
                app_title: "全球法布施".to_string(),
                operation: "discover".to_string(),
                tool_name: None,
                arguments: json!({}),
                status_message: None,
            })),
            cx,
        );
    });
    let launched = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.miniapps
                    .miniapp_overlay
                    .as_ref()
                    .is_some_and(|ov| ov.read(cx).launch_context.is_some())
            })
            .unwrap_or(false)
    })
    .await;
    assert!(launched, "[T1-F11-04] MiniAppWebMcpCommand must launch overlay with context");

    let ov = shell
        .read_with(&mut cx, |s, _| s.miniapps.miniapp_overlay.clone().unwrap())
        .unwrap();
    let _ = ov.update(&mut cx, |w, wcx| {
        w.handle_bridge_event(
            "webmcp_response",
            r#"{"request_id":"req_f11_ws","ok":true,"result":{"merit":108},"tools":[],"state_summary":{"merit":108},"control_mode":"ai_controlling"}"#,
        );
        wcx.notify();
    });
    let got_ws_reply = pump_until(&app, 1500, |_| {
        while let Ok(frame) = client_rx.try_recv() {
            if let WsClientFrame::MiniAppWebMcpResult(resp) = frame {
                if resp.request_id == "req_f11_ws" && resp.ok {
                    return true;
                }
            }
        }
        false
    })
    .await;
    assert!(
        got_ws_reply,
        "[T1-F11-04] BotAttachWebView webmcp_response must round-trip via AppShell as WsClientFrame::MiniAppWebMcpResult"
    );

    // [T1-F11-05] Bridge events for MainButton, BackButton, Popup, Close
    wv.handle_bridge_event(
        "web_app_setup_main_button",
        r#"{"text":"Confirm","is_visible":true,"is_active":true}"#,
    );
    assert!(wv.ui_schema.main_button.as_ref().is_some_and(|b| b.text == "Confirm"));
    wv.handle_bridge_event("web_app_setup_back_button", r#"{"is_visible":true}"#);
    assert!(wv.ui_schema.back_button_visible);
    wv.handle_bridge_event(
        "web_app_open_popup",
        r#"{"title":"Notice","message":"Ready","buttons":[{"id":"ok","type":"ok","text":"OK"}]}"#,
    );
    assert!(wv.custom_popup.is_some());
    wv.handle_bridge_event("web_app_close", "{}");
    assert!(!wv.is_open);
}

#[test]
fn tier1_f12_webmcp_call_tool_step_trace_and_failure_guard() {
    run_in_headless_subprocess("tier1_f12_webmcp_call_tool_step_trace_and_failure_guard");
}

async fn t1_f12_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();

    // [T1-F12-01] & [T1-F12-03] Real HermesEngine + mock OpenAI HTTP server + PlatformImBridge webmcp_call_tool failure & non-fabricated response
    let (openai_base, saw_tool_error, turn_calls) = client
        .runtime()
        .spawn(spawn_mock_openai_webmcp_server())
        .await
        .expect("spawn openai mock");
    let bridge_calls = Arc::new(Mutex::new(Vec::new()));
    let bridge = Arc::new(MockWebMcpImBridge {
        calls: bridge_calls.clone(),
    });
    let hermes_tmp = tempfile::TempDir::new().expect("hermes tmpdir");
    let mut cfg = HermesEngineConfig::default();
    cfg.workspace_root = hermes_tmp.path().to_path_buf();
    cfg.data_dir = None;
    let engine = HermesEngine::new(cfg).expect("create HermesEngine");
    engine
        .providers()
        .register_provider(ProviderConfig::openai_compatible(
            "mock_f12",
            "Mock F12",
            openai_base,
            "gpt-4o-mock",
            "sk-test-f12",
        ));
    engine
        .providers()
        .set_active("mock_f12", Some("gpt-4o-mock"))
        .expect("set active provider");
    engine.providers().set_allow_local_fallback(false);
    engine.set_platform_im_bridge(bridge.clone());

    let events = client
        .runtime()
        .spawn(async move {
            let mut rx = engine
                .run_turn_stream(
                    HermesTurnRequest::new("请帮我法布施金刚经")
                        .with_chat_bot(CID_FABUSHI as i64, UID_FABUSHI.to_string()),
                )
                .await
                .expect("start hermes turn stream");
            let mut collected = Vec::new();
            while let Some(ev) = rx.recv().await {
                let is_done = matches!(ev, HermesEvent::Done { .. } | HermesEvent::Error { .. });
                collected.push(ev);
                if is_done {
                    break;
                }
            }
            collected
        })
        .await
        .expect("join hermes turn");

    assert!(
        events.iter().any(|ev| matches!(
            ev,
            HermesEvent::ToolStart { tool_name, arguments, .. }
                if tool_name == "webmcp_call_tool"
                    && arguments.get("tool_name").and_then(|v| v.as_str()) == Some("fabushi_send_dharma")
        )),
        "[T1-F12-01] HermesEngine must emit ToolStart for webmcp_call_tool"
    );
    assert!(
        events.iter().any(|ev| matches!(
            ev,
            HermesEvent::ToolResult { tool_name, success: false, output, .. }
                if tool_name == "webmcp_call_tool"
                    && output.contains("未注册的 WebMCP 工具: fabushi_send_dharma")
                    && output.contains("打开或刷新「全球法布施」小程序")
        )),
        "[T1-F12-01/03] HermesEngine must emit ToolResult(success=false) with actionable Mini App guidance"
    );
    assert!(
        events.iter().any(|ev| matches!(
            ev,
            HermesEvent::Done { tool_calls_count: 1, response_text, .. }
                if response_text.contains("操作失败") && response_text.contains("fabushi_send_dharma")
        )),
        "[T1-F12-03] HermesEngine Done event must report failure without fabricated success"
    );
    assert!(*saw_tool_error.lock().unwrap(), "[T1-F12-03] Tool error must be passed back to model");
    assert_eq!(*turn_calls.lock().unwrap(), 2);
    assert_eq!(bridge_calls.lock().unwrap().len(), 1);

    // [T1-F12-02] Real HermesToolExecutor::execute("webmcp_call_tool", ...) guardrails & disconnected error
    let (res_empty_tool, res_no_bridge, res_cross_chat, res_disconnected) = client
        .runtime()
        .spawn(async move {
            let mut ctx = HermesToolContext::new(hermes_tmp.path());
            ctx.chat_id = Some(CID_FABUSHI as i64);
            let r_no_bridge = HermesToolExecutor::execute(
                "webmcp_call_tool",
                json!({ "tool_name": "fabushi_send_dharma" }),
                &ctx,
            )
            .await;

            ctx.platform_im_bridge = Some(bridge);
            let r_empty = HermesToolExecutor::execute(
                "webmcp_call_tool",
                json!({ "tool_name": "   " }),
                &ctx,
            )
            .await;
            let r_cross = HermesToolExecutor::execute(
                "webmcp_call_tool",
                json!({ "chat_id": 999_999, "tool_name": "fabushi_send_dharma" }),
                &ctx,
            )
            .await;
            let r_disc = HermesToolExecutor::execute(
                "webmcp_call_tool",
                json!({ "tool_name": "fabushi_send_dharma", "arguments": { "sutra": "金刚经" } }),
                &ctx,
            )
            .await;
            (r_empty, r_no_bridge, r_cross, r_disc)
        })
        .await
        .expect("join tool executor checks");
    assert!(!res_empty_tool.success && res_empty_tool.output.contains("non-empty 'tool_name'"));
    assert!(!res_no_bridge.success && res_no_bridge.output.contains("PlatformImBridge is not attached"));
    assert!(!res_cross_chat.success && res_cross_chat.output.contains("Cross-chat target mismatch"));
    assert!(
        !res_disconnected.success
            && res_disconnected
                .output
                .contains("打开或刷新「全球法布施」小程序")
    );

    // [T1-F12-04] AppShell WS BotActivityChanged -> NewMessage -> EditMessage -> sidebar preview -> handle_stage_bot_button
    let (client_tx, mut client_rx) = tokio::sync::mpsc::unbounded_channel::<WsClientFrame>();
    let (_server_tx, server_rx) = tokio::sync::mpsc::unbounded_channel::<WsServerFrame>();
    client.ws_channel.attach_channels(client_tx, server_rx);
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::BotActivityChanged {
                chat_id: ChatId(CID_FABUSHI as i64),
                bot_user_id: UserId(UID_FABUSHI),
                state: "tool".to_string(),
                detail: Some("webmcp_call_tool".to_string()),
            }),
            cx,
        );
        assert_eq!(s.stage.read(cx).bot_activity_state, "tool");
        assert_eq!(
            s.stage.read(cx).bot_activity_detail.as_deref(),
            Some("webmcp_call_tool")
        );
        assert_eq!(
            s.sidebar
                .read(cx)
                .dialogs
                .iter()
                .find(|d| d.id == CID_FABUSHI)
                .and_then(|d| d.typing_indicator.as_deref()),
            Some("正在调用 webmcp_call_tool...")
        );

        let streaming_msg = make_proto_text_msg(
            8001,
            CID_FABUSHI,
            UID_FABUSHI,
            "🔧 **执行步骤 (1/2)**\n1. ⏳ `webmcp_call_tool` (`fabushi_send_dharma`)...",
            None,
        );
        s.handle_ws_server_frame(WsServerFrame::Update(UpdateEvent::NewMessage(streaming_msg)), cx);

        let final_text = "🔧 **执行步骤 (2/2 · 1 步失败)**\n1. ✅ `webmcp_discover` · 12ms\n2. ❌ `webmcp_call_tool` (`fabushi_send_dharma`) — 未注册的 WebMCP 工具\n\n⚠️ **小程序操作未完成**\n请先点击下方按钮打开或刷新「全球法布施」小程序后重试。";
        let edited_msg = make_proto_text_msg(
            8001,
            CID_FABUSHI,
            UID_FABUSHI,
            final_text,
            Some(vec![vec![ProtoInlineKeyboardButton {
                text: "打开全球法布施小程序".to_string(),
                callback_data: None,
                url: None,
                web_app_url: Some("gpui-miniapp://fabushi_bot/fabushi".to_string()),
            }]]),
        );
        s.handle_ws_server_frame(WsServerFrame::Update(UpdateEvent::EditMessage(edited_msg)), cx);
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::BotActivityChanged {
                chat_id: ChatId(CID_FABUSHI as i64),
                bot_user_id: UserId(UID_FABUSHI),
                state: "idle".to_string(),
                detail: None,
            }),
            cx,
        );

        let st = s.stage.read(cx);
        let last_msg = st.messages.last().expect("edited stage message");
        assert!(last_msg.bot_keyboard.is_some());
        match &last_msg.content {
            MessageContent::Text { text, is_edited } => {
                assert!(*is_edited, "[T1-F12-04] EditMessage must mark is_edited = true");
                let blocks = parse_rich_blocks(text, &[]);
                assert!(blocks.len() >= 3, "[T1-F12-04] step trace markdown must parse cleanly");
            }
            other => panic!("expected text message, got {other:?}"),
        }
        let preview = s
            .sidebar
            .read(cx)
            .dialogs
            .iter()
            .find(|d| d.id == CID_FABUSHI)
            .map(|d| d.last_message.clone())
            .unwrap_or_default();
        assert!(
            !preview.starts_with("🔧") && preview.contains("小程序操作未完成"),
            "[T1-F12-04] sidebar preview must strip step header: {preview:?}"
        );
        assert_eq!(st.bot_activity_state, "idle");
        assert!(
            !s.composer.read(cx).compose_send_disabled,
            "[T1-F12-04] composer must remain enabled after WebMCP tool error"
        );

        s.handle_stage_bot_button(
            8001,
            InlineButtonType::WebApp("gpui-miniapp://fabushi_bot/fabushi".to_string()),
            cx,
        );
        assert!(
            s.miniapps.miniapp_overlay.is_some() || s.miniapps.pending_miniapp_confirm.is_some(),
            "[T1-F12-04] clicking WebApp failure CTA must launch Mini App"
        );
    });

    // [T1-F12-05] Real BotAttachWebView::handle_bridge_event("webmcp_response", ok: false) -> AppShell drain -> WsClientFrame::MiniAppWebMcpResult
    let ov = shell
        .update(&mut cx, |s, cx| s.ensure_miniapp_overlay(cx))
        .unwrap();
    let _ = ov.update(&mut cx, |w, wcx| {
        w.handle_bridge_event(
            "webmcp_response",
            r#"{"request_id":"req_err","ok":false,"result":{},"error":"未注册的 WebMCP 工具: fabushi_send_dharma","tools":[{"name":"get_state","description":"State","input_schema":{},"read_only":true}],"state_summary":{"boot_error":"HTTP 404 app.wasm"},"control_mode":"ai_controlling"}"#,
        );
        assert!(w.active_webmcp_tool.is_none());
        assert_eq!(
            w.control_status_text,
            "操作提示: 未注册的 WebMCP 工具: fabushi_send_dharma"
        );
        assert!(w.webmcp_discovered_tools.iter().any(|t| t.name == "get_state"));
        assert_eq!(
            w.last_webmcp_state_summary
                .as_ref()
                .and_then(|v| v["boot_error"].as_str()),
            Some("HTTP 404 app.wasm")
        );
        wcx.notify();
    });
    let got_err_ws = pump_until(&app, 1500, |_| {
        while let Ok(frame) = client_rx.try_recv() {
            if let WsClientFrame::MiniAppWebMcpResult(resp) = frame {
                if resp.request_id == "req_err"
                    && !resp.ok
                    && resp.error.as_deref() == Some("未注册的 WebMCP 工具: fabushi_send_dharma")
                {
                    return true;
                }
            }
        }
        false
    })
    .await;
    assert!(
        got_err_ws,
        "[T1-F12-05] failed webmcp_response must propagate via AppShell drain to WsClientFrame::MiniAppWebMcpResult"
    );
}

// ============================================================================
// TIER 2: BOUNDARY & CORNER CASES (60 cases across Features 1–12)
// ============================================================================

#[test]
fn tier2_f01_profile_isolation_boundaries() {
    run_in_headless_subprocess("tier2_f01_profile_isolation_boundaries");
}

async fn t2_f01_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let mut mock_data = default_mock_server_data();
    // [T2-F01-01] Whitespace-only bio on Group
    if let Some(grp) = mock_data.peers.get_mut(&CID_GROUP) {
        grp.bio = Some("   \n\t  ".to_string());
    }
    // [T2-F01-02] Long 1500-char CJK+emoji bio on Fabushi
    if let Some(fab) = mock_data.peers.get_mut(&CID_FABUSHI) {
        fab.bio = Some("🪷法布施净心禅室·".repeat(150));
        fab.delay_ms = 300; // [T2-F01-03] Delayed response to test stale async overwrite guard
    }
    if let Some(prof) = mock_data.bot_profiles.get_mut("fabushi_bot") {
        prof.bot.about = "🪷法布施净心禅室·".repeat(150);
    }

    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, mock_data);
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T2-F01-03] Rapid ping-pong: trigger delayed Fabushi fetch, immediately switch Group -> Alice -> Grok
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
        s.sync_selected_chat_inspector_and_stage(CID_GROK, cx);
    });
    // Wait past Fabushi's 300ms delay
    app.background_executor()
        .timer(Duration::from_millis(500))
        .await;
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(
            insp.profile.username.to_ascii_lowercase().contains("grokai"),
            "[T2-F01-03] delayed Fabushi HTTP response overwrote active @GrokAI profile: {:?}",
            insp.profile.username
        );
        assert!(
            !insp.profile.bio.contains("法布施"),
            "[T2-F01-03] delayed Fabushi bio overwrote @GrokAI bio"
        );
    });

    // [T2-F01-01] & [T2-F01-02] Long bio on Fabushi -> whitespace-only bio on Group -> completely empty
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = pump_until(&app, 1500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.bio.contains("法布施"))
            .unwrap_or(false)
    })
    .await;
    // [T2-F01-04] Bio must not be duplicated inside facts
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        let dup_count = insp
            .profile
            .facts
            .iter()
            .filter(|(k, v)| k == "Bio" || k == "简介" || v == &insp.profile.bio)
            .count();
        assert_eq!(dup_count, 0, "[T2-F01-04] bio must not duplicate in profile.facts");
    });

    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
    });
    let _ = pump_until(&app, 1500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).members.is_group_or_channel)
            .unwrap_or(false)
    })
    .await;
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(
            insp.profile.bio.trim().is_empty(),
            "[T2-F01-01/02] whitespace-only group bio retained stale text: {:?}",
            insp.profile.bio
        );
    });

    // [T2-F01-05] Unknown chat_id in select_authoritative_dialog returns false and preserves current state
    let _ = shell.update(&mut cx, |s, cx| {
        let prev_name = s.inspector.read(cx).profile.name.clone();
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    assert!(!select_authoritative_dialog(sb, st, insp, 999_999));
                    assert_eq!(insp.profile.name, prev_name);
                });
            });
        });
    });
}

#[test]
fn tier2_f02_profile_branching_boundaries() {
    run_in_headless_subprocess("tier2_f02_profile_branching_boundaries");
}

fn t2_f02_body(cx: &mut App) {
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = setup_authenticated_shell(client, cx);
    let insp_ent = shell.read(cx).inspector.clone();

    // [T2-F02-01] Group with 0 members, empty invite_link, empty bio renders cleanly & computes Group layout
    insp_ent.update(cx, |insp, icx| {
        insp.clear_peer();
        insp.open();
        insp.set_peer("Empty Group", "", "EG", 0x3A7AFE);
        insp.sync_members(Vec::new(), String::new(), true);
        let _ = render_contact_profile_panel(
            &insp.profile,
            &insp.members,
            &insp.shared_media,
            &insp.collapsed_sections,
            false,
            false,
            None,
            TdesktopThemePalette::default(),
            "en",
            icx,
        );
        let empty_grp_layout = build_contact_profile_layout(
            &insp.profile,
            &insp.members,
            TdesktopThemePalette::default(),
            "en",
        );
        assert!(empty_grp_layout.is_group_or_channel);
        assert_eq!(empty_grp_layout.info_section_title, "Group Info");
        assert_eq!(empty_grp_layout.members_row_label.as_deref(), Some("View Members"));
        assert_eq!(empty_grp_layout.leave_row_label.as_deref(), Some("Leave Group"));
        assert!(!empty_grp_layout.show_secret_chat);
        assert!(!empty_grp_layout.show_block_contact);
        assert!(insp.members.is_group_or_channel);
        assert!(!insp.profile.is_bot);
    });

    // [T2-F02-02] Channel (CID_CHANNEL, chat_type = 3) branching
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    assert!(select_authoritative_dialog(sb, st, insp, CID_CHANNEL));
                    assert!(st.is_group_chat);
                    assert!(!st.is_bot_chat);
                });
            });
        });
    });

    // [T2-F02-03] format_bot_mau_subtitle boundary values (0 vs 1_250_000)
    assert_eq!(format_bot_mau_subtitle(0, "zh-CN"), "机器人");
    assert_eq!(format_bot_mau_subtitle(0, "en"), "bot");
    assert_eq!(format_bot_mau_subtitle(1_250_000, "en"), "1,250,000 monthly users");

    // [T2-F02-04] Group containing mixed AI Bot + human members preserves !profile.is_bot
    insp_ent.update(cx, |insp, _| {
        insp.sync_members(
            vec![
                MemberItem {
                    id: UID_VIEWER,
                    name: "Viewer".into(),
                    initials: "VW".into(),
                    color: 0x0A84FF,
                    role: "Owner".into(),
                    is_online: true,
                },
                MemberItem {
                    id: UID_FABUSHI,
                    name: "Fabushi".into(),
                    initials: "FA".into(),
                    color: 0x8F59F9,
                    role: "AI Bot".into(),
                    is_online: true,
                },
            ],
            "https://t.me/+mix".into(),
            true,
        );
        assert!(insp.members.is_group_or_channel);
        assert!(!insp.profile.is_bot);
    });

    // [T2-F02-05] Saved Messages detection
    let mut sb = ChatSidebar::new();
    let mut st = ConversationStage::new();
    let mut insp = ContextInspector::new();
    apply_authoritative_chat_list(
        &mut sb,
        &mut st,
        &mut insp,
        &[ChatSummary {
            chat_id: 777,
            title: "Saved Messages".to_string(),
            chat_type: 5,
            last_message: None,
            unread_count: 0,
            is_group: Some(false),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: None,
        }],
    );
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, 777));
    assert!(st.is_saved_messages);
    assert!(!st.is_bot_chat && !st.is_group_chat);
}

#[test]
fn tier2_f03_copy_feedback_timer_and_fence_boundaries() {
    run_in_headless_subprocess("tier2_f03_copy_feedback_timer_and_fence_boundaries");
}

async fn t2_f03_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();
    let insp_ent = shell.read_with(&mut cx, |s, _| s.inspector.clone()).unwrap();

    // [T2-F03-01] Generation-fenced multiple copies: Copy #1, wait 1.2s, Copy #2, wait 1.2s -> Copy #2 STILL visible
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        insp.open();
        insp.show_copied_feedback("已复制: first", icx);
    });
    app.background_executor()
        .timer(Duration::from_millis(1200))
        .await;
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        insp.show_copied_feedback("已复制: second", icx);
    });
    app.background_executor()
        .timer(Duration::from_millis(1200))
        .await;
    let still_second = insp_ent
        .read_with(&mut cx, |insp, _| {
            insp.copied_feedback.as_deref() == Some("已复制: second")
        })
        .unwrap_or(false);
    assert!(
        still_second,
        "[T2-F03-01] Copy #1 timer prematurely cleared Copy #2!"
    );
    // Wait remaining ~1.6s -> Copy #2 auto-dismisses
    let auto_done = pump_until(&app, 2200, |app| {
        insp_ent
            .read_with(app, |insp, _| insp.copied_feedback.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(auto_done, "[T2-F03-01] Copy #2 did not auto-dismiss after its full window");

    // [T2-F03-02] Rapid copy -> switch chat (<50ms) -> new chat copy is untouched by old timer
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        insp.show_copied_feedback("已复制: old_bot", icx);
    });
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
    });
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        assert!(insp.copied_feedback.is_none(), "[T2-F03-02] switch didn't clear");
        insp.show_copied_feedback("已复制: @alice", icx);
    });
    app.background_executor()
        .timer(Duration::from_millis(1200))
        .await;
    let alice_still = insp_ent
        .read_with(&mut cx, |insp, _| {
            insp.copied_feedback.as_deref() == Some("已复制: @alice")
        })
        .unwrap_or(false);
    assert!(alice_still, "[T2-F03-02] old chat timer cleared new chat's copy feedback");

    // [T2-F03-03] Long 250-char CJK+URL copy feedback clears cleanly
    let _ = insp_ent.update(&mut cx, |insp, icx| {
        insp.show_copied_feedback(format!("已复制: https://example.com/{}", "法".repeat(200)), icx);
    });
    let cleared_long = pump_until(&app, 3200, |app| {
        insp_ent
            .read_with(app, |insp, _| insp.copied_feedback.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(cleared_long, "[T2-F03-03] long copy feedback did not auto-dismiss");

    // [T2-F03-04] BotProfileBox::open resets stale copied_feedback
    let mut box_model = BotProfileBox::new(
        build_fabushi_bot_profile(Some(CID_FABUSHI as i64), true),
        TdesktopThemePalette::default(),
        "zh-CN",
    );
    box_model.copied_feedback = Some("已复制: old".to_string());
    box_model.open(build_grok_bot_profile(Some(CID_GROK as i64), true));
    assert!(box_model.copied_feedback.is_none());

    // [T2-F03-05] Re-opening inspector after close never resurrects stale feedback
    let _ = insp_ent.update(&mut cx, |insp, _| {
        insp.copied_feedback = Some("已复制: stale".to_string());
        insp.clear_peer();
        insp.open();
        assert!(insp.copied_feedback.is_none());
    });
}

#[test]
fn tier2_f04_stage_toast_isolation_boundaries() {
    run_in_headless_subprocess("tier2_f04_stage_toast_isolation_boundaries");
}

fn t2_f04_body(_cx: &mut App) {
    let mut sb = ChatSidebar::new();
    let mut st = ConversationStage::new();
    let mut insp = ContextInspector::new();
    let chats = vec![
        ChatSummary {
            chat_id: CID_FABUSHI,
            title: "Fabushi".into(),
            chat_type: 4,
            last_message: None,
            unread_count: 0,
            is_group: Some(false),
            is_bot: Some(true),
            avatar_color: None,
            updated_at: None,
        },
        ChatSummary {
            chat_id: CID_GROUP,
            title: "Group".into(),
            chat_type: 1,
            last_message: None,
            unread_count: 0,
            is_group: Some(true),
            is_bot: Some(false),
            avatar_color: None,
            updated_at: None,
        },
    ];
    apply_authoritative_chat_list(&mut sb, &mut st, &mut insp, &chats);

    // [T2-F04-01] Re-selecting SAME chat_id preserves messages
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, CID_FABUSHI));
    st.add_message(MessageItemModel {
        id: 1,
        sender_id: UID_FABUSHI,
        sender_name: "Fabushi".into(),
        avatar_initials: "FA".into(),
        avatar_color: 0x8F59F9,
        direction: MessageDirection::Incoming,
        timestamp: "10:00".into(),
        delivery_status: DeliveryStatus::Read,
        reply_to: None,
        reactions: Vec::new(),
        content: MessageContent::Text {
            text: "Keep me".into(),
            is_edited: false,
        },
        bot_keyboard: None,
        is_pinned: false,
    });
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, CID_FABUSHI));
    assert_eq!(st.messages.len(), 1);

    // [T2-F04-02] Rapid A -> B -> A preserves A's history without leaking B's toast_banner
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, CID_GROUP));
    st.toast_banner = Some("Group transient\nCJK 提示".to_string());
    st.copied_code_msg = Some(99);
    st.reaction_picker_msg_id = Some(99);
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, CID_FABUSHI));
    assert_eq!(st.messages.len(), 1);
    assert!(
        st.toast_banner.is_none(),
        "[T2-F04-02] toast_banner leaked back into A"
    );
    assert!(
        st.copied_code_msg.is_none(),
        "[T2-F04-02] copied_code_msg leaked across chat switch into A"
    );
    assert!(
        st.reaction_picker_msg_id.is_none(),
        "[T2-F04-02] reaction_picker_msg_id leaked across chat switch into A"
    );

    // [T2-F04-03] clear_selection() resets toast_banner, copied_code_msg, and reaction_picker_msg_id even when active chat becomes None
    st.toast_banner = Some("Another transient".to_string());
    st.copied_code_msg = Some(77);
    st.reaction_picker_msg_id = Some(88);
    st.clear_selection();
    assert!(st.active_chat_id.is_none(), "[T2-F04-03] active_chat_id must be None");
    assert!(st.toast_banner.is_none(), "[T2-F04-03] toast_banner must clear on clear_selection");
    assert!(st.copied_code_msg.is_none(), "[T2-F04-03] copied_code_msg must clear on clear_selection");
    assert!(
        st.reaction_picker_msg_id.is_none(),
        "[T2-F04-03] reaction_picker_msg_id must clear on clear_selection"
    );

    // [T2-F04-04] Reselecting B after clear_selection leaves copied_code_msg and reaction_picker_msg_id isolated and empty
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, CID_GROUP));
    assert!(st.copied_code_msg.is_none(), "[T2-F04-04] copied_code_msg must not resurrect");
    assert!(
        st.reaction_picker_msg_id.is_none(),
        "[T2-F04-04] reaction_picker_msg_id must not resurrect"
    );

    // [T2-F04-05] room_agent_states and toast_banner isolation across group switches
    assert!(st.room_agent_states.is_empty(), "[T2-F04-05] room_agent_states must be empty");
    assert!(st.toast_banner.is_none(), "[T2-F04-05] toast_banner must stay None");
}

#[test]
fn tier2_f05_grok_bot_edge_cases() {
    run_in_headless_subprocess("tier2_f05_grok_bot_edge_cases");
}

fn t2_f05_body(cx: &mut App) {
    // [T2-F05-01] Case-insensitive @grokai vs @GrokAI mention matching
    let cands = build_mention_candidates(
        true,
        &[ChatMemberItem {
            user_id: UID_GROK,
            role: "member".into(),
            admin_title: None,
            admin_rights: 0,
            permissions_bitmask: 0,
            joined_at_ms: 0,
        }],
        &[ContactDto {
            user_id: UID_GROK,
            first_name: "Grok".into(),
            last_name: None,
            username: Some("GrokAI".into()),
            phone: None,
            is_bot: true,
            is_mutual: false,
            chat_id: Some(CID_GROK as i64),
        }],
        &[],
        None,
    );
    assert!(cands.iter().any(|(row, _)| row.username.eq_ignore_ascii_case("grokai")));

    // [T2-F05-02] Empty bot commands hides slash autocomplete
    let mut comp = CanonicalComposer::new();
    comp.set_bot_slash_commands(Vec::new());
    comp.set_text("/");
    comp.sync_autocomplete();
    assert!(!comp.bot_ac_visible);

    // [T2-F05-03] Mixed InlineKeyboardButton types via hermes_keyboard_to_stage
    let mixed = hermes_keyboard_to_stage(&[vec![
        HermesInlineButtonDto {
            text: "CB".into(),
            callback_data: Some("a:b".into()),
            url: None,
            web_app_url: None,
        },
        HermesInlineButtonDto {
            text: "URL".into(),
            callback_data: None,
            url: Some("https://x.ai".into()),
            web_app_url: None,
        },
        HermesInlineButtonDto {
            text: "App".into(),
            callback_data: None,
            url: None,
            web_app_url: Some("/miniapps/grokai/app".into()),
        },
    ]])
    .expect("mixed keyboard");
    assert_eq!(mixed[0][0].button_type, InlineButtonType::Callback("a:b".into()));
    assert_eq!(mixed[0][1].button_type, InlineButtonType::Url("https://x.ai".into()));
    assert_eq!(
        mixed[0][2].button_type,
        InlineButtonType::WebApp("/miniapps/grokai/app".into())
    );

    // [T2-F05-04] Malformed JSON in decode_inline_keyboard_json returns None
    assert!(decode_inline_keyboard_json(&json!("not_an_array")).is_none());
    assert!(decode_inline_keyboard_json(&json!([])).is_none());

    // [T2-F05-05] Rapid Grok -> Group -> Fabushi keeps is_bot_chat & is_group_chat accurate
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = setup_authenticated_shell(client, cx);
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_GROK);
                    assert!(!st.is_group_chat);
                    select_authoritative_dialog(sb, st, insp, CID_GROUP);
                    assert!(st.is_group_chat);
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                    assert!(!st.is_group_chat);
                });
            });
        });
    });
}

#[test]
fn tier2_f06_miniapp_menu_button_layout_boundaries() {
    run_in_headless_subprocess("tier2_f06_miniapp_menu_button_layout_boundaries");
}

async fn t2_f06_body(app: AsyncApp) {
    let mut cx = app.clone();
    // [T2-F06-01] is_small shrink on typing and restore on empty
    let mut btn = HistoryViewBotMenuButtonViewControls::new();
    btn.set_bot_menu(UID_FABUSHI, "fabushi_bot", "全球法布施", "/miniapps/fabushi_bot/fabushi");
    let full_w = btn.width_px;
    assert!(btn.refresh("全球法布施", true));
    assert!(btn.is_small && btn.width_px == 36.0 && btn.width_px < full_w);
    assert!(btn.refresh("全球法布施", false));
    assert!(!btn.is_small && btn.width_px == full_w);

    // [T2-F06-02] Empty / whitespace menu_button_url keeps is_active = false
    let mut invalid_btn = HistoryViewBotMenuButtonViewControls::new();
    invalid_btn.set_bot_menu(UID_FABUSHI, "fabushi_bot", "Open", "   ");
    assert!(!invalid_btn.is_active);

    // [T2-F06-03] Empty label falls back to "Open App"
    let mut fallback_btn = HistoryViewBotMenuButtonViewControls::new();
    fallback_btn.set_bot_menu(UID_FABUSHI, "fabushi_bot", "  ", "/miniapps/x/y");
    assert_eq!(fallback_btn.text, "Open App");

    // [T2-F06-04] Narrow 800px / long 60-char label + typing shrink keeps MenuButton valid outside #cmp-input-wrap
    let mut rt = WindowSubsystemRuntime::new();
    rt.apply_viewport_width(800.0);
    assert!(rt.adaptive.is_normal() && !rt.adaptive.is_one_column());
    let long_label = "全球法布施超级长名称测试菜单按钮边界宽度检查".repeat(2);
    let comp_ent = cx
        .update(|cx| cx.new(|_| CanonicalComposer::new()))
        .unwrap();
    let _ = comp_ent.update(&mut cx, |c, ccx| {
        c.palette = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        c.sync_bot_menu_with_cx(
            UID_FABUSHI,
            "fabushi_bot".to_string(),
            long_label.clone(),
            "/miniapps/fabushi_bot/fabushi".to_string(),
            ccx,
        );
        assert!(c.bot_menu_controls.is_active && !c.bot_menu_controls.is_small);
        assert!(c.bot_menu_controls.width_px > 48.0);
        c.set_text("hello");
        c.sync_bot_menu_compact_state(Some(ccx));
        let menu_ent = c.ensure_bot_menu_entity(ccx);
        assert!(c.bot_menu_controls.is_small && c.bot_menu_controls.width_px == 36.0);
        assert!(menu_ent.read(ccx).is_small && menu_ent.read(ccx).width_px == 36.0);
        let _narrow_strip = render_text_input_strip_with_options(
            c.text(),
            c.ime_machine.cursor_index,
            None,
            None,
            true,
            false,
            false,
            false,
            false,
            c.calculate_content_height(),
            false,
            Some(menu_ent.into_any_element()),
            None,
            false,
            &c.palette,
            "zh-CN",
            ccx,
            |_this, _act: ComposerBarAction, _cx| {},
        );
    });

    // [T2-F06-05] MenuButtonCommands does not leak a Mini App URL
    let mut grok_btn = HistoryViewBotMenuButtonViewControls::new();
    grok_btn.set_bot_menu(UID_GROK, "GrokAI", "Menu", "");
    assert!(!grok_btn.is_active);
    let _ = app;
}

#[test]
fn tier2_f07_plus_compose_residual_state_boundaries() {
    run_in_headless_subprocess("tier2_f07_plus_compose_residual_state_boundaries");
}

async fn t2_f07_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    let _ = shell.update(&mut cx, |s, cx| {
        s.theme_state.palette =
            TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        s.sync_theme_and_locale_to_views(cx);
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        s.composer.update(cx, |c, _| {
            // [T2-F07-01] editing_message active before `+`
            c.begin_edit(10, "old edit text".into());
            // [T2-F07-02] send_as_options populated with non-empty SendAsPeer entries before `+`
            c.set_send_as_options(
                vec![
                    SendAsPeer::new(UID_VIEWER, "Viewer".to_string(), false),
                    SendAsPeer::new(999_001, "Channel Admin".to_string(), false),
                ],
                Some(999_001),
            );
            assert_eq!(c.send_as_options.len(), 2);
            assert_eq!(c.selected_send_as_peer_id, Some(999_001));
        });

        // Toggle + open and let GPUI's real observe_sidebar_entity fire
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
    });

    // [T2-F07-05] Pump real GPUI observer -> enter_compose_or_modal_isolation -> sync_compose_state_to_composer
    let isolated = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.composer.read(cx).is_compose_isolated
                    && s.composer.read(cx).compose_send_disabled
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        isolated,
        "[T2-F07-05] observe_sidebar_entity must isolate composer and set compose_send_disabled when 0 recipients selected"
    );

    let _ = shell.read_with(&mut cx, |s, cx| {
        let comp = s.composer.read(cx);
        assert!(
            comp.editing_message.is_none(),
            "[T2-F07-01] editing_message leaked into + compose"
        );
        assert!(
            comp.send_as_options.is_empty(),
            "[T2-F07-02] send_as_options leaked into + compose"
        );
        assert!(
            comp.selected_send_as_peer_id.is_none(),
            "[T2-F07-02] selected_send_as_peer_id leaked into + compose"
        );
        assert!(
            comp.compose_send_disabled,
            "[T2-F07-05] compose_send_disabled must be true when 0 recipients selected"
        );
        assert!(s.should_hide_composer(cx));
        assert!(s.is_compose_or_creation_modal_active(cx));
    });

    // [T2-F07-03] Rapid toggle open -> closed -> open -> closed creates zero phantom rows
    for _ in 0..3 {
        let _ = shell.update(&mut cx, |s, cx| {
            s.sidebar.update(cx, |sb, scx| {
                sb.exit_compose_mode();
                scx.notify();
            });
        });
        let exited = pump_until(&app, 1000, |app| {
            shell
                .read_with(app, |s, cx| !s.composer.read(cx).is_compose_isolated)
                .unwrap_or(false)
        })
        .await;
        assert!(exited, "[T2-F07-03] rapid exit_compose_mode must un-isolate");

        let _ = shell.update(&mut cx, |s, cx| {
            s.sidebar.update(cx, |sb, scx| {
                sb.toggle_plus_menu();
                scx.notify();
            });
        });
        let re_entered = pump_until(&app, 1000, |app| {
            shell
                .read_with(app, |s, cx| s.composer.read(cx).is_compose_isolated)
                .unwrap_or(false)
        })
        .await;
        assert!(re_entered, "[T2-F07-03] rapid toggle_plus_menu must re-isolate");
    }
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    let final_exit = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| !s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    assert!(final_exit, "[T2-F07-03] final exit_compose_mode must un-isolate");
    let _ = shell.read_with(&mut cx, |s, cx| {
        assert_eq!(s.sidebar.read(cx).dialogs.len(), 5);
        assert!(!s.sidebar.read(cx).dialogs.iter().any(|d| d.peer_name == "新信息"));
    });

    // [T2-F07-04] Enter + from Group chat -> exit -> Group composer stays without bot menu via observer
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_GROUP);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
    });
    let grp_plus = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    assert!(grp_plus, "[T2-F07-04] Group + compose must isolate");
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    let grp_restored = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| !s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    assert!(grp_restored, "[T2-F07-04] Group exit_compose_mode must un-isolate");
    let _ = shell.read_with(&mut cx, |s, cx| {
        assert!(!s.composer.read(cx).bot_menu_controls.is_active);
    });
}

#[test]
fn tier2_f08_start_bar_and_bot_keyboard_flag_boundaries() {
    run_in_headless_subprocess("tier2_f08_start_bar_and_bot_keyboard_flag_boundaries");
}

fn t2_f08_body(_cx: &mut App) {
    // [T2-F08-01] Without RESIZE -> maximize_size = true, height >= max_outer_height
    let mut kb = BotKeyboard::new();
    kb.set_markup(
        vec![vec![BotKeyboardButton::new("One", BotKeyboardButtonKind::Default)]],
        0,
        None,
    );
    assert!(kb.maximize_size);
    assert!(kb.calculated_height() >= kb.max_outer_height);

    // [T2-F08-02] 10 rows with RESIZE clamps to max_outer_height
    let ten_rows: Vec<Vec<BotKeyboardButton>> = (0..10)
        .map(|i| vec![BotKeyboardButton::new(format!("Btn {i}"), BotKeyboardButtonKind::Default)])
        .collect();
    kb.set_markup(ten_rows, kb_flags::RESIZE, None);
    assert!(!kb.maximize_size);
    assert_eq!(kb.calculated_height(), kb.max_outer_height);

    // [T2-F08-03] & [T2-F08-04] Switching away from unstarted bot clears prestart state
    let mut sb = ChatSidebar::new();
    let mut st = ConversationStage::new();
    let mut insp = ContextInspector::new();
    apply_authoritative_chat_list(
        &mut sb,
        &mut st,
        &mut insp,
        &[
            ChatSummary {
                chat_id: CID_GROK,
                title: "Grok".into(),
                chat_type: 4,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(true),
                avatar_color: None,
                updated_at: None,
            },
            ChatSummary {
                chat_id: CID_ALICE,
                title: "Alice".into(),
                chat_type: 0,
                last_message: None,
                unread_count: 0,
                is_group: Some(false),
                is_bot: Some(false),
                avatar_color: None,
                updated_at: None,
            },
        ],
    );
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, CID_GROK));
    st.set_bot_prestart_state(Some("Intro".into()), true, false);
    st.pending_bot_start = true;
    assert!(select_authoritative_dialog(&mut sb, &mut st, &mut insp, CID_ALICE));
    assert!(!st.show_bot_start_bar);
    assert!(st.bot_description_card.is_none());
    assert!(!st.pending_bot_start);

    // [T2-F08-05] Mixed BotKeyboardButtonKind variants
    kb.set_markup(
        vec![vec![
            BotKeyboardButton::new("Phone", BotKeyboardButtonKind::RequestPhone),
            BotKeyboardButton::new("Loc", BotKeyboardButtonKind::RequestLocation),
            BotKeyboardButton::new("CB", BotKeyboardButtonKind::Callback(b"go".to_vec())),
            BotKeyboardButton::new("URL", BotKeyboardButtonKind::Url("https://t.me".into())),
        ]],
        kb_flags::RESIZE | kb_flags::PERSISTENT,
        None,
    );
    assert!(kb.persistent);
    assert_eq!(kb.rows[0].len(), 4);
}

#[test]
fn tier2_f09_window_toast_wrapping_and_fence_boundaries() {
    run_in_headless_subprocess("tier2_f09_window_toast_wrapping_and_fence_boundaries");
}

async fn t2_f09_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T2-F09-01] 300-char PROVIDER_ERROR at 800px window width still does not overlap composer
    let long_err = format!(
        "PROVIDER_ERROR: {}",
        "Missing active AI provider configuration for this account; open Settings -> AI Providers. "
            .repeat(4)
    );
    let _ = shell.update(&mut cx, |s, cx| {
        s.window_runtime.apply_viewport_width(800.0);
        assert!(
            s.window_runtime.adaptive.is_normal()
                && (s.window_runtime.adaptive.current_width - 800.0).abs() < f32::EPSILON
        );
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.composer.update(cx, |c, _| c.set_text("Line 1\nLine 2\nLine 3"));
        s.show_chat_toast(CID_FABUSHI, long_err.clone(), 5000, cx);
    });
    let _ = shell.update(&mut cx, |s, cx| {
        assert_eq!(s.composer.read(cx).calculate_line_count(), 3);
        assert_eq!(s.composer.read(cx).calculate_content_height(), px(84.0));
        assert!(!s.should_hide_composer(cx));
        let toast = s
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .expect("[T2-F09-01] 300-char PROVIDER_ERROR banner should be active");
        assert_eq!(toast.text, long_err);
        assert_eq!(toast.chat_id, Some(CID_FABUSHI));
        assert_eq!(toast.duration_ms, 5000);
        assert_eq!(s.scheduled_toast_id, Some(toast.id));
        s.window_runtime.controller.dismiss_current_toast();
        s.schedule_current_toast_dismiss(cx);
        assert!(s.window_runtime.controller.current_toast.is_none());
        assert_eq!(s.composer.read(cx).text(), "Line 1\nLine 2\nLine 3");
    });

    // [T2-F09-02] Generation/ID-fenced timer: Toast #1 dismissed manually -> Toast #2 not killed by #1's timer
    let _ = shell.update(&mut cx, |s, cx| {
        s.window_runtime.controller.dismiss_current_toast();
        s.show_toast("Toast #1", 600, cx);
    });
    app.background_executor()
        .timer(Duration::from_millis(150))
        .await;
    let _ = shell.update(&mut cx, |s, cx| {
        s.window_runtime.controller.dismiss_current_toast();
        s.show_toast("Toast #2", 1800, cx);
    });
    app.background_executor()
        .timer(Duration::from_millis(650))
        .await;
    let _ = shell.read_with(&mut cx, |s, _| {
        assert_eq!(
            s.window_runtime
                .controller
                .current_toast
                .as_ref()
                .map(|t| t.text.as_str()),
            Some("Toast #2"),
            "[T2-F09-02] Toast #1's stale timer dismissed Toast #2!"
        );
    });

    // [T2-F09-03] duration_ms == 0 normalizes to > 0 (3000ms)
    let _ = shell.update(&mut cx, |s, _| {
        s.window_runtime.controller.dismiss_current_toast();
        s.window_runtime.enqueue_toast("Zero dur", 0);
        let dur = s
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .map(|t| t.duration_ms)
            .unwrap_or(0);
        assert_eq!(dur, 3000, "[T2-F09-03] 0ms toast must normalize to 3000ms");
    });

    // [T2-F09-04] & [T2-F09-05] Queue advance + Unicode/newline text
    let _ = shell.update(&mut cx, |s, _| {
        s.window_runtime.enqueue_toast("队列消息 二\n包含特殊字符 <>&", 2000);
        s.window_runtime.controller.dismiss_current_toast();
        assert!(s
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .is_some_and(|t| t.text.contains("队列消息")));
    });
}

#[test]
fn tier2_f10_local_fabushi_provider_isolation_boundaries() {
    run_in_headless_subprocess("tier2_f10_local_fabushi_provider_isolation_boundaries");
}

async fn t2_f10_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();

    // [T2-F10-01] Empty slice -> 0 hydrated, not usable
    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("hermes-local-v1").unwrap());
    assert_eq!(hydrate_local_subscriptions_into_fabushi(&host, &[]), 0);
    assert!(!has_usable_local_fabushi_provider(&host));

    // [T2-F10-02] Placeholder-only ProviderRouterState in AiOauthVaultBundle is NOT usable
    let placeholder_state = ProviderRouter::new().snapshot();
    let bundle = AiOauthVaultBundle {
        providers_json: serde_json::to_value(&placeholder_state).unwrap(),
        subscriptions_json: serde_json::Value::Null,
        updated_at_ms: 1_700_000_000_000,
    };
    let n = hydrate_ai_oauth_vault_bundle_into_fabushi(&host, &bundle).unwrap();
    assert_eq!(n, 0);
    assert!(!has_usable_local_fabushi_provider(&host));

    // [T2-F10-03] User 1 per-user hydration via wrapper does not leak to User 2
    assert_eq!(
        hydrate_local_subscriptions_into_fabushi_for_user(
            &host,
            &[sample_vertex_item()],
            Some("100001")
        ),
        1
    );
    assert!(has_usable_local_fabushi_provider_for_user(&host, Some("100001")));
    assert!(!has_usable_local_fabushi_provider_for_user(&host, Some("200002")));

    // [T2-F10-04] resolve_fabushi_bot_target excludes Group, Channel, BotFather
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();
    let _ = shell.update(&mut cx, |s, cx| {
        assert!(s.resolve_fabushi_bot_target(CID_GROUP, cx).is_none());
        assert!(s.resolve_fabushi_bot_target(CID_CHANNEL, cx).is_none());
        assert!(s.resolve_fabushi_bot_target(CID_FABUSHI, cx).is_some());
        s.session_sync
            .chat_peer_user_ids
            .insert(9001, BOTFATHER_USER_ID as u64);
        assert!(s.resolve_fabushi_bot_target(9001, cx).is_none());
    });

    // [T2-F10-05] Error turn resets bot_activity_state to "idle" & clears sidebar typing
    let _ = shell.update(&mut cx, |s, cx| {
        s.attach_local_fabushi_host(host);
        let target = s.resolve_fabushi_bot_target(CID_FABUSHI, cx).unwrap();
        s.spawn_local_fabushi_turn(CID_FABUSHI, target, None, "test".into(), cx);
    });
    let idle_restored = pump_until(&app, 4000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.stage.read(cx).bot_activity_state == "idle"
                    && s.sidebar
                        .read(cx)
                        .dialogs
                        .iter()
                        .find(|d| d.id == CID_FABUSHI)
                        .is_some_and(|d| d.typing_indicator.is_none())
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        idle_restored,
        "[T2-F10-05] spawn_local_fabushi_turn error must reset typing indicator and bot_activity_state"
    );
}

#[test]
fn tier2_f11_webmcp_bridge_and_popup_boundaries() {
    run_in_headless_subprocess("tier2_f11_webmcp_bridge_and_popup_boundaries");
}

fn t2_f11_body(_cx: &mut App) {
    let mut wv = BotAttachWebView::new();
    // [T2-F11-01] Empty string state_update ignored
    wv.last_webmcp_state_summary = Some(json!({"step": 1}));
    wv.handle_bridge_event("webmcp_state_update", r#"{"state_summary":"   "}"#);
    assert_eq!(wv.last_webmcp_state_summary, Some(json!({"step": 1})));

    // [T2-F11-02] FIFO queue preserves multiple commands until ready; internal discover suppresses WebMcpResponseReady
    for i in 1..=3 {
        wv.pending_webmcp_queue.push_back(MiniAppWebMcpCommand {
            request_id: format!("q_{i}"),
            bot_username: "fabushi_bot".into(),
            short_name: "fabushi".into(),
            chat_id: CID_FABUSHI as i64,
            bot_user_id: UID_FABUSHI as i64,
            app_title: "全球法布施".into(),
            operation: "call_tool".into(),
            tool_name: Some("get_state".into()),
            arguments: json!({}),
            status_message: None,
        });
    }
    assert_eq!(wv.pending_webmcp_queue.len(), 3);
    wv.handle_bridge_event("web_app_ready", "{}");
    wv.handle_bridge_event("__gpui_webmcp_page_ready", r#"{"host_wrapper_only":true}"#);
    assert!(!wv.webmcp_page_ready && wv.pending_webmcp_queue.len() == 3);
    wv.handle_bridge_event("iframe_ready", "{}");
    assert!(wv.webmcp_page_ready && wv.webmcp_iframe_ready && wv.pending_webmcp_queue.is_empty());
    let _ = wv.take_pending_actions();
    wv.handle_bridge_event(
        "webmcp_response",
        r#"{"request_id":"__gpui_internal_discover_1","ok":true,"result":{},"tools":[{"name":"fabushi_send_dharma","description":"Send","input_schema":{},"read_only":false}],"state_summary":{"ready":true},"control_mode":"ai_controlling"}"#,
    );
    assert!(wv.webmcp_discovered_tools.iter().any(|t| t.name == "fabushi_send_dharma"));
    assert!(
        wv.take_pending_actions().is_empty(),
        "[T2-F11-02] __gpui_internal_discover_* response must not emit WebMcpResponseReady"
    );

    // [T2-F11-03] web_app_open_link rejects javascript: scheme, accepts https://
    wv.handle_bridge_event("web_app_open_link", r#"{"url":"javascript:alert(1)"}"#);
    assert!(wv.take_pending_actions().is_empty());
    wv.handle_bridge_event("web_app_open_link", r#"{"url":"https://fabushi.example.org"}"#);
    assert_eq!(wv.take_pending_actions().len(), 1);

    // [T2-F11-04] Empty buttons array injects default OK button
    wv.handle_bridge_event(
        "web_app_open_popup",
        r#"{"title":"Alert","message":"Msg","buttons":[]}"#,
    );
    assert_eq!(wv.custom_popup.as_ref().unwrap().buttons.len(), 1);
    assert_eq!(wv.custom_popup.as_ref().unwrap().buttons[0].text, "OK");

    // [T2-F11-05] Write access prompt when allows_write_to_pm == false
    wv.allows_write_to_pm = false;
    wv.handle_bridge_event("web_app_request_write_access", "{}");
    assert!(wv.write_access_prompt_open);
}

#[test]
fn tier2_f12_webmcp_failure_guard_boundaries() {
    run_in_headless_subprocess("tier2_f12_webmcp_failure_guard_boundaries");
}

async fn t2_f12_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, srv_state) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T2-F12-01] Long 500-char JSON step trace + CJK payload truncation in sidebar preview & rich block parsing
    let long_step_json = "{\"sutra\":\"金刚经\",\"payload\":\"".to_string() + &"禅".repeat(480) + "\"}";
    let step_trace_msg = format!(
        "🔧 **执行步骤 (2/2 · 1 步失败)**\n1. ❌ `webmcp_call_tool` (`fabushi_send_dharma`) — `{long_step_json}`\n\n⚠️ **小程序操作未完成**：未注册工具 fabushi_send_dharma，请刷新小程序。{}",
        "详情".repeat(120)
    );
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        let msg1 = make_proto_text_msg(
            9100,
            CID_FABUSHI,
            UID_FABUSHI,
            "🔧 **执行步骤 (1/2)**\n1. ⏳ `webmcp_call_tool`...",
            None,
        );
        s.handle_ws_server_frame(WsServerFrame::Update(UpdateEvent::NewMessage(msg1)), cx);
        let msg2 = make_proto_text_msg(9100, CID_FABUSHI, UID_FABUSHI, &step_trace_msg, None);
        s.handle_ws_server_frame(WsServerFrame::Update(UpdateEvent::EditMessage(msg2)), cx);

        let preview = s
            .sidebar
            .read(cx)
            .dialogs
            .iter()
            .find(|d| d.id == CID_FABUSHI)
            .map(|d| d.last_message.clone())
            .unwrap_or_default();
        assert!(
            !preview.starts_with("🔧") && preview.chars().count() <= 163,
            "[T2-F12-01] sidebar preview must strip step header & truncate to <=163 chars: len={}",
            preview.chars().count()
        );
        let blocks = parse_rich_blocks(&step_trace_msg, &[]);
        assert!(!blocks.is_empty(), "[T2-F12-01] long step trace must parse cleanly");
    });

    // [T2-F12-02] & [T2-F12-03] Repeated local turn errors with CJK & HTML meta-chars keep stage/composer responsive
    let _ = shell.update(&mut cx, |s, cx| {
        let before_count = s.stage.read(cx).messages.len();
        for i in 1..=3 {
            s.apply_local_fabushi_turn_error_to_shell(
                CID_FABUSHI,
                UID_FABUSHI as i64,
                "全球法布施",
                &format!("❌ 错误 #{i}: <script>未注册工具</script> \"fabushi_send_dharma\"\nLine2"),
                cx,
            );
            let toast_txt = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .map(|t| t.text.clone())
                .unwrap_or_default();
            assert!(toast_txt.chars().count() <= 80 && !toast_txt.contains('\n'));
            s.window_runtime.controller.dismiss_current_toast();
        }
        assert_eq!(s.stage.read(cx).messages.len(), before_count + 3);
        assert_eq!(s.stage.read(cx).bot_activity_state, "idle");
        assert!(s
            .sidebar
            .read(cx)
            .dialogs
            .iter()
            .find(|d| d.id == CID_FABUSHI)
            .is_some_and(|d| d.typing_indicator.is_none()));
        s.composer.update(cx, |c, ccx| {
            assert!(!c.compose_send_disabled);
            c.ime_machine.insert_text("retry");
            assert!(c.text().ends_with("retry"));
            ccx.notify();
        });
    });

    // [T2-F12-04] Closed WebView refuses synthetic flush on macOS & AppShell handles launch failure cleanly
    let mut wv = BotAttachWebView::new();
    wv.launch(
        AttachBotEntry {
            bot_id: UID_FABUSHI,
            name: "全球法布施".into(),
            short_name: "fabushi".into(),
            allowed_peer_types: peer_types::USER | peer_types::BOT,
            in_main_menu: true,
            in_attach_menu: true,
            disclaimer_required: false,
            request_write_access: false,
        },
        "/miniapps/fabushi_bot/fabushi",
    );
    wv.handle_bridge_event("__gpui_webmcp_page_ready", "{}");
    assert!(wv.webmcp_page_ready);
    wv.close();
    assert!(
        !wv.is_open
            && !wv.is_floating_pip
            && !wv.webmcp_page_ready
            && !wv.webmcp_iframe_ready
            && wv.webmcp_discovered_tools.is_empty()
            && wv.pending_webmcp_queue.is_empty()
            && wv.control_mode == MiniAppControlMode::Idle
    );

    let closed_cmd = MiniAppWebMcpCommand {
        request_id: "closed_1".into(),
        bot_username: "fabushi_bot".into(),
        short_name: "fabushi".into(),
        chat_id: CID_FABUSHI as i64,
        bot_user_id: UID_FABUSHI as i64,
        app_title: "全球法布施".into(),
        operation: "call_tool".into(),
        tool_name: Some("fabushi_send_dharma".into()),
        arguments: json!({"amount": 1}),
        status_message: None,
    };
    #[cfg(target_os = "macos")]
    {
        let _guard = RestoreEnv::temporarily_unset("GPUI_OFFLINE_TEST");
        wv.dispatch_webmcp_command(closed_cmd.clone());
        assert!(!wv.webmcp_page_ready && !wv.has_native_webview());
        assert_eq!(
            wv.pending_webmcp_queue.len(),
            1,
            "[T2-F12-04] closed WebView must queue instead of synthesizing ok:true when offline fallback disabled"
        );
        assert!(
            wv.take_pending_actions().is_empty(),
            "[T2-F12-04] closed WebView must not emit WebMcpResponseReady"
        );
        wv.close();
        assert!(wv.pending_webmcp_queue.is_empty());
        assert_eq!(wv.control_mode, MiniAppControlMode::Idle);
    }
    #[cfg(not(target_os = "macos"))]
    {
        wv.dispatch_webmcp_command(closed_cmd);
        assert!(!wv.is_open);
        assert_eq!(wv.control_mode, MiniAppControlMode::AiControlling);
        assert_eq!(wv.take_pending_actions().len(), 1);
    }

    // AppShell launch-failure path when closed overlay gets MiniAppWebMcpCommand and /launch returns 404
    srv_state.lock().unwrap().miniapp_launch_fail = true;
    let _ = shell.update(&mut cx, |s, cx| {
        if let Some(ov) = s.miniapps.miniapp_overlay.clone() {
            ov.update(cx, |w, wcx| {
                w.handle_bridge_event("web_app_close", "{}");
                wcx.notify();
            });
        }
    });
    let overlay_closed = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, _| s.miniapps.miniapp_overlay.is_none())
            .unwrap_or(false)
    })
    .await;
    assert!(overlay_closed);

    let _ = shell.update(&mut cx, |s, cx| {
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::MiniAppWebMcpCommand(MiniAppWebMcpCommand {
                request_id: "closed_shell_404".into(),
                bot_username: "fabushi_bot".into(),
                short_name: "fabushi".into(),
                chat_id: CID_FABUSHI as i64,
                bot_user_id: UID_FABUSHI as i64,
                app_title: "全球法布施".into(),
                operation: "call_tool".into(),
                tool_name: Some("fabushi_send_dharma".into()),
                arguments: json!({}),
                status_message: None,
            })),
            cx,
        );
    });
    let launch_err_shown = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, _| {
                s.miniapps.miniapp_overlay.is_none()
                    && s.window_runtime
                        .controller
                        .current_toast
                        .as_ref()
                        .is_some_and(|t| t.text.contains("Mini App launch failed"))
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        launch_err_shown,
        "[T2-F12-04] closed overlay + 404 /launch must close overlay and show Mini App launch failed toast"
    );

    // [T2-F12-05] Inline callback recovery after WebMCP error navigates cleanly
    let _ = shell.update(&mut cx, |s, cx| {
        s.execute_hermes_inline_callback("hermes:open_ai_providers".into(), cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
    });
}

// ============================================================================
// TIER 3: CROSS-FEATURE PAIRWISE COMBINATIONS (12 cases)
// ============================================================================

#[test]
fn tier3_pairwise_miniapp_slash_typing_and_plus_compose() {
    run_in_headless_subprocess("tier3_pairwise_miniapp_slash_typing_and_plus_compose");
}

async fn t3_pair_a_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T3-01] F6 + F8: Bot Mini App MenuButton + / slash commands + typing is_small shrink
    let _ = shell.update(&mut cx, |s, cx| {
        s.theme_state.palette =
            TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        s.sync_theme_and_locale_to_views(cx);
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        s.composer.update(cx, |c, ccx| {
            assert!(c.bot_menu_controls.is_active);
            assert!(!c.bot_slash_commands.is_empty());
            c.set_text("/");
            c.ime_machine.cursor_index = 1;
            c.sync_autocomplete();
            c.sync_bot_menu_compact_state(Some(ccx));
            assert!(c.bot_ac_visible, "[T3-01] / autocomplete must activate with MenuButton");
            assert!(
                c.bot_menu_controls.is_small,
                "[T3-01] typing '/' must automatically shrink MenuButton to compact is_small mode"
            );
            c.clear();
            c.sync_bot_menu_compact_state(Some(ccx));
            assert!(
                !c.bot_menu_controls.is_small,
                "[T3-01] c.clear() must restore MenuButton from is_small"
            );
            assert!(
                c.bot_menu_controls.width_px > 36.0,
                "[T3-01] c.clear() must restore expanded MenuButton width_px"
            );
            assert!(!c.ensure_bot_menu_entity(ccx).read(ccx).is_small);
        });
    });

    // [T3-02] F6 + F7: Active Mini App + `+` compose -> cancel -> restore @fabushi_bot MenuButton & slash
    let _ = shell.update(&mut cx, |s, cx| {
        s.composer.update(cx, |c, _| c.set_text("unfinished message"));
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
        cx.notify();
    });
    let t3_isolated = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    assert!(t3_isolated, "[T3-02] + compose must isolate composer");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let comp = s.composer.read(cx);
        assert!(
            !comp.bot_menu_controls.is_active && comp.text().is_empty(),
            "[T3-02] + compose must clear bot menu & draft"
        );
    });
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    let t3_restored = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| {
                !s.composer.read(cx).is_compose_isolated
                    && s.composer.read(cx).bot_menu_controls.is_active
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        t3_restored,
        "[T3-02] cancel + compose must restore @fabushi_bot menu & slash commands via observer"
    );
    let _ = shell.read_with(&mut cx, |s, cx| {
        let comp = s.composer.read(cx);
        assert!(comp.bot_menu_controls.is_active && !comp.bot_slash_commands.is_empty());
    });

    // [T3-03] F7 + F8: Unstarted bot START bar + `+` compose -> cancel -> START bar preserved cleanly
    let _ = shell.update(&mut cx, |s, cx| {
        if let Some(p) = s.miniapps.bot_profiles_cache.get_mut("grokai") {
            p.has_started_chat = false;
        }
        s.sync_selected_chat_inspector_and_stage(CID_GROK, cx);
        s.stage.update(cx, |st, _| {
            st.messages.clear();
            st.set_bot_prestart_state(Some("Intro".into()), true, false);
        });
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
        assert!(s.sidebar.read(cx).is_apple_compose_active());
    });
    let _ = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    let _ = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| !s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    let _ = shell.read_with(&mut cx, |s, cx| {
        assert!(s.stage.read(cx).show_bot_start_bar);
        assert!(s.should_hide_composer(cx));
    });
    let _ = app;
}

#[test]
fn tier3_pairwise_copy_toast_inspector_and_chat_switch() {
    run_in_headless_subprocess("tier3_pairwise_copy_toast_inspector_and_chat_switch");
}

async fn t3_pair_b_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T3-04] F1 + F3: Inspector open + copy bio/username -> immediate switch Bot -> Group clears BOTH bio & copied_feedback
    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| insp.open());
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.is_bot)
            .unwrap_or(false)
    })
    .await;
    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| {
            insp.copied_feedback = Some("已复制: @fabushi_bot".to_string());
        });
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
        let insp = s.inspector.read(cx);
        assert!(insp.copied_feedback.is_none(), "[T3-04] copied_feedback leaked");
        assert!(insp.profile.bio.is_empty(), "[T3-04] bio leaked");
    });

    // [T3-05] F2 + F4: Group branching + Stage toast_banner isolation on switch
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).members.is_group_or_channel)
            .unwrap_or(false)
    })
    .await;
    let _ = shell.update(&mut cx, |s, cx| {
        s.stage.update(cx, |st, _| {
            st.toast_banner = Some("Group link copied".into());
        });
        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
        assert!(
            s.stage.read(cx).toast_banner.is_none(),
            "[T3-05] stage.toast_banner leaked from group to 1v1"
        );
    });

    // [T3-06] F1 + F2 + F3 + F4: Rapid Bot -> Group -> 1v1 -> Bot with active copy & stage banners
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        s.inspector.update(cx, |insp, _| {
            insp.copied_feedback = Some("已复制: fabushi".into());
        });
        s.stage.update(cx, |st, _| {
            st.toast_banner = Some("Stage banner".into());
        });
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
        s.sync_selected_chat_inspector_and_stage(CID_GROK, cx);
        assert!(s.inspector.read(cx).copied_feedback.is_none());
        assert!(s.stage.read(cx).toast_banner.is_none());
    });
}

#[test]
fn tier3_pairwise_provider_error_toast_and_menu_hit_test() {
    run_in_headless_subprocess("tier3_pairwise_provider_error_toast_and_menu_hit_test");
}

async fn t3_pair_c_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T3-07] F6 + F9: PROVIDER_ERROR toast + active MenuButton hit-test & launch dispatch non-occlusion
    let menu_ent = shell
        .update(&mut cx, |s, cx| {
            s.theme_state.palette =
                TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
            s.sync_theme_and_locale_to_views(cx);
            s.sidebar.update(cx, |sb, sb_cx| {
                s.stage.update(sb_cx, |st, st_cx| {
                    s.inspector.update(st_cx, |insp, _| {
                        select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                    });
                });
            });
            s.sync_composer_and_inspector_miniapp_state(cx);
            let menu_ent = s.composer.update(cx, |c, ccx| {
                assert!(c.bot_menu_controls.is_active && !c.bot_menu_controls.is_small);
                c.set_text("Line 1\nLine 2\nLine 3");
                c.sync_bot_menu_compact_state(Some(ccx));
                let menu_ent = c.ensure_bot_menu_entity(ccx);
                assert_eq!(c.calculate_line_count(), 3);
                assert_eq!(c.calculate_content_height(), px(84.0));
                assert!(c.bot_menu_controls.is_small && c.bot_menu_controls.width_px == 36.0);
                assert!(menu_ent.read(ccx).is_small);
                let _strip = render_text_input_strip_with_options(
                    c.text(),
                    c.ime_machine.cursor_index,
                    None,
                    None,
                    true,
                    false,
                    false,
                    false,
                    false,
                    c.calculate_content_height(),
                    c.is_overflow_scroll_active(),
                    Some(menu_ent.clone().into_any_element()),
                    None,
                    c.compose_send_disabled,
                    &c.palette,
                    "zh-CN",
                    ccx,
                    |_this, _act: ComposerBarAction, _cx| {},
                );
                ccx.notify();
                menu_ent
            });
            s.show_chat_toast(
                CID_FABUSHI,
                "PROVIDER_ERROR: Please configure AI provider before sending.",
                4000,
                cx,
            );
            assert!(s.window_runtime.controller.current_toast.is_some());
            assert!(s.scheduled_toast_id.is_some());
            menu_ent
        })
        .unwrap();

    // Clicking external MenuButton while PROVIDER_ERROR toast is active must still dispatch Mini App launch
    let _ = menu_ent.update(&mut cx, |m, mcx| {
        m.pending_launch = Some((
            UID_FABUSHI,
            "fabushi_bot".to_string(),
            "全球法布施".to_string(),
            "/miniapps/fabushi_bot/fabushi".to_string(),
        ));
        mcx.notify();
    });
    let menu_launched = pump_until(&app, 1500, |app| {
        shell
            .read_with(app, |s, _| {
                s.miniapps.miniapp_overlay.is_some() || s.miniapps.pending_miniapp_confirm.is_some()
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        menu_launched,
        "[T3-07] MenuButton launch must dispatch cleanly while PROVIDER_ERROR toast is active"
    );
    let _ = shell.update(&mut cx, |s, cx| {
        s.window_runtime.controller.dismiss_current_toast();
        s.schedule_current_toast_dismiss(cx);
        assert!(s.window_runtime.controller.current_toast.is_none());
        assert!(s.composer.read(cx).bot_menu_controls.is_active);
        assert_eq!(s.composer.read(cx).text(), "Line 1\nLine 2\nLine 3");
    });

    // [T3-08] F9 + F10: Local Fabushi PROVIDER_ERROR inline card + hermes:open_ai_providers navigation
    let empty_host = Arc::new(FabushiBotHost::new_unified_ephemeral("hermes-local-v1").unwrap());
    empty_host.core_engine().providers().set_allow_local_fallback(false);
    empty_host.core_engine().providers().set_offline_rule_bot_allowed(false);
    empty_host.sync_all_providers();
    let _ = shell.update(&mut cx, |s, cx| {
        s.window_runtime.controller.dismiss_current_toast();
        s.attach_local_fabushi_host(empty_host);
        let target = s.resolve_fabushi_bot_target(CID_FABUSHI, cx).unwrap();
        s.spawn_local_fabushi_turn(CID_FABUSHI, target, None, "Hello".into(), cx);
    });
    let got_card = pump_until(&app, 4000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.stage.read(cx).messages.iter().any(|m| {
                    m.bot_keyboard.as_ref().is_some_and(|rows| {
                        rows.iter().flatten().any(|b| {
                            b.button_type
                                == InlineButtonType::Callback("hermes:open_ai_providers".into())
                        })
                    })
                })
            })
            .unwrap_or(false)
    })
    .await;
    assert!(got_card, "[T3-08] inline PROVIDER_ERROR CTA card missing");
    let _ = shell.update(&mut cx, |s, cx| {
        s.execute_hermes_inline_callback("hermes:open_ai_providers".into(), cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert_eq!(
            s.settings_router.read(cx).settings_section,
            SettingsSectionType::AiProviders
        );
    });

    // [T3-09] F5 + F8: @GrokAI START bar -> /start -> BotKeyboard + spec parity
    let _ = shell.update(&mut cx, |s, cx| {
        s.active_panel = ActiveWorkspacePanel::Chats;
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_GROK);
                    st.set_bot_prestart_state(Some("What can Grok do?".into()), true, false);
                });
            });
        });
        assert!(s.stage.read(cx).show_bot_start_bar);
        s.stage.update(cx, |st, _| {
            st.set_bot_prestart_state(None, false, false);
        });
        assert!(!s.stage.read(cx).show_bot_start_bar);
    });
    let mut kb = BotKeyboard::new();
    kb.set_markup(
        vec![vec![BotKeyboardButton::new("/help", BotKeyboardButtonKind::Default)]],
        kb_flags::RESIZE | kb_flags::SINGLE_USE,
        None,
    );
    assert!(kb.has_markup() && kb.single_use);
}

#[test]
fn tier3_pairwise_webmcp_miniapp_and_failure_guard() {
    run_in_headless_subprocess("tier3_pairwise_webmcp_miniapp_and_failure_guard");
}

async fn t3_pair_d_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let (client_tx, mut client_rx) = tokio::sync::mpsc::unbounded_channel::<WsClientFrame>();
    let (_server_tx, server_rx) = tokio::sync::mpsc::unbounded_channel::<WsServerFrame>();
    client.ws_channel.attach_channels(client_tx, server_rx);
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T3-10] F6 + F11: External MenuButton -> BotAttachWebView WebMCP readiness & flush
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        assert!(s.composer.read(cx).bot_menu_controls.is_active);
    });
    let mut wv = BotAttachWebView::new();
    wv.launch(
        AttachBotEntry {
            bot_id: UID_FABUSHI,
            name: "全球法布施".into(),
            short_name: "fabushi".into(),
            allowed_peer_types: peer_types::USER | peer_types::BOT,
            in_main_menu: true,
            in_attach_menu: true,
            disclaimer_required: false,
            request_write_access: false,
        },
        "/miniapps/fabushi_bot/fabushi",
    );
    wv.pending_webmcp_queue.push_back(MiniAppWebMcpCommand {
        request_id: "t3_10".into(),
        bot_username: "fabushi_bot".into(),
        short_name: "fabushi".into(),
        chat_id: CID_FABUSHI as i64,
        bot_user_id: UID_FABUSHI as i64,
        app_title: "全球法布施".into(),
        operation: "discover".into(),
        tool_name: None,
        arguments: json!({}),
        status_message: None,
    });
    wv.handle_bridge_event("__gpui_webmcp_page_ready", "{}");
    assert!(wv.webmcp_page_ready && wv.pending_webmcp_queue.is_empty());
    let _ = wv.take_pending_actions();

    // [T3-11] F11 + F12: AppShell WS MiniAppWebMcpCommand -> failed webmcp_response -> WsClientFrame::MiniAppWebMcpResult + stage CTA
    let _ = shell.update(&mut cx, |s, cx| {
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::MiniAppWebMcpCommand(MiniAppWebMcpCommand {
                request_id: "t3_11_cmd".into(),
                bot_username: "fabushi_bot".into(),
                short_name: "fabushi".into(),
                chat_id: CID_FABUSHI as i64,
                bot_user_id: UID_FABUSHI as i64,
                app_title: "全球法布施".into(),
                operation: "call_tool".into(),
                tool_name: Some("fabushi_send_dharma".into()),
                arguments: json!({}),
                status_message: None,
            })),
            cx,
        );
    });
    let ov_ready = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.miniapps
                    .miniapp_overlay
                    .as_ref()
                    .is_some_and(|ov| ov.read(cx).launch_context.is_some())
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ov_ready, "[T3-11] overlay must launch for MiniAppWebMcpCommand");
    let ov = shell
        .read_with(&mut cx, |s, _| s.miniapps.miniapp_overlay.clone().unwrap())
        .unwrap();
    let _ = ov.update(&mut cx, |w, wcx| {
        w.handle_bridge_event(
            "webmcp_response",
            r#"{"request_id":"t3_11_err","ok":false,"result":{},"error":"未注册的 WebMCP 工具: fabushi_send_dharma","tools":[],"state_summary":null,"control_mode":"idle"}"#,
        );
        wcx.notify();
    });
    let got_t3_11_ws = pump_until(&app, 1500, |_| {
        while let Ok(frame) = client_rx.try_recv() {
            if let WsClientFrame::MiniAppWebMcpResult(resp) = frame {
                if resp.request_id == "t3_11_err"
                    && !resp.ok
                    && resp
                        .error
                        .as_deref()
                        .is_some_and(|e| e.contains("未注册的 WebMCP 工具"))
                {
                    return true;
                }
            }
        }
        false
    })
    .await;
    assert!(
        got_t3_11_ws,
        "[T3-11] failed webmcp_response must send !ok MiniAppWebMcpResult over WsChannel"
    );
    let _ = shell.update(&mut cx, |s, cx| {
        let guard_msg = make_proto_text_msg(
            8111,
            CID_FABUSHI,
            UID_FABUSHI,
            "⚠️ **小程序操作未完成**：未注册的 WebMCP 工具: fabushi_send_dharma",
            Some(vec![vec![ProtoInlineKeyboardButton {
                text: "打开全球法布施".into(),
                callback_data: None,
                url: None,
                web_app_url: Some("gpui-miniapp://fabushi_bot/fabushi".into()),
            }]]),
        );
        s.handle_ws_server_frame(WsServerFrame::Update(UpdateEvent::NewMessage(guard_msg)), cx);
        s.handle_stage_bot_button(
            8111,
            InlineButtonType::WebApp("gpui-miniapp://fabushi_bot/fabushi".into()),
            cx,
        );
        assert!(s.miniapps.miniapp_overlay.is_some() || s.miniapps.pending_miniapp_confirm.is_some());
    });

    // [T3-12] F10 + F12: Per-user Fabushi provider hydration + recovery
    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("hermes-local-v1").unwrap());
    hydrate_local_subscriptions_into_fabushi(&host, &[sample_vertex_item()]);
    let _ = shell.update(&mut cx, |s, cx| {
        s.attach_local_fabushi_host(host);
        assert!(s.can_run_local_fabushi_turn(cx));
    });
    let _ = app;
}

// ============================================================================
// TIER 4: REAL-WORLD APPLICATION SCENARIOS (6 end-to-end user journeys)
// ============================================================================

#[test]
fn tier4_scenario_01_fabushi_cold_start_menu_miniapp_webmcp() {
    run_in_headless_subprocess("tier4_scenario_01_fabushi_cold_start_menu_miniapp_webmcp");
}

async fn t4_s01_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T4-01] Cold start @fabushi_bot -> START bar -> /start -> external MenuButton -> WebMCP readiness -> clean failure recovery
    let _ = shell.update(&mut cx, |s, cx| {
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        s.stage.update(cx, |st, _| {
            st.set_bot_prestart_state(
                Some("全球法布施与净心禅室".to_string()),
                true,
                true,
            );
        });
        assert!(s.stage.read(cx).show_bot_start_bar);
        s.stage.update(cx, |st, _| {
            st.set_bot_prestart_state(None, false, true);
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        let comp = s.composer.read(cx);
        assert!(comp.bot_menu_controls.is_active);
        assert_eq!(comp.bot_menu_controls.text, "全球法布施");
    });

    let mut wv = BotAttachWebView::new();
    wv.launch(
        AttachBotEntry {
            bot_id: UID_FABUSHI,
            name: "全球法布施".into(),
            short_name: "fabushi".into(),
            allowed_peer_types: peer_types::USER | peer_types::BOT,
            in_main_menu: true,
            in_attach_menu: true,
            disclaimer_required: false,
            request_write_access: false,
        },
        "/miniapps/fabushi_bot/fabushi",
    );
    wv.handle_bridge_event("__gpui_webmcp_page_ready", "{}");
    wv.handle_bridge_event(
        "webmcp_state_update",
        r#"{"state_summary":{"merit_count":7}}"#,
    );
    assert!(wv.webmcp_page_ready);
    assert_eq!(
        wv.last_webmcp_state_summary.as_ref().and_then(|v| v["merit_count"].as_i64()),
        Some(7)
    );

    // Unregistered tool failure -> gpui_webmcp_tools_changed -> retry recovery sequence
    wv.handle_bridge_event(
        "webmcp_response",
        r#"{"request_id":"s01_fail","ok":false,"result":{},"error":"未注册的 WebMCP 工具: fabushi_send_dharma","tools":[],"state_summary":null,"control_mode":"ai_controlling"}"#,
    );
    assert!(wv.control_status_text.contains("操作提示: 未注册的 WebMCP 工具"));
    let fail_actions = wv.take_pending_actions();
    assert!(matches!(
        fail_actions.first(),
        Some(BotWebViewPendingAction::WebMcpResponseReady(r)) if !r.ok
    ));

    wv.handle_bridge_event(
        "gpui_webmcp_tools_changed",
        &json!({
            "tools": [{
                "name": "fabushi_send_dharma",
                "description": "Send Dharma",
                "input_schema": { "type": "object" },
                "read_only": false
            }]
        })
        .to_string(),
    );
    assert!(wv.webmcp_discovered_tools.iter().any(|t| t.name == "fabushi_send_dharma"));
    wv.handle_bridge_event(
        "webmcp_response",
        r#"{"request_id":"s01_retry","ok":true,"result":{"accepted":true},"tools":[],"state_summary":{"merit_count":8},"control_mode":"ai_controlling"}"#,
    );
    assert_eq!(wv.control_status_text, "AI 操作已完成，等待下一步指令");
    assert_eq!(
        wv.last_webmcp_state_summary.as_ref().and_then(|v| v["merit_count"].as_i64()),
        Some(8)
    );
    let retry_actions = wv.take_pending_actions();
    assert!(matches!(
        retry_actions.first(),
        Some(BotWebViewPendingAction::WebMcpResponseReady(r)) if r.ok
    ));
    let _ = app;
}

#[test]
fn tier4_scenario_02_fabushi_copy_switch_group_zero_leakage() {
    run_in_headless_subprocess("tier4_scenario_02_fabushi_copy_switch_group_zero_leakage");
}

async fn t4_s02_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T4-02] Open @fabushi_bot inspector -> copy username -> immediately switch to Group AA 记账分账助手 -> zero bio/toast leakage
    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| insp.open());
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.bio.contains("全球法布施"))
            .unwrap_or(false)
    })
    .await;

    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| {
            insp.copied_feedback = Some("已复制: @fabushi_bot".into());
        });
        s.stage.update(cx, |st, _| {
            st.toast_banner = Some("Bot toast".into());
        });
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
    });
    let ok_grp = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| {
                let insp = s.inspector.read(cx);
                insp.members.is_group_or_channel && insp.members.members.len() == 3
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_grp, "[T4-02] group hydration failed");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        let st = s.stage.read(cx);
        assert!(insp.profile.bio.is_empty(), "[T4-02] @fabushi_bot bio leaked into group!");
        assert!(insp.copied_feedback.is_none(), "[T4-02] 已复制 leaked into group!");
        assert!(st.toast_banner.is_none(), "[T4-02] stage toast leaked into group!");
        assert!(!insp.profile.is_bot);
        assert_eq!(insp.members.members.len(), 3);
    });
}

#[test]
fn tier4_scenario_03_fabushi_miniapp_plus_compose_cancel_restore() {
    run_in_headless_subprocess("tier4_scenario_03_fabushi_miniapp_plus_compose_cancel_restore");
}

async fn t4_s03_body(app: AsyncApp) {
    let mut cx = app.clone();
    let client = new_isolated_client("http://127.0.0.1:9".to_string());
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T4-03] @fabushi_bot Mini App active -> click + compose (0 recipients) -> composer & menu hidden -> cancel -> restore
    let _ = shell.update(&mut cx, |s, cx| {
        s.theme_state.palette =
            TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        s.sync_theme_and_locale_to_views(cx);
        s.sidebar.update(cx, |sb, sb_cx| {
            s.stage.update(sb_cx, |st, st_cx| {
                s.inspector.update(st_cx, |insp, _| {
                    select_authoritative_dialog(sb, st, insp, CID_FABUSHI);
                });
            });
        });
        s.sync_composer_and_inspector_miniapp_state(cx);
        assert!(s.composer.read(cx).bot_menu_controls.is_active);
        s.composer.update(cx, |c, _| c.set_text("hello fabushi"));
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
        assert!(s.sidebar.read(cx).is_apple_compose_active());
    });
    let s03_isolated = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    assert!(s03_isolated, "[T4-03] + compose must isolate via observer");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let comp = s.composer.read(cx);
        assert!(!comp.bot_menu_controls.is_active, "[T4-03] menu leaked in + compose");
        assert!(comp.text().is_empty(), "[T4-03] draft leaked in + compose");
        assert!(s.should_hide_composer(cx));
    });
    let _ = shell.update(&mut cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    let s03_restored = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| {
                !s.composer.read(cx).is_compose_isolated
                    && s.composer.read(cx).bot_menu_controls.is_active
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        s03_restored,
        "[T4-03] menu button did not restore after cancelling +"
    );
    let _ = app;
}

#[test]
fn tier4_scenario_04_grok_provider_error_banner_cta_and_recovery() {
    run_in_headless_subprocess("tier4_scenario_04_grok_provider_error_banner_cta_and_recovery");
}

async fn t4_s04_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T4-04] Unconfigured provider on Bot -> inline PROVIDER_ERROR card + non-overlapping toast -> navigate to AI Providers -> hydrate & succeed
    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("hermes-local-v1").unwrap());
    host.core_engine().providers().set_allow_local_fallback(false);
    host.core_engine().providers().set_offline_rule_bot_allowed(false);
    host.sync_all_providers();
    let _ = shell.update(&mut cx, |s, cx| {
        s.attach_local_fabushi_host(host.clone());
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
        let target = s.resolve_fabushi_bot_target(CID_FABUSHI, cx).unwrap();
        s.spawn_local_fabushi_turn(CID_FABUSHI, target, None, "你好".into(), cx);
    });
    let got_err_card = pump_until(&app, 4000, |app| {
        shell
            .read_with(app, |s, cx| {
                s.stage.read(cx).messages.iter().any(|m| {
                    m.bot_keyboard.as_ref().is_some_and(|rows| {
                        rows.iter().flatten().any(|b| {
                            b.button_type
                                == InlineButtonType::Callback("hermes:open_ai_providers".into())
                        })
                    })
                })
            })
            .unwrap_or(false)
    })
    .await;
    assert!(got_err_card, "[T4-04] expected inline PROVIDER_ERROR CTA card");

    let _ = shell.update(&mut cx, |s, cx| {
        s.execute_hermes_inline_callback("hermes:open_ai_providers".into(), cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert_eq!(
            s.settings_router.read(cx).settings_section,
            SettingsSectionType::AiProviders
        );
    });
    hydrate_local_subscriptions_into_fabushi(&host, &[sample_vertex_item()]);
    let _ = shell.read_with(&mut cx, |s, cx| {
        assert!(s.can_run_local_fabushi_turn(cx), "[T4-04] provider recovery failed");
    });
}

#[test]
fn tier4_scenario_05_multi_peer_rapid_hopping_under_active_toasts() {
    run_in_headless_subprocess("tier4_scenario_05_multi_peer_rapid_hopping_under_active_toasts");
}

async fn t4_s05_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T4-05] Rapid hopping: @fabushi_bot -> Group -> Alice -> @GrokAI under active copy toast & stage banner
    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| insp.open());
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = pump_until(&app, 2000, |app| {
        shell
            .read_with(app, |s, cx| s.inspector.read(cx).profile.is_bot)
            .unwrap_or(false)
    })
    .await;

    let _ = shell.update(&mut cx, |s, cx| {
        s.inspector.update(cx, |insp, _| {
            insp.copied_feedback = Some("已复制: @fabushi_bot".into());
        });
        s.stage.update(cx, |st, _| {
            st.toast_banner = Some("Transient stage toast".into());
        });
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
        assert!(s.inspector.read(cx).copied_feedback.is_none());
        assert!(s.stage.read(cx).toast_banner.is_none());

        s.sync_selected_chat_inspector_and_stage(CID_ALICE, cx);
        s.sync_selected_chat_inspector_and_stage(CID_GROK, cx);
    });
    let ok_grok = pump_until(&app, 3000, |app| {
        shell
            .read_with(app, |s, cx| {
                let insp = s.inspector.read(cx);
                insp.profile.is_bot
                    && insp.profile.username.to_ascii_lowercase().contains("grokai")
            })
            .unwrap_or(false)
    })
    .await;
    assert!(ok_grok, "[T4-05] final hop to @GrokAI failed");
    let _ = shell.read_with(&mut cx, |s, cx| {
        let insp = s.inspector.read(cx);
        assert!(insp.profile.phone.is_empty(), "[T4-05] Alice phone leaked to GrokAI");
        assert!(insp.profile.miniapp_url.is_none(), "[T4-05] Fabushi Mini App leaked to GrokAI");
        assert!(insp.copied_feedback.is_none());
    });
}

#[test]
fn tier4_scenario_06_full_desktop_alignment_end_to_end_journey() {
    run_in_headless_subprocess("tier4_scenario_06_full_desktop_alignment_end_to_end_journey");
}

async fn t4_s06_body(app: AsyncApp) {
    let mut cx = app.clone();
    let bootstrap = new_isolated_client("http://127.0.0.1:9".to_string());
    let (base_url, _srv) = spawn_alignment_mock_http_server(&bootstrap, default_mock_server_data());
    let client = new_isolated_client(base_url);
    let _rt = client.runtime().enter();
    let shell = cx
        .update(|cx| setup_authenticated_shell(client.clone(), cx))
        .unwrap();

    // [T4-06] Full end-to-end alignment journey across Bot, Group, +, Copy, Toast, Fabushi, and WebMCP
    let host = Arc::new(FabushiBotHost::new_unified_ephemeral("hermes-local-v1").unwrap());
    hydrate_local_subscriptions_into_fabushi(&host, &[sample_vertex_item()]);
    let _ = shell.update(&mut cx, |s, cx| {
        s.attach_local_fabushi_host(host);
        s.theme_state.palette =
            TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18);
        s.sync_theme_and_locale_to_views(cx);
        s.inspector.update(cx, |insp, _| insp.open());
        s.sync_selected_chat_inspector_and_stage(CID_FABUSHI, cx);
    });
    let _ = pump_until(&app, 2500, |app| {
        shell
            .read_with(app, |s, cx| {
                s.inspector.read(cx).profile.is_bot && s.composer.read(cx).bot_menu_controls.is_active
            })
            .unwrap_or(false)
    })
    .await;

    let _ = shell.update(&mut cx, |s, cx| {
        assert!(s.can_run_local_fabushi_turn(cx));
        s.inspector.update(cx, |insp, _| {
            insp.copied_feedback = Some("已复制: @fabushi_bot".into());
        });
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
        cx.notify();
    });
    let s06_isolated = pump_until(&app, 1000, |app| {
        shell
            .read_with(app, |s, cx| s.composer.read(cx).is_compose_isolated)
            .unwrap_or(false)
    })
    .await;
    assert!(s06_isolated, "[T4-06] + compose must isolate via observer");
    let _ = shell.update(&mut cx, |s, cx| {
        assert!(!s.composer.read(cx).bot_menu_controls.is_active);
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
        s.sync_selected_chat_inspector_and_stage(CID_GROUP, cx);
        assert!(s.inspector.read(cx).copied_feedback.is_none());
        assert!(s.inspector.read(cx).profile.bio.is_empty());
    });
}
