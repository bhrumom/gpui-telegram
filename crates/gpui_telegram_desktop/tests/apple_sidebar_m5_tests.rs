//! Comprehensive Milestone M5 (Features F11, F12, F13) integration and headless GPUI tests.
//!
//! Covers:
//! - Hard regression guardrails (`IconName::ALL == 64`, 7 `IconName::EXTRA` hub SVGs, `NavRail`, `ChatSidebar`)
//! - `AppleAppSidebar` mount, 6-section switching, `SemanticWorkspaceTree` distinct `state_hash`, collapse & Quick Drawer
//! - `PublishReviewModal` live `validate_creation_for_review()`, HTTP submit/422/409, and Admin Reviewer Queue (`403` vs `200`)
//! - IME CJK+ASCII isolation, `Cmd+A/C/V`, background shortcut blocking, and `Escape` priority cascade
//! - Unified Chat-to-Hub deep-links across all 4 entry points (`tg://apple/*` & `creation:open:*`) + bare `tg://` parsing
//! - Live WS `UserCreationChanged` / `PublicCatalogChanged("removed")` eviction, `@CreatorStudioBot` launch, and logout/account-switch wipe

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use gpui::{App, AppContext, Application, AssetSource, AsyncApp, ClipboardItem, Entity};
use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
use gpui_telegram_desktop::app::session::send_as_peers::SendAsPeer;
use gpui_telegram_desktop::components::bots::bot_attach_web_view::{
    BotAttachWebView, BotWebViewInputTarget, BotWebViewPendingAction,
};
use gpui_telegram_desktop::components::ui::layers::standalone_layer_stack::LayerStackOption;
use gpui_telegram_desktop::components::ui::TextInputController;
use gpui_telegram_desktop::theme::{IconName, TdesktopThemeMode, TelegramAssetSource};
use gpui_telegram_desktop::views::app_shell::shell_content_hub::{
    is_apple_deep_link_candidate, unwrap_apple_callback_uri,
};
use gpui_telegram_desktop::views::app_shell::shell_state::PendingMiniAppConfirm;
use gpui_telegram_desktop::views::auth::{AuthWizardEvent, LoginInputField};
use gpui_telegram_desktop::views::settings::sections::settings_ai_providers::AiProviderField;
use gpui_telegram_desktop::views::settings::sections::settings_computers::{
    AssistInputField, CloudFormField, SettingsComputersPage, SettingsComputersTab,
};
use gpui_telegram_desktop::views::settings::sections::settings_information::ProfileEditField;
use gpui_telegram_desktop::views::settings::sections::settings_local_passcode::SettingsLocalPasscodeSections;
use gpui_telegram_core::core::deep_links::parse_apple_deep_link;
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleSidebarEvent, AppleSidebarSection, COLLAPSED_WIDTH as APPLE_COLLAPSED_WIDTH,
    EXPANDED_WIDTH as APPLE_EXPANDED_WIDTH,
};
use gpui_telegram_desktop::views::boxes::modal_types::ModalDialogKind;
use gpui_telegram_desktop::views::boxes::peers::bot_profile_box::{
    BotProfileBox, BotProfileBoxAction,
};
use gpui_telegram_desktop::views::boxes::peers::create_managed_bot_box::CreateManagedBotBoxPeers;
use gpui_telegram_desktop::views::canonical_composer::BotSlashCommandItem;
use gpui_telegram_desktop::views::chat_sidebar::{
    ChatSidebar, CreationInputField, DialogDeliveryStatus, DialogItemModel, COLLAPSED_WIDTH,
    DEFAULT_WIDTH, ROW_HEIGHT,
};
use gpui_telegram_desktop::views::composer::bot_keyboard::{
    BotKeyboardButton, BotKeyboardButtonKind,
};
use gpui_telegram_desktop::views::computer_workbench::WorkbenchTab;
use gpui_telegram_desktop::views::content_hub::publish_review_modal::{
    contains_obscured_xss_pattern, is_invisible_format_char, normalize_non_empty_reason,
    visible_char_count,
};
use gpui_telegram_desktop::views::content_hub::{
    ContentHubEvent, ContentHubInputTarget, ContentHubSection, EditableFieldState, GameInput,
    GameStatus, MiniGameRuntime, PublishModalField, PublishReviewModalState, ReviewerAccessState,
};
use gpui_telegram_desktop::views::drawer::inspector_tabs::{
    BotConfigFocusField, InspectorAction, InspectorSubpage, InspectorTab,
};
use gpui_telegram_desktop::views::global_tasks_workspace::{
    StudioInputTarget, TasksWorkspaceTab,
};
use gpui_telegram_desktop::views::main_menu_drawer::MainMenuCommand;
use gpui_telegram_desktop::views::nav_rail::{
    NavDestination, NavFolderItem, NavRail, NAV_RAIL_WIDTH,
};
use gpui_telegram_desktop::views::settings::settings_type::SettingsSectionType;
use gpui_telegram_desktop::views::shell::{ActiveWorkspacePanel, AppShell};
use gpui_telegram_desktop::views::stage::cards::bot_assist_card::BotAssistLoginField;
use gpui_telegram_desktop::views::stage::cards::text_card::{parse_markdown_spans, RichSpanKind};
use gpui_telegram_desktop::views::stage::cards::InlineButtonType;
use gpui_telegram_desktop::views::stage::entities::{parse_rich_blocks, RichBlock};
use gpui_telegram_desktop::views::stage::entities_render::RichTextAction;
use gpui_telegram_desktop::views::stage::stage_bars::TopBarUiAction;
use gpui_telegram_media::media::audio::{MusicPlaybackState, MusicTrackEntry};
use gpui_telegram_proto::creations::{
    AppStoreCatalogResponse, AppStoreListingPayload, BookChapter, BookPage, BookPayload,
    BookRichBlock, CardPair, CatalogQueryResponse, CoverArtwork, CreationAuditListResponse,
    CreationKind, CreationListResponse, CreationPayload, CreationReviewAuditDto,
    CreationReviewStatus, CreationSyncResponse, CreationVisibility, GameCategory, GameEngineKind,
    GamePayload, GameSpec, GridPuzzleLevel, LinkedMiniAppRef, MusicPayload, MusicTrack, NoteEvent,
    PublicCatalogItemDto, ReviewDecision, ReviewDecisionRequest, SubmitReviewRequest,
    SubmitReviewResponse, TriviaQuestion, UpdateCreationRequest, UserCreationDto,
    UserCreationStatesResponse, CREATOR_STUDIO_BOT_USER_ID,
};
use gpui_telegram_proto::miniapp_contracts::{
    BotPublicProfileDto, BotRegistryEntry, MiniAppEntry, MiniAppLaunchMode,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::frames::WsServerFrame;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CHILD_ENV: &str = "GPUI_M5_APPLE_SIDEBAR_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_M5_APPLE_SIDEBAR_TARGET";

type AsyncRunnerFn = fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()>>>;

enum Runner {
    Sync(fn(&mut App)),
    Async(AsyncRunnerFn),
}

fn dispatch_headless_target(target: &str) -> ! {
    let runner = match target {
        "test_m5_gpui_sidebar_navigation_theme_collapse_and_quick_drawer" => {
            Runner::Sync(body_sidebar_navigation_theme_collapse_and_quick_drawer)
        }
        "test_m5_gpui_publish_review_modal_and_reviewer_queue" => {
            Runner::Async(|app| Box::pin(body_publish_review_modal_and_reviewer_queue(app)))
        }
        "test_m5_gpui_ime_isolation_escape_priority_and_shortcut_blocking" => {
            Runner::Sync(body_ime_isolation_escape_priority_and_shortcut_blocking)
        }
        "test_m5_gpui_deep_links_all_4_entry_points" => {
            Runner::Sync(body_deep_links_all_4_entry_points)
        }
        "test_m5_gpui_ws_eviction_creator_bot_and_account_switch_wipe" => {
            Runner::Async(|app| Box::pin(body_ws_eviction_creator_bot_and_account_switch_wipe(app)))
        }
        "test_m5_gpui_reg_f1_shell_observer_and_modal_close_ime_clear" => {
            Runner::Async(|app| {
                Box::pin(body_reg_f1_shell_observer_and_modal_close_ime_clear(app))
            })
        }
        "test_m5_gpui_reg_f2_b1_update_then_failed_submit_no_stale_409" => {
            Runner::Async(|app| {
                Box::pin(body_reg_f2_b1_update_then_failed_submit_no_stale_409(app))
            })
        }
        "test_m5_gpui_reg_f3_b2_per_section_catalog_gen_and_ws_query_isolation" => {
            Runner::Async(|app| {
                Box::pin(body_reg_f3_b2_per_section_catalog_gen_and_ws_query_isolation(app))
            })
        }
        "test_m5_gpui_reg_f4_b3_remote_delete_vertical_switch_and_detail_clear" => {
            Runner::Sync(body_reg_f4_b3_remote_delete_vertical_switch_and_detail_clear)
        }
        "test_m5_gpui_reg_b4_ime_priority_quick_drawer_settings_modal_and_miniapp" => {
            Runner::Sync(body_reg_b4_ime_priority_quick_drawer_settings_modal_and_miniapp)
        }
        "test_m5_gpui_iter3_reg_focus_ime_escape_smuggling_and_unauth_guards" => {
            Runner::Sync(body_iter3_reg_focus_ime_escape_smuggling_and_unauth_guards)
        }
        "test_m5_gpui_iter3_reg_async_reject_audit_bot_profile_and_delete" => {
            Runner::Async(|app| {
                Box::pin(body_iter3_reg_async_reject_audit_bot_profile_and_delete(app))
            })
        }
        "test_m5_gpui_iter4_reg_ime_sync_deeplink_webapp_and_unauth_guards" => {
            Runner::Sync(body_iter4_reg_ime_sync_deeplink_webapp_and_unauth_guards)
        }
        "test_m5_gpui_iter4_reg_async_review_queue_prune_and_ws_monotonicity" => {
            Runner::Async(|app| {
                Box::pin(body_iter4_reg_async_review_queue_prune_and_ws_monotonicity(
                    app,
                ))
            })
        }
        "test_m5_gpui_iter5_reg_deterministic_async_fencing_and_detail_monotonicity" => {
            Runner::Async(|app| {
                Box::pin(
                    body_iter5_reg_deterministic_async_fencing_and_detail_monotonicity(app),
                )
            })
        }
        "test_m5_gpui_iter6_reg_inspector_media_compose_hub_and_observer_ime" => {
            Runner::Sync(body_iter6_reg_inspector_media_compose_hub_and_observer_ime)
        }
        "test_m5_gpui_iter7_reg_zorder_observers_passcode_unicode_and_ws" => {
            Runner::Sync(body_iter7_reg_zorder_observers_passcode_unicode_and_ws)
        }
        "test_m5_gpui_iter8_reg_final_hardening_closure" => {
            Runner::Async(|app| Box::pin(body_iter8_reg_final_hardening_closure(app)))
        }
        "test_m5_gpui_iter9_reg_completeness_closure" => {
            Runner::Async(|app| Box::pin(body_iter9_reg_completeness_closure(app)))
        }
        other => {
            eprintln!("unknown M5 headless target: {other}");
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
                            let msg = payload
                                .downcast_ref::<&str>()
                                .copied()
                                .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                                .unwrap_or("panic");
                            eprintln!("[m5 headless panic] {target_owned}: {msg}");
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
                                let msg = payload
                                    .downcast_ref::<&str>()
                                    .copied()
                                    .or_else(|| {
                                        payload.downcast_ref::<String>().map(|s| s.as_str())
                                    })
                                    .unwrap_or("panic");
                                eprintln!("[m5 async headless panic] {t}: {msg}");
                                std::process::exit(101);
                            }
                        }
                    })
                    .detach();
                }
            });
    }));
    if let Err(payload) = outer {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("panic");
        eprintln!("[m5 outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_M5_HEADLESS_ENTRY: extern "C" fn() = macos_mod_init_m5_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_m5_entry() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
            dispatch_headless_target(&target);
        }
    }
}

fn run_in_headless_subprocess(test_fn_name: &str, offline: bool) {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(test_fn_name);
    }

    let tmp_dir = TempDir::new().expect("create TempDir for M5 headless test");
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp_dir.path())
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture");
    if offline {
        cmd.env("GPUI_OFFLINE_TEST", "1");
    }
    let output = cmd.output().expect("spawn M5 headless child");
    assert!(
        output.status.success(),
        "M5 headless test failed for {test_fn_name}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn sample_auth(user_id: u64, username: &str) -> AuthResponse {
    AuthResponse {
        token: format!("tok_m5_{user_id}"),
        session_id: format!("sess_m5_{user_id}"),
        user_id,
        username: username.to_string(),
        first_name: Some(username.to_string()),
        created_at: None,
        expires_at: None,
    }
}

fn sample_book_creation(
    creation_id: &str,
    owner_user_id: i64,
    title: &str,
    summary: &str,
    readable_body: &str,
    status: CreationReviewStatus,
    cloud_revision: i64,
) -> UserCreationDto {
    UserCreationDto {
        creation_id: creation_id.to_string(),
        owner_user_id,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(9001),
        kind: CreationKind::Book,
        title: title.to_string(),
        subtitle: "Apple Books Original".to_string(),
        summary: summary.to_string(),
        author_display_name: "Ada Lovelace".to_string(),
        language: "zh-CN".to_string(),
        genres: vec!["Sci-Fi".to_string()],
        tags: vec!["space".to_string()],
        cover: CoverArtwork {
            gradient_start_hex: "#FF9500".to_string(),
            gradient_end_hex: "#FF5E3A".to_string(),
            accent_hex: "#FFFFFF".to_string(),
            icon_emoji: "📖".to_string(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        age_rating: "4+".to_string(),
        payload: CreationPayload::Book(BookPayload {
            author: "Ada Lovelace".to_string(),
            series_name: None,
            series_index: None,
            blurb: summary.to_string(),
            estimated_read_mins: 8,
            word_count: readable_body.chars().count() as u32,
            chapters: vec![BookChapter {
                chapter_id: "ch1".to_string(),
                chapter_number: 1,
                title: "Chapter 1".to_string(),
                subtitle: None,
                pages: vec![BookPage {
                    page_number: 1,
                    blocks: vec![BookRichBlock::Paragraph {
                        text: readable_body.to_string(),
                    }],
                }],
            }],
        }),
        version: 1,
        cloud_revision,
        review_status: status,
        visibility: CreationVisibility::Private,
        has_live_published_version: status == CreationReviewStatus::Published,
        published_version: (status == CreationReviewStatus::Published).then_some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: None,
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_000_000 + cloud_revision * 1000,
        deleted_at_ms: None,
        rating_avg: 4.9,
        rating_count: 12,
        engagement_count: 100,
        save_or_install_count: 42,
    }
}

fn sample_public_book_item(creation_id: &str, title: &str) -> PublicCatalogItemDto {
    PublicCatalogItemDto {
        creation_id: creation_id.to_string(),
        owner_user_id: 1001,
        kind: CreationKind::Book,
        title: title.to_string(),
        subtitle: "Featured".to_string(),
        summary: "A rich published sci-fi adventure available in Apple Books.".to_string(),
        author_display_name: "Ada Lovelace".to_string(),
        language: "en".to_string(),
        genres: vec!["Sci-Fi".to_string()],
        tags: vec!["featured".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        published_version: 1,
        published_at_ms: 1_700_000_001_000,
        updated_at_ms: 1_700_000_002_000,
        rating_avg: 4.8,
        rating_count: 25,
        engagement_count: 420,
        save_or_install_count: 88,
        featured_rank: Some(1),
        deep_link: format!("tg://apple/books/{creation_id}"),
        payload: None,
    }
}

fn sample_public_catalog_item(
    creation_id: &str,
    kind: CreationKind,
    title: &str,
) -> PublicCatalogItemDto {
    let slug = match kind {
        CreationKind::Book => "books",
        CreationKind::Music => "music",
        CreationKind::Game => "games",
        CreationKind::App => "appstore",
    };
    PublicCatalogItemDto {
        creation_id: creation_id.to_string(),
        owner_user_id: 1001,
        kind,
        title: title.to_string(),
        subtitle: "Featured".to_string(),
        summary: format!("Published catalog entry for {title}."),
        author_display_name: "Ada Lovelace".to_string(),
        language: "en".to_string(),
        genres: vec!["Sci-Fi".to_string()],
        tags: vec!["featured".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        published_version: 1,
        published_at_ms: 1_700_000_001_000,
        updated_at_ms: 1_700_000_002_000,
        rating_avg: 4.8,
        rating_count: 25,
        engagement_count: 420,
        save_or_install_count: 88,
        featured_rank: Some(1),
        deep_link: format!("tg://apple/{slug}/{creation_id}"),
        payload: None,
    }
}

/// Lightweight in-process mock HTTP server speaking real M1/M2/M4 `gpui_telegram_proto` JSON.
struct MockM5State {
    admin_allowed: AtomicBool,
    creations: Mutex<Vec<UserCreationDto>>,
    review_queue: Mutex<Vec<UserCreationDto>>,
    created_bot_chat_peer_ids: Mutex<Vec<u64>>,
    review_decisions_received: Mutex<Vec<(String, ReviewDecisionRequest)>>,
    put_requests_received: Mutex<Vec<(String, UpdateCreationRequest)>>,
    fail_next_submit_status: Mutex<Option<u16>>,
    submit_delay_ms: AtomicU64,
    audit_delay_ms: AtomicU64,
    review_queue_delay_ms: AtomicU64,
    review_queue_requests_started: AtomicU64,
    review_queue_responses_served: AtomicU64,
    detail_delay_ms: AtomicU64,
    detail_requests_started: AtomicU64,
    detail_responses_served: AtomicU64,
    creations_sync_delay_ms: AtomicU64,
    creations_sync_requests_started: AtomicU64,
    creations_sync_responses_served: AtomicU64,
    detail_overrides: Mutex<std::collections::HashMap<String, UserCreationDto>>,
    serve_unstarted_creator_bot_profile: AtomicBool,
    bot_profile_requests: AtomicU64,
    catalog_books: Mutex<Vec<PublicCatalogItemDto>>,
    catalog_music: Mutex<Vec<PublicCatalogItemDto>>,
    catalog_games: Mutex<Vec<PublicCatalogItemDto>>,
    catalog_appstore: Mutex<AppStoreCatalogResponse>,
    recorded_catalog_requests: Mutex<Vec<String>>,
    next_chat_id: AtomicU64,
}

impl MockM5State {
    fn new(admin_allowed: bool, creations: Vec<UserCreationDto>, queue: Vec<UserCreationDto>) -> Self {
        Self {
            admin_allowed: AtomicBool::new(admin_allowed),
            creations: Mutex::new(creations),
            review_queue: Mutex::new(queue),
            created_bot_chat_peer_ids: Mutex::new(Vec::new()),
            review_decisions_received: Mutex::new(Vec::new()),
            put_requests_received: Mutex::new(Vec::new()),
            fail_next_submit_status: Mutex::new(None),
            submit_delay_ms: AtomicU64::new(0),
            audit_delay_ms: AtomicU64::new(0),
            review_queue_delay_ms: AtomicU64::new(0),
            review_queue_requests_started: AtomicU64::new(0),
            review_queue_responses_served: AtomicU64::new(0),
            detail_delay_ms: AtomicU64::new(0),
            detail_requests_started: AtomicU64::new(0),
            detail_responses_served: AtomicU64::new(0),
            creations_sync_delay_ms: AtomicU64::new(0),
            creations_sync_requests_started: AtomicU64::new(0),
            creations_sync_responses_served: AtomicU64::new(0),
            detail_overrides: Mutex::new(std::collections::HashMap::new()),
            serve_unstarted_creator_bot_profile: AtomicBool::new(false),
            bot_profile_requests: AtomicU64::new(0),
            catalog_books: Mutex::new(Vec::new()),
            catalog_music: Mutex::new(Vec::new()),
            catalog_games: Mutex::new(Vec::new()),
            catalog_appstore: Mutex::new(AppStoreCatalogResponse::default()),
            recorded_catalog_requests: Mutex::new(Vec::new()),
            next_chat_id: AtomicU64::new(88_001),
        }
    }

    fn prune_catalog_item(&self, cid: &str) {
        self.catalog_books
            .lock()
            .unwrap()
            .retain(|i| i.creation_id != cid);
        self.catalog_music
            .lock()
            .unwrap()
            .retain(|i| i.creation_id != cid);
        self.catalog_games
            .lock()
            .unwrap()
            .retain(|i| i.creation_id != cid);
        let mut appstore = self.catalog_appstore.lock().unwrap();
        appstore.featured.retain(|i| i.creation_id != cid);
        appstore.apps.retain(|i| i.creation_id != cid);
        appstore.cross_vertical.retain(|i| i.creation_id != cid);
    }
}

fn parse_query_param(raw_path: &str, key: &str) -> Option<String> {
    let (_, query) = raw_path.split_once('?')?;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key && !v.is_empty() {
                return Some(v.replace('+', " "));
            }
        }
    }
    None
}

fn spawn_mock_m5_http_server(
    rt: &tokio::runtime::Handle,
    state: Arc<MockM5State>,
) -> String {
    let listener = rt.block_on(async { TcpListener::bind("127.0.0.1:0").await.expect("bind mock") });
    let addr = listener.local_addr().expect("local_addr");
    rt.spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let st = state.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut tmp = [0u8; 8192];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let n = match stream.read(&mut tmp).await {
                        Ok(n) if n > 0 => n,
                        _ => break,
                    };
                    raw.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let hdr = String::from_utf8_lossy(&raw[..pos]);
                            for line in hdr.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse::<usize>().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(he) = header_end {
                        if raw.len() >= he + content_len {
                            break;
                        }
                    }
                }
                if raw.is_empty() {
                    return;
                }
                let req_str = String::from_utf8_lossy(&raw).to_string();
                let mut lines = req_str.lines();
                let first_line = lines.next().unwrap_or_default();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("GET");
                let raw_path = parts.next().unwrap_or("/");
                let path = raw_path.split('?').next().unwrap_or(raw_path);
                let body_str = header_end
                    .and_then(|he| std::str::from_utf8(raw.get(he..)?).ok())
                    .unwrap_or("");

                let is_creations_sync_get = method == "GET" && path == "/api/v1/creations/sync";
                let is_review_queue_get =
                    method == "GET" && path == "/api/v1/creations/review-queue";
                let is_detail_get = method == "GET"
                    && path.starts_with("/api/v1/creations/")
                    && !path.ends_with("/audit")
                    && path != "/api/v1/creations/sync"
                    && path != "/api/v1/creations/review-queue";

                let (status, json_body) = match (method, path) {
                    ("GET", "/api/v1/creations/sync") => {
                        let items = st.creations.lock().unwrap().clone();
                        st.creations_sync_requests_started
                            .fetch_add(1, Ordering::SeqCst);
                        let delay = st.creations_sync_delay_ms.load(Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        let latest_revision =
                            items.iter().map(|c| c.cloud_revision).max().unwrap_or(0);
                        (
                            "200 OK",
                            serde_json::to_string(&CreationSyncResponse {
                                latest_revision,
                                items,
                            })
                            .unwrap(),
                        )
                    }
                    ("GET", "/api/v1/catalog/my-states") => (
                        "200 OK",
                        serde_json::to_string(&UserCreationStatesResponse { items: Vec::new() })
                            .unwrap(),
                    ),
                    ("GET", "/api/v1/creations/review-queue") => {
                        let allowed = st.admin_allowed.load(Ordering::SeqCst);
                        let items = if allowed {
                            st.review_queue.lock().unwrap().clone()
                        } else {
                            Vec::new()
                        };
                        st.review_queue_requests_started
                            .fetch_add(1, Ordering::SeqCst);
                        let delay = st.review_queue_delay_ms.load(Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        if !allowed {
                            (
                                "403 Forbidden",
                                r#"{"error":"forbidden","message":"Admin reviewer privilege required"}"#
                                    .to_string(),
                            )
                        } else {
                            (
                                "200 OK",
                                serde_json::to_string(&CreationListResponse {
                                    items,
                                    latest_revision: 10,
                                })
                                .unwrap(),
                            )
                        }
                    }
                    ("POST", "/api/v1/chats/create" | "/api/v1/chats") => {
                        let v: serde_json::Value =
                            serde_json::from_str(body_str).unwrap_or_default();
                        let peer_uid = v
                            .get("peer_user_id")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0);
                        st.created_bot_chat_peer_ids.lock().unwrap().push(peer_uid);
                        let cid = st.next_chat_id.fetch_add(1, Ordering::Relaxed) as i64;
                        let resp = serde_json::json!({
                            "chat_id": cid,
                            "title": "Creator Studio",
                            "kind": "bot",
                            "chat_type": 4,
                            "creator_id": 2001,
                            "username": "CreatorStudioBot",
                            "about": null,
                            "is_group": false,
                            "is_channel": false,
                            "is_bot": true,
                            "is_forum": false,
                            "is_archived": false,
                            "is_pinned": false,
                            "is_muted": false,
                            "mute_until_ms": null,
                            "pts": 1,
                            "last_message_id": 0,
                            "last_message_date_ms": 0,
                            "unread_count": 0,
                            "read_inbox_max_id": 0,
                            "read_outbox_max_id": 0,
                            "member_count": 2
                        });
                        ("201 Created", resp.to_string())
                    }
                    ("PUT", p) if p.starts_with("/api/v1/creations/") => {
                        let cid = p.trim_start_matches("/api/v1/creations/");
                        let req: UpdateCreationRequest =
                            serde_json::from_str(body_str).unwrap_or_default();
                        st.put_requests_received
                            .lock()
                            .unwrap()
                            .push((cid.to_string(), req.clone()));
                        let mut guard = st.creations.lock().unwrap();
                        if let Some(item) = guard.iter_mut().find(|c| c.creation_id == cid) {
                            if let Some(exp) = req.expected_revision {
                                if exp != item.cloud_revision {
                                    let err = serde_json::json!({
                                        "error": "revision_conflict",
                                        "message": "Stale revision",
                                        "expected_revision": exp,
                                        "current_revision": item.cloud_revision
                                    });
                                    ("409 Conflict", err.to_string())
                                } else {
                                    if let Some(t) = req.title {
                                        item.title = t;
                                    }
                                    if let Some(s) = req.summary {
                                        item.summary = s;
                                    }
                                    if let Some(vis) = req.visibility {
                                        item.visibility = vis;
                                    }
                                    if item.review_status == CreationReviewStatus::Rejected {
                                        item.review_status = CreationReviewStatus::Draft;
                                        item.rejection_reason = None;
                                        item.review_notes = None;
                                    }
                                    item.cloud_revision += 1;
                                    ("200 OK", serde_json::to_string(item).unwrap())
                                }
                            } else {
                                if item.review_status == CreationReviewStatus::Rejected {
                                    item.review_status = CreationReviewStatus::Draft;
                                    item.rejection_reason = None;
                                    item.review_notes = None;
                                }
                                item.cloud_revision += 1;
                                ("200 OK", serde_json::to_string(item).unwrap())
                            }
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("POST", p) if p.starts_with("/api/v1/creations/") && p.ends_with("/submit-review") => {
                        let delay = st.submit_delay_ms.load(Ordering::Relaxed);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        let cid = p
                            .trim_start_matches("/api/v1/creations/")
                            .trim_end_matches("/submit-review");
                        let forced_fail = st.fail_next_submit_status.lock().unwrap().take();
                        let mut guard = st.creations.lock().unwrap();
                        if let Some(item) = guard.iter_mut().find(|c| c.creation_id == cid) {
                            if forced_fail == Some(422) {
                                let err = serde_json::json!({
                                    "error": "validation_failed",
                                    "message": "Server policy requires extra chapter metadata",
                                    "validation_errors": ["Server policy requires extra chapter metadata"]
                                });
                                ("422 Unprocessable Entity", err.to_string())
                            } else if forced_fail == Some(500) {
                                (
                                    "500 Internal Server Error",
                                    r#"{"error":"internal","message":"Transient submit failure"}"#
                                        .to_string(),
                                )
                            } else if item.summary.chars().count() < 10 {
                                let err = serde_json::json!({
                                    "error": "validation_failed",
                                    "message": "Validation failed",
                                    "validation_errors": ["Summary must be at least 10 characters"]
                                });
                                ("422 Unprocessable Entity", err.to_string())
                            } else {
                                item.review_status = CreationReviewStatus::PendingReview;
                                item.rejection_reason = None;
                                item.cloud_revision += 1;
                                let resp = SubmitReviewResponse {
                                    creation: item.clone(),
                                    validation_errors: Vec::new(),
                                };
                                ("200 OK", serde_json::to_string(&resp).unwrap())
                            }
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("POST", p) if p.starts_with("/api/v1/creations/") && p.ends_with("/withdraw-review") => {
                        let cid = p
                            .trim_start_matches("/api/v1/creations/")
                            .trim_end_matches("/withdraw-review");
                        let updated = {
                            let mut guard = st.creations.lock().unwrap();
                            if let Some(item) = guard.iter_mut().find(|c| c.creation_id == cid) {
                                item.review_status = CreationReviewStatus::Draft;
                                item.cloud_revision += 1;
                                Some(item.clone())
                            } else {
                                None
                            }
                        };
                        if let Some(item) = updated {
                            st.review_queue
                                .lock()
                                .unwrap()
                                .retain(|c| c.creation_id != cid);
                            ("200 OK", serde_json::to_string(&item).unwrap())
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("POST", p) if p.starts_with("/api/v1/creations/") && p.ends_with("/unpublish") => {
                        let cid = p
                            .trim_start_matches("/api/v1/creations/")
                            .trim_end_matches("/unpublish");
                        let updated = {
                            let mut guard = st.creations.lock().unwrap();
                            if let Some(item) = guard.iter_mut().find(|c| c.creation_id == cid) {
                                item.review_status = CreationReviewStatus::Unpublished;
                                item.has_live_published_version = false;
                                item.visibility = CreationVisibility::Private;
                                item.cloud_revision += 1;
                                Some(item.clone())
                            } else {
                                None
                            }
                        };
                        if let Some(item) = updated {
                            st.review_queue
                                .lock()
                                .unwrap()
                                .retain(|c| c.creation_id != cid);
                            st.prune_catalog_item(cid);
                            ("200 OK", serde_json::to_string(&item).unwrap())
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("POST", p) if p.starts_with("/api/v1/creations/") && p.ends_with("/review") => {
                        let cid = p
                            .trim_start_matches("/api/v1/creations/")
                            .trim_end_matches("/review");
                        let req: ReviewDecisionRequest =
                            serde_json::from_str(body_str).expect("valid ReviewDecisionRequest");
                        st.review_decisions_received
                            .lock()
                            .unwrap()
                            .push((cid.to_string(), req.clone()));
                        let reviewed = {
                            let mut queue = st.review_queue.lock().unwrap();
                            if let Some(idx) = queue.iter().position(|c| c.creation_id == cid) {
                                let mut item = queue.remove(idx);
                                item.cloud_revision += 1;
                                match req.decision {
                                    ReviewDecision::Approve => {
                                        item.review_status = CreationReviewStatus::Published;
                                        item.visibility =
                                            req.visibility.unwrap_or(CreationVisibility::Public);
                                        item.has_live_published_version = true;
                                        item.published_version = Some(1);
                                    }
                                    ReviewDecision::Reject => {
                                        item.review_status = CreationReviewStatus::Rejected;
                                        item.rejection_reason = req.reason.clone();
                                    }
                                }
                                Some(item)
                            } else {
                                None
                            }
                        };
                        if let Some(item) = reviewed {
                            {
                                let mut creations = st.creations.lock().unwrap();
                                if let Some(slot) =
                                    creations.iter_mut().find(|c| c.creation_id == cid)
                                {
                                    *slot = item.clone();
                                }
                            }
                            ("200 OK", serde_json::to_string(&item).unwrap())
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("DELETE", p) if p.starts_with("/api/v1/creations/") => {
                        let cid = p.trim_start_matches("/api/v1/creations/");
                        let deleted = {
                            let mut guard = st.creations.lock().unwrap();
                            if let Some(item) = guard.iter_mut().find(|c| c.creation_id == cid) {
                                item.deleted_at_ms = Some(1_700_000_099_000);
                                item.cloud_revision += 1;
                                Some(item.clone())
                            } else {
                                None
                            }
                        };
                        if let Some(item) = deleted {
                            st.review_queue
                                .lock()
                                .unwrap()
                                .retain(|c| c.creation_id != cid);
                            st.prune_catalog_item(cid);
                            ("200 OK", serde_json::to_string(&item).unwrap())
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("GET", p) if p.starts_with("/api/v1/creations/") && p.ends_with("/audit") => {
                        let delay = st.audit_delay_ms.load(Ordering::Relaxed);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        let cid = p
                            .trim_start_matches("/api/v1/creations/")
                            .trim_end_matches("/audit");
                        let resp = CreationAuditListResponse {
                            items: vec![CreationReviewAuditDto {
                                audit_id: 1,
                                creation_id: cid.to_string(),
                                actor_user_id: 1001,
                                from_status: CreationReviewStatus::Draft,
                                to_status: CreationReviewStatus::PendingReview,
                                action: "submit_review".to_string(),
                                reason: None,
                                notes: None,
                                validation_errors: Vec::new(),
                                created_at_ms: 1_700_000_005_000,
                            }],
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                    ("GET", p) if p.starts_with("/api/v1/bots/profile/") => {
                        st.bot_profile_requests.fetch_add(1, Ordering::Relaxed);
                        if st.serve_unstarted_creator_bot_profile.load(Ordering::Relaxed) {
                            let prof = BotPublicProfileDto {
                                bot: BotRegistryEntry {
                                    bot_user_id: CREATOR_STUDIO_BOT_USER_ID,
                                    username: "CreatorStudioBot".to_string(),
                                    display_name: "Creator Studio".to_string(),
                                    description: "Create books, music, and games in cloud."
                                        .to_string(),
                                    is_official: true,
                                    ..Default::default()
                                },
                                has_started_chat: false,
                                ..Default::default()
                            };
                            ("200 OK", serde_json::to_string(&prof).unwrap())
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("GET", p) if is_detail_get => {
                        let cid = p.trim_start_matches("/api/v1/creations/");
                        let override_hit = st.detail_overrides.lock().unwrap().get(cid).cloned();
                        let found = override_hit.or_else(|| {
                            st.creations
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|c| c.creation_id == cid)
                                .cloned()
                        });
                        st.detail_requests_started.fetch_add(1, Ordering::SeqCst);
                        let delay = st.detail_delay_ms.load(Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        if let Some(dto) = found {
                            ("200 OK", serde_json::to_string(&dto).unwrap())
                        } else {
                            ("404 Not Found", r#"{"error":"not_found","message":"missing"}"#.to_string())
                        }
                    }
                    ("GET", "/api/v1/catalog/appstore") => {
                        st.recorded_catalog_requests
                            .lock()
                            .unwrap()
                            .push(raw_path.to_string());
                        let q_lower = parse_query_param(raw_path, "q")
                            .map(|s| s.to_ascii_lowercase());
                        let mut resp = st.catalog_appstore.lock().unwrap().clone();
                        if let Some(q) = q_lower {
                            resp.featured.retain(|i| i.title.to_ascii_lowercase().contains(&q));
                            resp.apps.retain(|i| i.title.to_ascii_lowercase().contains(&q));
                            resp.cross_vertical
                                .retain(|i| i.title.to_ascii_lowercase().contains(&q));
                        }
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                    ("GET", "/api/v1/catalog") => {
                        st.recorded_catalog_requests
                            .lock()
                            .unwrap()
                            .push(raw_path.to_string());
                        let kind_param = parse_query_param(raw_path, "kind").unwrap_or_default();
                        let q_lower = parse_query_param(raw_path, "q")
                            .map(|s| s.to_ascii_lowercase());
                        let mut items = match kind_param.as_str() {
                            "book" => st.catalog_books.lock().unwrap().clone(),
                            "music" => st.catalog_music.lock().unwrap().clone(),
                            "game" => st.catalog_games.lock().unwrap().clone(),
                            _ => Vec::new(),
                        };
                        if let Some(q) = q_lower {
                            items.retain(|i| i.title.to_ascii_lowercase().contains(&q));
                        }
                        let total = items.len();
                        (
                            "200 OK",
                            serde_json::to_string(&CatalogQueryResponse { items, total }).unwrap(),
                        )
                    }
                    ("GET", p) if p.starts_with("/api/v1/catalog/") => {
                        st.recorded_catalog_requests
                            .lock()
                            .unwrap()
                            .push(raw_path.to_string());
                        ("200 OK", r#"{"items":[],"total":0}"#.to_string())
                    }
                    _ => ("200 OK", "{}".to_string()),
                };

                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
                    json_body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
                if is_creations_sync_get {
                    st.creations_sync_responses_served
                        .fetch_add(1, Ordering::SeqCst);
                }
                if is_review_queue_get {
                    st.review_queue_responses_served
                        .fetch_add(1, Ordering::SeqCst);
                }
                if is_detail_get {
                    st.detail_responses_served.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });
    format!("http://{addr}")
}

async fn wait_until(
    app: &mut AsyncApp,
    max_iters: usize,
    desc: &str,
    mut check: impl FnMut(&mut AsyncApp) -> bool,
) {
    for _ in 0..max_iters {
        if check(app) {
            return;
        }
        app.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    panic!(
        "wait_until timed out after {max_iters} iters ({}ms): {desc}",
        max_iters * 20
    );
}

#[test]
fn test_m5_guardrails_icons_nav_rail_chat_sidebar_and_game_polish() {
    // 1. Hard guardrail: IconName::ALL stays 64; all 7 hub SVGs are in IconName::EXTRA and load cleanly.
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL MUST remain 64");
    let extra_hub_icons = [
        IconName::HubBooks,
        IconName::HubMusic,
        IconName::HubGames,
        IconName::HubAppStore,
        IconName::HubCreations,
        IconName::HubPublish,
        IconName::HubSparkles,
    ];
    let src = TelegramAssetSource::new();
    let listed_icons = src.list("icons").expect("list icons");
    for icon in extra_hub_icons {
        assert!(
            IconName::EXTRA.contains(&icon),
            "{icon:?} must be registered exclusively in IconName::EXTRA"
        );
        assert!(
            !IconName::ALL.contains(&icon),
            "{icon:?} must NEVER be added to IconName::ALL"
        );
        let bytes = src
            .load(icon.path())
            .expect("load icon")
            .expect("icon bytes present");
        let svg_text = std::str::from_utf8(bytes.as_ref()).expect("valid UTF-8 SVG");
        assert!(
            svg_text.contains("<svg") && svg_text.contains("currentColor"),
            "SVG {} must be monochrome currentColor markup",
            icon.path()
        );
        assert!(
            listed_icons.iter().any(|entry| entry.as_ref() == icon.path()),
            "list(\"icons\") must expose EXTRA icon {}",
            icon.path()
        );
    }

    // 2. NavRail & ChatSidebar guardrails.
    let nav = NavRail::new();
    assert_eq!(nav.items.len(), 12, "9 legacy destinations + upstream Browser + CodeScene + Community");
    assert_eq!(NAV_RAIL_WIDTH, 52.0);
    assert_eq!(DEFAULT_WIDTH, 288.0);
    assert_eq!(COLLAPSED_WIDTH, 64.0);
    assert_eq!(ROW_HEIGHT, 62.0);

    let mut sb = ChatSidebar::new();
    assert!(!sb.has_leading_app_sidebar, "default must be false");
    assert!(sb.dialogs.is_empty(), "truthful-empty default dialogs");
    sb.set_has_leading_app_sidebar(true);
    sb.configure_for_cloud_workspace("http://127.0.0.1:8080");
    assert!(
        sb.has_leading_app_sidebar,
        "configure_for_cloud_workspace must preserve has_leading_app_sidebar"
    );

    // 3. Bare tg:// auto-link parsing in both inline entities and text_card spans (EN + CJK + case-insensitive E2).
    let blocks = parse_rich_blocks("Open tg://apple/books/cr_42?action=read now!", &[]);
    let has_tg_link = blocks.iter().any(|b| match b {
        RichBlock::Paragraph(segs) => segs
            .iter()
            .any(|s| s.flags.link_url.as_deref() == Some("tg://apple/books/cr_42?action=read")),
        _ => false,
    });
    assert!(
        has_tg_link,
        "bare tg:// URI must auto-link in stage/entities.rs"
    );
    let cjk_blocks = parse_rich_blocks("推荐：tg://apple/books/cr_42。请点击查看", &[]);
    let has_cjk_entity_link = cjk_blocks.iter().any(|b| match b {
        RichBlock::Paragraph(segs) => segs
            .iter()
            .any(|s| s.flags.link_url.as_deref() == Some("tg://apple/books/cr_42")),
        _ => false,
    });
    assert!(
        has_cjk_entity_link,
        "CJK trailing punctuation after bare tg:// URI must trim cleanly in stage/entities.rs"
    );
    let colon_blocks = parse_rich_blocks("Open tg://apple/books/cr_42:", &[]);
    let has_colon_entity_link = colon_blocks.iter().any(|b| match b {
        RichBlock::Paragraph(segs) => segs
            .iter()
            .any(|s| s.flags.link_url.as_deref() == Some("tg://apple/books/cr_42")),
        _ => false,
    });
    assert!(
        has_colon_entity_link,
        "ASCII ':' trailing punctuation after bare tg:// URI must trim cleanly in stage/entities.rs"
    );

    let spans = parse_markdown_spans("Check tg://apple/games/cr_99!");
    assert!(
        spans.iter().any(|s| {
            matches!(&s.kind, RichSpanKind::Link(u) if u == "tg://apple/games/cr_99")
                && s.text == "tg://apple/games/cr_99"
        }),
        "bare tg:// URI must auto-link in text_card.rs"
    );
    let cjk_spans = parse_markdown_spans("推荐：tg://apple/books/cr_42。");
    assert!(
        cjk_spans.iter().any(|s| {
            matches!(&s.kind, RichSpanKind::Link(u) if u == "tg://apple/books/cr_42")
                && s.text == "tg://apple/books/cr_42"
        }),
        "CJK trailing '。' must trim cleanly in text_card.rs"
    );
    let upper_spans = parse_markdown_spans("试玩 TG://apple/music/cr_88！马上开始");
    assert!(
        upper_spans.iter().any(|s| {
            matches!(&s.kind, RichSpanKind::Link(u) if u == "TG://apple/music/cr_88")
        }),
        "case-insensitive TG:// with CJK punctuation must auto-link in text_card.rs"
    );
    let colon_spans = parse_markdown_spans("Open tg://apple/books/cr_42:");
    assert!(
        colon_spans.iter().any(|s| {
            matches!(&s.kind, RichSpanKind::Link(u) if u == "tg://apple/books/cr_42")
        }),
        "ASCII ':' trailing punctuation must trim cleanly in text_card.rs"
    );

    // 3b. E1 candidate detector: catches malformed/XSS tg://apple/* & creation:open:* without false positives.
    assert!(is_apple_deep_link_candidate("tg://apple/books/<script>alert(1)</script>"));
    assert!(is_apple_deep_link_candidate("tg://apple/books/../../etc/passwd"));
    assert!(is_apple_deep_link_candidate("TG:///APPLE?tab=javascript:alert(1)"));
    assert!(is_apple_deep_link_candidate("creation:open:book:../secret"));
    assert!(!is_apple_deep_link_candidate("apple"));
    assert!(!is_apple_deep_link_candidate("https://apple.com"));
    assert!(!is_apple_deep_link_candidate("tg://resolve?domain=foo"));
    assert!(!is_apple_deep_link_candidate("tg://applesauce/1"));

    // 3c. EditableFieldState::split_at_cursor UTF-8 CJK + emoji/ZWJ boundary safety.
    for (sample, mid_char, expected_left, expected_right) in [
        ("", 0, "", ""),
        ("Hello", 2, "He", "llo"),
        ("星际图书", 2, "星际", "图书"),
        ("🚀👨‍👩‍👧‍👦中文!", 1, "🚀", "👨‍👩‍👧‍👦中文!"),
    ] {
        let mut f = EditableFieldState::new(sample);
        f.cursor_char = 0;
        assert_eq!(f.split_at_cursor(), ("", sample));
        f.cursor_char = mid_char;
        assert_eq!(f.split_at_cursor(), (expected_left, expected_right));
        f.cursor_char = 999;
        assert_eq!(f.split_at_cursor(), (sample, ""));
    }

    // 4. Challenger 2 draft-only polish: GridPuzzle target_grid with 64 leading empty rows falls back safely.
    let mut bad_target = vec![Vec::<i32>::new(); 64];
    bad_target.push(vec![1, 2, 0, 3]);
    let payload = GamePayload {
        developer: "Dev".to_string(),
        category: GameCategory::Puzzle,
        controls_hint: "Arrows".to_string(),
        rules_markdown: "Slide tiles".to_string(),
        engine_kind: GameEngineKind::GridPuzzle,
        supports_leaderboard: true,
        estimated_session_mins: 3,
        spec: GameSpec::GridPuzzle {
            rows: 2,
            cols: 2,
            levels: vec![GridPuzzleLevel {
                level_id: "l1".to_string(),
                title: "L1".to_string(),
                initial_grid: vec![vec![1, 2], vec![0, 3]],
                target_grid: Some(bad_target),
                max_moves: Some(10),
                hint: None,
            }],
        },
        achievements: Vec::new(),
    };
    let mut rt = MiniGameRuntime::from_payload(&payload, 42);
    rt.apply_input(GameInput::Start);
    rt.apply_input(GameInput::ClickCell { row: 1, col: 1 });
    assert_eq!(
        rt.snapshot().status,
        GameStatus::Won,
        "sliding 3 left into canonical [[1,2],[3,0]] target must win instead of collapsing to all-zero"
    );
}

#[test]
fn test_m5_gpui_sidebar_navigation_theme_collapse_and_quick_drawer() {
    run_in_headless_subprocess(
        "test_m5_gpui_sidebar_navigation_theme_collapse_and_quick_drawer",
        true,
    );
}

fn body_sidebar_navigation_theme_collapse_and_quick_drawer(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));

    // Unauthenticated guard: deep-link returns false without mutating active_panel.
    let unauth_routed = shell.update(cx, |s, cx| {
        assert!(!s.is_authenticated);
        assert!(s.sidebar.read(cx).has_leading_app_sidebar);
        s.handle_apple_deep_link("tg://apple/books/cr_unauth", cx)
    });
    assert!(!unauth_routed);
    assert_eq!(shell.read(cx).active_panel, ActiveWorkspacePanel::Chats);

    // Authenticate and seed a live dialog + active stage chat.
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.sidebar.update(cx, |sb, _| {
            sb.dialogs.push(DialogItemModel {
                id: 5001,
                peer_name: "Alice Chat".to_string(),
                avatar_initials: "AC".to_string(),
                avatar_color: 0x007AFF,
                last_message: "Hello".to_string(),
                timestamp: "10:00".to_string(),
                unread_count: 3,
                is_muted: false,
                is_pinned: false,
                is_online: true,
                typing_indicator: None,
                delivery_status: DialogDeliveryStatus::Read,
                folder: "All".to_string(),
            });
            sb.select_dialog(5001);
        });
        s.stage.update(cx, |st, _| {
            st.active_chat_id = Some(5001);
            st.chat_title = "Alice Chat".to_string();
        });
    });

    // Switch across all 6 destinations and verify distinct inspect_state active_panel & state_hash.
    let transitions = [
        (AppleSidebarSection::Chats, "Chats"),
        (AppleSidebarSection::Books, "ContentHub(Books)"),
        (AppleSidebarSection::Music, "ContentHub(Music)"),
        (AppleSidebarSection::Games, "ContentHub(Games)"),
        (AppleSidebarSection::AppStore, "ContentHub(AppStore)"),
        (AppleSidebarSection::MyCreations, "ContentHub(MyCreations)"),
    ];
    let mut seen_hashes = std::collections::HashSet::new();
    for (sec, expected_panel) in transitions {
        shell.update(cx, |s, cx| {
            s.apple_sidebar.update(cx, |sb, scx| sb.select_section(sec, scx));
        });
        let tree = shell.read(cx).inspect_state(cx);
        assert_eq!(tree.window.active_panel, expected_panel);
        assert!(
            seen_hashes.insert(tree.state_hash.clone()),
            "state_hash must be unique for {expected_panel}"
        );
        // ChatSidebar and ConversationStage remain 100% preserved across ContentHub switches!
        assert_eq!(shell.read(cx).sidebar.read(cx).dialogs.len(), 1);
        assert_eq!(shell.read(cx).stage.read(cx).active_chat_id, Some(5001));
    }

    // Collapse / expand AppleAppSidebar (204px <-> 68px) and toggle Quick Creations Drawer.
    assert_eq!(
        shell.read(cx).apple_sidebar.read(cx).effective_width(),
        APPLE_EXPANDED_WIDTH
    );
    shell.update(cx, |s, cx| {
        s.apple_sidebar.update(cx, |sb, scx| sb.toggle_collapsed(scx));
    });
    assert_eq!(
        shell.read(cx).apple_sidebar.read(cx).effective_width(),
        APPLE_COLLAPSED_WIDTH
    );
    shell.update(cx, |s, cx| {
        s.apple_sidebar
            .update(cx, |sb, scx| sb.toggle_creations_drawer(scx));
    });
    assert!(shell.read(cx).apple_sidebar.read(cx).creations_drawer_open);
    assert!(shell.read(cx).content_hub.read(cx).quick_drawer_open);

    // Theme & i18n propagation across all 4 modes + zh-CN / en.
    for mode in [
        TdesktopThemeMode::Night,
        TdesktopThemeMode::Tinted,
        TdesktopThemeMode::Day,
        TdesktopThemeMode::Classic,
    ] {
        shell.update(cx, |s, cx| {
            s.apply_theme_selection(mode, None, None, cx);
        });
        assert_eq!(shell.read(cx).apple_sidebar.read(cx).palette.mode, mode);
        assert_eq!(shell.read(cx).content_hub.read(cx).palette.mode, mode);
    }
    shell.update(cx, |s, cx| {
        s.update_preferences(cx, |p| p.localization.language_code = "en-US".to_string());
    });
    assert_eq!(shell.read(cx).apple_sidebar.read(cx).language_id, "en");
    assert_eq!(shell.read(cx).content_hub.read(cx).language_id, "en");
    shell.update(cx, |s, cx| {
        s.update_preferences(cx, |p| p.localization.language_code = "zh-CN".to_string());
    });
    assert_eq!(shell.read(cx).apple_sidebar.read(cx).language_id, "zh-CN");
    assert_eq!(shell.read(cx).content_hub.read(cx).language_id, "zh-CN");

    // Non-ContentHub transitions from Books (Settings, Calls, TaskCenter, McpTools) sync AppleAppSidebar -> Chats.
    for dest in [
        NavDestination::Settings,
        NavDestination::Calls,
        NavDestination::TaskCenter,
        NavDestination::McpTools,
    ] {
        shell.update(cx, |s, cx| {
            s.switch_to_content_hub(ContentHubSection::Books, cx);
            assert_eq!(
                s.apple_sidebar.read(cx).active_section,
                AppleSidebarSection::Books
            );
            s.activate_nav_destination(dest, cx);
        });
        assert_eq!(
            shell.read(cx).apple_sidebar.read(cx).active_section,
            AppleSidebarSection::Chats,
            "AppleAppSidebar must sync to Chats when leaving Books for {dest:?}"
        );
    }
}

#[test]
fn test_m5_gpui_publish_review_modal_and_reviewer_queue() {
    run_in_headless_subprocess("test_m5_gpui_publish_review_modal_and_reviewer_queue", false);
}

async fn body_publish_review_modal_and_reviewer_queue(mut app: AsyncApp) {
    let invalid_draft = sample_book_creation(
        "cr_invalid_1",
        1001,
        "Short",
        "too short",
        "tiny body",
        CreationReviewStatus::Draft,
        2,
    );
    let valid_draft = sample_book_creation(
        "cr_valid_1",
        1001,
        "Starlight Atlas",
        "An expansive sci-fi novel exploring deep space navigation.",
        "Chapter one begins aboard the orbital observatory as distant beacons ignite across the frontier of charted space.",
        CreationReviewStatus::Draft,
        3,
    );
    let pending_for_queue = sample_book_creation(
        "cr_queue_1",
        1002,
        "Quantum Harmonies",
        "A lyrical novel about acoustic engineering across star systems.",
        "Every resonance chamber carried the quiet memory of melodies composed centuries before the colony ship left orbit.",
        CreationReviewStatus::PendingReview,
        4,
    );
    let pending_for_unlisted = sample_book_creation(
        "cr_queue_2",
        1002,
        "Hidden Garden",
        "A private anthology shared strictly through unlisted links.",
        "Through glass corridors and misty conservatories, rare botanical specimens bloomed in quiet symmetry under artificial sun.",
        CreationReviewStatus::PendingReview,
        5,
    );
    let pending_for_reject = sample_book_creation(
        "cr_queue_3",
        1002,
        "Needs Revision",
        "A draft submission that requires clearer chapter pacing.",
        "The narrator wandered through several incomplete scenes waiting for an editor to request stronger narrative continuity.",
        CreationReviewStatus::PendingReview,
        6,
    );
    let mock_state = Arc::new(MockM5State::new(
        false,
        vec![invalid_draft.clone(), valid_draft.clone()],
        vec![pending_for_queue, pending_for_unlisted, pending_for_reject],
    ));

    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");

    // 1. Login as non-admin -> HTTP 403 on review-queue degrades silently to Forbidden without toast spam.
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
    });
    wait_until(
        &mut app,
        60,
        "initial login sync: reviewer_access == Forbidden and my_creations.len() == 2",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub.read(cx).publish_modal.reviewer_access
                        == ReviewerAccessState::Forbidden
                        && s.content_hub.read(cx).my_creations.len() == 2
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (init_access, init_count, has_inv, has_val) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.publish_modal.reviewer_access,
                h.my_creations.len(),
                h.find_my_creation("cr_invalid_1").is_some(),
                h.find_my_creation("cr_valid_1").is_some(),
            )
        })
        .unwrap();
    assert_eq!(init_access, ReviewerAccessState::Forbidden);
    assert_eq!(init_count, 2);
    assert!(has_inv);
    assert!(has_val);

    // Re-probe 403 (even with user_initiated=true) and verify silent Forbidden degradation with zero 403 error/toast.
    let _ = shell.update(&mut app, |s, cx| {
        s.window_runtime.controller.clear_all_toasts();
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.reviewer_access = ReviewerAccessState::Unknown;
            h.publish_modal.reviewer_error = None;
        });
        s.probe_reviewer_queue(cx, true);
    });
    wait_until(
        &mut app,
        50,
        "user-initiated 403 review-queue re-probe sets reviewer_access == Forbidden",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub.read(cx).publish_modal.reviewer_access
                        == ReviewerAccessState::Forbidden
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (access, reviewer_err, toast_text) = shell
        .read_with(&app, |s, cx| {
            let pm = &s.content_hub.read(cx).publish_modal;
            (
                pm.reviewer_access,
                pm.reviewer_error.clone(),
                s.window_runtime
                    .controller
                    .current_toast
                    .as_ref()
                    .map(|t| t.text.clone()),
            )
        })
        .unwrap();
    assert_eq!(access, ReviewerAccessState::Forbidden);
    assert!(
        reviewer_err.is_none(),
        "HTTP 403 must degrade silently without populating reviewer_error: {reviewer_err:?}"
    );
    if let Some(ref txt) = toast_text {
        let lower = txt.to_ascii_lowercase();
        assert!(
            !lower.contains("403")
                && !lower.contains("forbidden")
                && !lower.contains("reviewer")
                && !txt.contains("审核"),
            "HTTP 403 on review-queue probe must NOT emit a 403/reviewer error toast, got: {txt}"
        );
    }

    // 2. Open PublishReviewModal on invalid draft -> live validate_creation_for_review() errors update live.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(invalid_draft.clone(), cx);
    });
    let (modal_open, errs_before, active_modal_id) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.is_publish_review_modal_open(),
                h.publish_modal.validation_errors.clone(),
                s.inspect_state(cx).active_modal,
            )
        })
        .unwrap();
    assert!(modal_open);
    assert_eq!(active_modal_id.as_deref(), Some("publish_review_modal"));
    assert!(
        !errs_before.is_empty(),
        "invalid draft must show live validate_creation_for_review() errors"
    );

    // 3. Test 409 revision_conflict surfacing on stale expected_revision.
    let _ = shell.update(&mut app, |s, cx| {
        let stale_req = UpdateCreationRequest {
            expected_revision: Some(999),
            title: Some("Stale Edit".to_string()),
            ..Default::default()
        };
        s.save_creation_draft_fenced("cr_invalid_1".to_string(), stale_req, cx);
    });
    wait_until(
        &mut app,
        50,
        "409 revision_conflict surfaces in publish_modal.server_error",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub
                        .read(cx)
                        .publish_modal
                        .server_error
                        .as_deref()
                        .is_some_and(|e| e.contains("409") || e.contains("冲突") || e.contains("Stale"))
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (srv_err_409, submitting_409) = shell
        .read_with(&app, |s, cx| {
            let pm = &s.content_hub.read(cx).publish_modal;
            (pm.server_error.clone(), pm.is_submitting)
        })
        .unwrap();
    let err_409 = srv_err_409.expect("409 revision_conflict must populate publish_modal.server_error");
    assert!(
        err_409.contains("409") || err_409.contains("冲突") || err_409.contains("Stale"),
        "unexpected 409 error: {err_409}"
    );
    assert!(!submitting_409);

    // 4. Test 422 validation_errors surfacing from server on submit.
    let _ = shell.update(&mut app, |s, cx| {
        s.submit_creation_for_review_fenced(
            "cr_invalid_1".to_string(),
            None,
            gpui_telegram_proto::creations::SubmitReviewRequest::default(),
            cx,
        );
    });
    wait_until(
        &mut app,
        50,
        "422 validation_errors from server surface in publish_modal.validation_errors",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub
                        .read(cx)
                        .publish_modal
                        .validation_errors
                        .iter()
                        .any(|e| e.contains("10 characters"))
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (val_errs_422, srv_err_422, submitting_422) = shell
        .read_with(&app, |s, cx| {
            let pm = &s.content_hub.read(cx).publish_modal;
            (
                pm.validation_errors.clone(),
                pm.server_error.clone(),
                pm.is_submitting,
            )
        })
        .unwrap();
    assert!(
        val_errs_422.iter().any(|e| e.contains("10 characters")),
        "expected server 422 validation error, got {val_errs_422:?}"
    );
    assert!(srv_err_422.is_some());
    assert!(!submitting_422);

    // 5. Open PublishReviewModal on valid draft -> 0 live errors -> submit -> PendingReview + audit trail.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(valid_draft.clone(), cx);
        assert!(s.content_hub.read(cx).publish_modal.validation_errors.is_empty());
        let submit_req = s.content_hub.read(cx).publish_modal.build_submit_request();
        s.submit_creation_for_review_fenced("cr_valid_1".to_string(), None, submit_req, cx);
    });
    wait_until(
        &mut app,
        50,
        "valid draft submit transitions cr_valid_1 to PendingReview and loads audit_trail",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.find_my_creation("cr_valid_1")
                        .is_some_and(|c| c.review_status == CreationReviewStatus::PendingReview)
                        && !h.publish_modal.audit_trail.is_empty()
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (sub_status, sub_rev, sub_audit, sub_submitting, sub_err) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            let c = h.find_my_creation("cr_valid_1");
            (
                c.map(|x| x.review_status),
                c.map(|x| x.cloud_revision),
                h.publish_modal.audit_trail.clone(),
                h.publish_modal.is_submitting,
                h.publish_modal.server_error.clone(),
            )
        })
        .unwrap();
    assert_eq!(sub_status, Some(CreationReviewStatus::PendingReview));
    assert_eq!(sub_rev, Some(4));
    assert!(!sub_audit.is_empty(), "audit_trail must be populated after submit");
    assert_eq!(sub_audit[0].action, "submit_review");
    assert_eq!(sub_audit[0].to_status, CreationReviewStatus::PendingReview);
    assert!(!sub_submitting);
    assert!(sub_err.is_none());

    // 6. Enable admin reviewer access -> 200 Review Queue -> reject blank blocked, valid reject & approve Public/Unlisted succeed.
    mock_state.admin_allowed.store(true, Ordering::Relaxed);
    let _ = shell.update(&mut app, |s, cx| {
        s.open_reviewer_queue_modal(cx);
    });
    wait_until(
        &mut app,
        50,
        "admin open_reviewer_queue_modal loads reviewer_access == Allowed and 3 queue items",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let pm = &s.content_hub.read(cx).publish_modal;
                    pm.reviewer_access == ReviewerAccessState::Allowed && pm.review_queue.len() == 3
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (q_access, q_len, q_tab_open, q_rev_err) = shell
        .read_with(&app, |s, cx| {
            let pm = &s.content_hub.read(cx).publish_modal;
            (
                pm.reviewer_access,
                pm.review_queue.len(),
                pm.show_reviewer_queue_tab,
                pm.reviewer_error.clone(),
            )
        })
        .unwrap();
    assert_eq!(q_access, ReviewerAccessState::Allowed);
    assert_eq!(q_len, 3);
    assert!(q_tab_open);
    assert!(q_rev_err.is_none());

    // Blank rejection reason MUST be rejected client-side without sending HTTP request.
    let blank_err = shell
        .update(&mut app, |s, cx| {
            s.content_hub.update(cx, |h, _| {
                h.publish_modal.reject_reason.set_text("   ");
                h.publish_modal.build_reject_request("en")
            })
        })
        .unwrap();
    assert!(blank_err.is_err());
    assert!(mock_state.review_decisions_received.lock().unwrap().is_empty());

    // Valid Reject + Approve(Public) + Approve(Unlisted).
    let _ = shell.update(&mut app, |s, cx| {
        let rej_req = s.content_hub.update(cx, |h, _| {
            h.publish_modal.reject_reason.set_text("Expand chapter 2 pacing");
            h.publish_modal.build_reject_request("en").expect("valid reject")
        });
        s.submit_review_decision_fenced("cr_queue_3".to_string(), rej_req, cx);
        let pub_req = s
            .content_hub
            .read(cx)
            .publish_modal
            .build_approve_request(CreationVisibility::Public);
        s.submit_review_decision_fenced("cr_queue_1".to_string(), pub_req, cx);
        let unl_req = s
            .content_hub
            .read(cx)
            .publish_modal
            .build_approve_request(CreationVisibility::Unlisted);
        s.submit_review_decision_fenced("cr_queue_2".to_string(), unl_req, cx);
    });
    wait_until(
        &mut app,
        60,
        "admin review decisions drain publish_modal.review_queue to empty",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub.read(cx).publish_modal.review_queue.is_empty()
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (q_empty, rev_err_after, rej_txt_after, submitting_after) = shell
        .read_with(&app, |s, cx| {
            let pm = &s.content_hub.read(cx).publish_modal;
            (
                pm.review_queue.is_empty(),
                pm.reviewer_error.clone(),
                pm.reject_reason.text.clone(),
                pm.is_submitting,
            )
        })
        .unwrap();
    assert!(q_empty, "review_queue must be empty after all 3 decisions");
    assert!(rev_err_after.is_none());
    assert!(rej_txt_after.is_empty());
    assert!(!submitting_after);
    let recorded = mock_state.review_decisions_received.lock().unwrap().clone();
    assert_eq!(recorded.len(), 3);
    assert!(recorded.iter().any(|(id, r)| {
        id == "cr_queue_3"
            && r.decision == ReviewDecision::Reject
            && r.reason.as_deref() == Some("Expand chapter 2 pacing")
    }));
    assert!(recorded.iter().any(|(id, r)| {
        id == "cr_queue_1"
            && r.decision == ReviewDecision::Approve
            && r.visibility == Some(CreationVisibility::Public)
    }));
    assert!(recorded.iter().any(|(id, r)| {
        id == "cr_queue_2"
            && r.decision == ReviewDecision::Approve
            && r.visibility == Some(CreationVisibility::Unlisted)
    }));
}

#[test]
fn test_m5_gpui_ime_isolation_escape_priority_and_shortcut_blocking() {
    run_in_headless_subprocess(
        "test_m5_gpui_ime_isolation_escape_priority_and_shortcut_blocking",
        true,
    );
}

fn body_ime_isolation_escape_priority_and_shortcut_blocking(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.set_inspector_open(true, cx);
        s.toggle_quick_creations_drawer(true, cx);
    });

    // 1. Quick Drawer search over Chats: CJK IME preedit + commit + ASCII + Backspace + Cmd+A/C/V -> zero leakage to composer.
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
        s.update_ime_marked_text("xing", cx);
        assert_eq!(s.active_text_and_preedit(cx).1.as_deref(), Some("xing"));
        s.insert_committed_text("星际", cx);
        s.insert_committed_text(" Atlas", cx);
        assert!(s.handle_non_char_key("backspace", false, cx));
        assert_eq!(s.content_hub.read(cx).quick_drawer_search.text, "星际 Atla");

        assert!(s.handle_shortcut_key("a", false, cx));
        assert!(s.content_hub.read(cx).quick_drawer_search.select_all);
        assert!(s.handle_shortcut_key("c", false, cx));
        // Headless Linux GPUI platform may stub OS clipboard as None; verify readback when supported.
        cx.write_to_clipboard(ClipboardItem::new_string("银河音乐".to_string()));
        if cx.read_from_clipboard().and_then(|c| c.text()).as_deref() == Some("银河音乐") {
            assert!(s.handle_shortcut_key("v", false, cx));
            assert_eq!(s.content_hub.read(cx).quick_drawer_search.text, "银河音乐");
        } else {
            s.insert_committed_text("银河音乐", cx);
            assert_eq!(s.content_hub.read(cx).quick_drawer_search.text, "银河音乐");
        }
        assert!(
            s.composer.read(cx).text().is_empty(),
            "Quick Drawer input MUST NEVER leak into CanonicalComposer"
        );
    });

    // 2. ContentHub(Books) catalog search + unfocused hub typing -> zero leakage to composer.
    shell.update(cx, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
        s.insert_committed_text("科幻小说 SciFi", cx);
        assert_eq!(s.content_hub.read(cx).catalog_search.text, "科幻小说 SciFi");
        s.content_hub.update(cx, |h, _| h.blur_input());
        s.insert_committed_text("should_not_leak", cx);
        assert!(
            s.composer.read(cx).text().is_empty(),
            "Unfocused typing inside ContentHub MUST NEVER leak into CanonicalComposer"
        );
    });

    // 3. PublishReviewModal CJK + multiline + Tab cycling + background shortcut blocking.
    let draft = sample_book_creation(
        "cr_ime_1",
        1001,
        "Initial",
        "Initial summary text long enough.",
        "Readable chapter content that exceeds fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    shell.update(cx, |s, cx| {
        s.open_publish_review_modal_for_creation(draft, cx);
        assert!(s.is_publish_review_modal_active(cx));
        assert_eq!(
            s.content_hub.read(cx).publish_modal.focused_field,
            Some(PublishModalField::Title)
        );
        assert!(s.handle_shortcut_key("a", false, cx));
        s.insert_committed_text("中文标题 Title", cx);
        assert_eq!(s.content_hub.read(cx).publish_modal.title.text, "中文标题 Title");

        // Tab cycles Title -> Subtitle -> AuthorDisplayName -> Summary
        assert!(s.handle_non_char_key("tab", false, cx));
        assert_eq!(
            s.content_hub.read(cx).publish_modal.focused_field,
            Some(PublishModalField::Subtitle)
        );
        assert!(s.handle_non_char_key("tab", false, cx));
        assert!(s.handle_non_char_key("tab", false, cx));
        assert_eq!(
            s.content_hub.read(cx).publish_modal.focused_field,
            Some(PublishModalField::Summary)
        );
        assert!(s.handle_non_char_key("enter", false, cx));
        assert!(s.content_hub.read(cx).publish_modal.summary.text.contains('\n'));
        assert!(s.composer.read(cx).text().is_empty());

        // Background shortcuts MUST be blocked while PublishReviewModal is open!
        assert!(s.handle_shortcut_key(",", false, cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books),
            "Cmd+, must NOT open Settings while PublishReviewModal is open"
        );
        assert!(s.handle_shortcut_key("i", false, cx));
        assert!(
            s.inspector.read(cx).is_open,
            "Cmd+I must NOT toggle Inspector while PublishReviewModal is open"
        );
        assert!(s.handle_shortcut_key("n", false, cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books),
            "Cmd+N must NOT switch to Chats while PublishReviewModal is open"
        );
        assert!(s.handle_shortcut_key("1", false, cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books),
            "Cmd+1 must NOT leave ContentHub while PublishReviewModal is open"
        );
    });

    // 4. Escape priority: PublishReviewModal -> Quick Creations Drawer -> ContextInspector.
    shell.update(cx, |s, cx| {
        assert!(s.is_publish_review_modal_active(cx));
        assert!(s.content_hub.read(cx).quick_drawer_open);
        assert!(s.inspector.read(cx).is_open);

        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.is_publish_review_modal_active(cx),
            "1st Escape must close PublishReviewModal"
        );
        assert!(
            s.content_hub.read(cx).quick_drawer_open,
            "Quick Drawer must still be open after 1st Escape"
        );
        assert!(
            s.inspector.read(cx).is_open,
            "Inspector must still be open after 1st Escape"
        );

        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.content_hub.read(cx).quick_drawer_open,
            "2nd Escape must close Quick Creations Drawer"
        );
        assert!(
            s.inspector.read(cx).is_open,
            "Inspector must still be open after 2nd Escape"
        );

        // C1-I7-7c: While on ContentHub(Books), unmounted Chats ContextInspector MUST NOT be mutated by Escape.
        assert!(
            !s.handle_non_char_key("escape", false, cx),
            "C1-I7-7c: Escape on ContentHub must not close unmounted Chats inspector"
        );
        assert!(
            s.inspector.read(cx).is_open,
            "C1-I7-7c: Hidden Chats ContextInspector must remain open until returning to Chats"
        );

        // Returning to Chats mounts ContextInspector; 3rd Escape closes it cleanly.
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.inspector.read(cx).is_open,
            "3rd Escape on Chats must close mounted ContextInspector"
        );
    });
}

#[test]
fn test_m5_gpui_deep_links_all_4_entry_points() {
    run_in_headless_subprocess("test_m5_gpui_deep_links_all_4_entry_points", true);
}

fn body_deep_links_all_4_entry_points(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
    });

    // Entry point 1: AppShell::handle_deep_link
    shell.update(cx, |s, cx| {
        assert!(s.handle_deep_link("tg://apple/books/cr_b1?tab=discover&action=read", cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
        );
        let h = s.content_hub.read(cx);
        assert_eq!(h.selected_creation_id.as_deref(), Some("cr_b1"));
        assert_eq!(h.active_sub_tab, "discover");
        assert_eq!(h.active_action_hint.as_deref(), Some("read"));
    });

    // Entry point 2a: handle_stage_bot_button InlineButtonType::Url (no external browser / toast fallback)
    shell.update(cx, |s, cx| {
        s.window_runtime.controller.clear_all_toasts();
        s.handle_stage_bot_button(
            10,
            InlineButtonType::Url("tg://apple/music/cr_m1?action=play".to_string()),
            cx,
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Music)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_m1")
        );
        assert!(
            s.window_runtime.controller.current_toast.is_none(),
            "InlineButtonType::Url Apple deep-link must NOT fall back to Copied URL toast"
        );
    });

    // Entry point 2b: handle_stage_bot_button InlineButtonType::Callback (legacy alias creation:open:*)
    shell.update(cx, |s, cx| {
        s.handle_stage_bot_button(
            11,
            InlineButtonType::Callback("creation:open:game:cr_g1".to_string()),
            cx,
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Games)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_g1")
        );
    });

    // Entry point 3: execute_hermes_inline_callback (GenerativeCard button -> must NOT send outgoing text)
    shell.update(cx, |s, cx| {
        let msg_before = s.stage.read(cx).messages.len();
        s.execute_hermes_inline_callback(
            "tg://apple/appstore/cr_a1?action=install".to_string(),
            cx,
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::AppStore)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_a1")
        );
        assert_eq!(s.stage.read(cx).messages.len(), msg_before);
    });

    // Entry point 4: RichTextAction::OpenUrl -> pending_open_url -> handle_stage_pending_actions (NO MediaViewer!)
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.handle_rich_text_action(
                RichTextAction::OpenUrl("tg://apple/creations/cr_c1".to_string()),
                stcx,
            );
        });
        s.handle_stage_pending_actions(cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::MyCreations)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_c1")
        );
        assert!(
            s.overlays.media_viewer_overlay.is_none(),
            "RichTextAction::OpenUrl on tg://apple/* must NOT open MediaViewer"
        );
    });

    // Entry point 5 (E1): Malformed/XSS tg://apple/* & creation:open:* payloads MUST NOT leak
    // into outgoing chat messages, Copied URL toasts, or MediaViewer!
    shell.update(cx, |s, cx| {
        let panel_before = s.active_panel;
        let msgs_before = s.stage.read(cx).messages.len();
        s.window_runtime.controller.clear_all_toasts();

        let xss_uri = "tg://apple/books/<script>alert(1)</script>".to_string();
        let traversal_alias = "creation:open:book:../secret".to_string();

        s.execute_hermes_inline_callback(xss_uri.clone(), cx);
        s.execute_hermes_inline_callback(traversal_alias.clone(), cx);
        s.handle_stage_bot_button(12, InlineButtonType::Url(xss_uri.clone()), cx);
        s.handle_stage_bot_button(13, InlineButtonType::Callback(traversal_alias), cx);
        s.stage.update(cx, |st, stcx| {
            st.handle_rich_text_action(RichTextAction::OpenUrl(xss_uri), stcx);
        });
        s.handle_stage_pending_actions(cx);

        assert_eq!(s.active_panel, panel_before);
        assert_eq!(
            s.stage.read(cx).messages.len(),
            msgs_before,
            "malformed/XSS tg://apple/* MUST NOT leak into outgoing messages"
        );
        assert!(
            s.window_runtime.controller.current_toast.is_none(),
            "malformed/XSS tg://apple/* MUST NOT trigger Copied URL fallback toast"
        );
        assert!(
            s.overlays.media_viewer_overlay.is_none(),
            "malformed/XSS tg://apple/* MUST NOT open MediaViewer"
        );
    });

    // Entry point 6 (E6): Routing a valid deep-link closes any open PublishReviewModal.
    let draft = sample_book_creation(
        "cr_dl_modal",
        1001,
        "Modal Draft",
        "Sufficient summary text for testing deep link close.",
        "Readable chapter body exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    shell.update(cx, |s, cx| {
        s.open_publish_review_modal_for_creation(draft, cx);
        assert!(s.is_publish_review_modal_active(cx));
        assert!(s.handle_apple_deep_link("tg://apple/books/cr_b9", cx));
        assert!(
            !s.is_publish_review_modal_active(cx),
            "handle_apple_deep_link must close any open PublishReviewModal"
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_b9")
        );
    });
}

#[test]
fn test_m5_gpui_ws_eviction_creator_bot_and_account_switch_wipe() {
    run_in_headless_subprocess(
        "test_m5_gpui_ws_eviction_creator_bot_and_account_switch_wipe",
        false,
    );
}

async fn body_ws_eviction_creator_bot_and_account_switch_wipe(mut app: AsyncApp) {
    let user_a_draft = sample_book_creation(
        "cr_ws_a1",
        2001,
        "Cloud Book V1",
        "Private draft owned by User A waiting for WS update.",
        "Sufficiently long readable chapter text so the cloud creation fixture is completely valid.",
        CreationReviewStatus::Draft,
        1,
    );
    let mock_state = Arc::new(MockM5State::new(
        true,
        vec![user_a_draft.clone()],
        Vec::new(),
    ));
    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");

    // 1. Authenticate User A, seed LocalStore public catalog + start headless MusicTransportEngine.
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(sample_auth(2001, "user_a"), cx);
        let pub_item = sample_public_book_item("cr_pub_evict", "Published Book");
        s.client
            .local_store
            .upsert_public_catalog_items(&[pub_item.clone()]);
        s.content_hub.update(cx, |h, _| {
            h.books_catalog.push(pub_item);
            h.music_engine.set_queue(vec![MusicTrackEntry {
                track_id: "trk_1".to_string(),
                creation_id: Some("cr_m_1".to_string()),
                title: "Synth Demo".to_string(),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                duration_ms: 10_000,
                sample_rate: 44_100,
                channels: 1,
                pcm_samples: Vec::new(),
                wav_bytes: Vec::new(),
                waveform_bars: Vec::new(),
                waveform_5bit: Vec::new(),
                lyrics: Vec::new(),
            }]);
            h.music_engine.play().expect("headless music play succeeds");
        });
    });
    wait_until(
        &mut app,
        60,
        "User A initial cloud sync loads cr_ws_a1",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub
                        .read(cx)
                        .find_my_creation("cr_ws_a1")
                        .is_some()
                })
                .unwrap_or(false)
        },
    )
    .await;
    let init_ws_c = shell
        .read_with(&app, |s, cx| {
            s.content_hub.read(cx).find_my_creation("cr_ws_a1").cloned()
        })
        .unwrap()
        .expect("cr_ws_a1 must sync on login");
    assert_eq!(init_ws_c.review_status, CreationReviewStatus::Draft);
    assert_eq!(init_ws_c.cloud_revision, 1);

    // 2. WS UserCreationChanged: foreign owner ignored; own owner triggers refresh & updates status.
    {
        let mut guard = mock_state.creations.lock().unwrap();
        guard[0].review_status = CreationReviewStatus::PendingReview;
        guard[0].cloud_revision = 5;
    }
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::UserCreationChanged {
                creation_id: "cr_foreign".to_string(),
                owner_user_id: 9999,
                kind: CreationKind::Book,
                review_status: CreationReviewStatus::Published,
                cloud_revision: 99,
            }),
            cx,
        );
        assert!(s.content_hub.read(cx).find_my_creation("cr_foreign").is_none());

        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::UserCreationChanged {
                creation_id: "cr_ws_a1".to_string(),
                owner_user_id: 2001,
                kind: CreationKind::Book,
                review_status: CreationReviewStatus::PendingReview,
                cloud_revision: 5,
            }),
            cx,
        );
    });
    wait_until(
        &mut app,
        50,
        "WS UserCreationChanged refreshes cr_ws_a1 to PendingReview rev 5",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub
                        .read(cx)
                        .find_my_creation("cr_ws_a1")
                        .is_some_and(|c| {
                            c.review_status == CreationReviewStatus::PendingReview
                                && c.cloud_revision == 5
                        })
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (ws_own, ws_foreign) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.find_my_creation("cr_ws_a1").cloned(),
                h.find_my_creation("cr_foreign").cloned(),
            )
        })
        .unwrap();
    let ws_own = ws_own.expect("cr_ws_a1 must remain present after WS update");
    assert_eq!(ws_own.review_status, CreationReviewStatus::PendingReview);
    assert_eq!(ws_own.cloud_revision, 5);
    assert!(ws_foreign.is_none());

    // 3. WS PublicCatalogChanged { action: "removed" } immediately evicts from LocalStore AND ContentHubWorkspace.
    let _ = shell.update(&mut app, |s, cx| {
        assert!(!s.client.local_store.load_public_catalog_items(None).is_empty());
        assert!(!s.content_hub.read(cx).books_catalog.is_empty());
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::PublicCatalogChanged {
                creation_id: "cr_pub_evict".to_string(),
                kind: CreationKind::Book,
                action: "removed".to_string(),
            }),
            cx,
        );
        assert!(
            s.client
                .local_store
                .load_public_catalog_items(None)
                .is_empty(),
            "WS removed must immediately evict item from LocalStore"
        );
        assert!(
            s.content_hub.read(cx).books_catalog.is_empty(),
            "WS removed must immediately evict item from ContentHubWorkspace"
        );
    });

    // 4. @CreatorStudioBot (UID 777_020) launch creates DM via POST /api/v1/chats and prefills slash commands.
    // Rapid double-invocation while in-flight must only emit ONE POST /api/v1/chats (Exp3 E4 #1).
    let _ = shell.update(&mut app, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::MyCreations, cx);
        s.open_creator_studio_bot_chat(Some("/newbook "), cx);
        s.open_creator_studio_bot_chat(Some("/newbook "), cx);
    });
    wait_until(
        &mut app,
        60,
        "@CreatorStudioBot DM creation switches active_panel to Chats and prefills /newbook ",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.active_panel == ActiveWorkspacePanel::Chats
                        && s.composer.read(cx).text() == "/newbook "
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (bot_panel, bot_sb_sec, bot_comp_txt, bot_comp_focused, bot_start_bar) = shell
        .read_with(&app, |s, cx| {
            (
                s.active_panel,
                s.apple_sidebar.read(cx).active_section,
                s.composer.read(cx).text().to_string(),
                s.composer.read(cx).is_input_focused,
                s.stage.read(cx).show_bot_start_bar,
            )
        })
        .unwrap();
    assert_eq!(bot_panel, ActiveWorkspacePanel::Chats);
    assert_eq!(bot_sb_sec, AppleSidebarSection::Chats);
    assert_eq!(bot_comp_txt, "/newbook ");
    assert!(bot_comp_focused);
    assert!(!bot_start_bar);
    assert_eq!(
        mock_state.created_bot_chat_peer_ids.lock().unwrap().as_slice(),
        &[CREATOR_STUDIO_BOT_USER_ID as u64]
    );
    // Reusing existing DM prefills /newmusic and /newgame without duplicate POST /api/v1/chats.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_creator_studio_bot_chat(Some("/newmusic "), cx);
        assert_eq!(s.composer.read(cx).text(), "/newmusic ");
        s.open_creator_studio_bot_chat(Some("/newgame "), cx);
        assert_eq!(s.composer.read(cx).text(), "/newgame ");
    });
    assert_eq!(
        mock_state.created_bot_chat_peer_ids.lock().unwrap().len(),
        1,
        "subsequent CreatorStudioBot launches must reuse local DM"
    );

    // 5. Account switch (User A -> User B) wipes creations cache, stops music engine, resets active_panel & reviewer_access.
    mock_state.creations.lock().unwrap().clear();
    let _ = shell.update(&mut app, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Music, cx);
        assert_eq!(
            s.content_hub.read(cx).music_engine.snapshot().state,
            MusicPlaybackState::Playing
        );
        s.apply_authenticated_session(sample_auth(2002, "user_b"), cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(
            s.apple_sidebar.read(cx).active_section,
            AppleSidebarSection::Chats
        );
        let h = s.content_hub.read(cx);
        assert_eq!(h.music_engine.snapshot().state, MusicPlaybackState::Stopped);
        assert!(h.music_engine.queue().is_empty());
        assert!(h.my_creations.is_empty());
        assert!(s.client.local_store.load_user_creations(None, None).is_empty());
    });

    // 6. Logout wipes all state and resets reviewer_access to Unknown.
    let _ = shell.update(&mut app, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Games, cx);
        s.logout(cx);
        assert!(!s.is_authenticated);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        let h = s.content_hub.read(cx);
        assert_eq!(h.publish_modal.reviewer_access, ReviewerAccessState::Unknown);
        assert!(h.my_creations.is_empty());
        assert!(h.music_engine.queue().is_empty());
        assert!(s.client.local_store.load_user_creations(None, None).is_empty());
    });
}

#[test]
fn test_m5_gpui_reg_f1_shell_observer_and_modal_close_ime_clear() {
    run_in_headless_subprocess(
        "test_m5_gpui_reg_f1_shell_observer_and_modal_close_ime_clear",
        false,
    );
}

async fn body_reg_f1_shell_observer_and_modal_close_ime_clear(mut app: AsyncApp) {
    let valid_draft = sample_book_creation(
        "cr_obs_1",
        1001,
        "Observer Atlas",
        "A rich sci-fi draft verifying AppShell reactivity on ContentHub updates.",
        "Chapter one begins aboard the orbital observatory as distant beacons ignite across charted space.",
        CreationReviewStatus::Draft,
        2,
    );
    let mock_state = Arc::new(MockM5State::new(
        false,
        vec![valid_draft.clone()],
        Vec::new(),
    ));
    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state);
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
    });
    wait_until(&mut app, 60, "R1 initial sync loads cr_obs_1", |a| {
        shell
            .read_with(a, |s, cx| {
                s.content_hub.read(cx).find_my_creation("cr_obs_1").is_some()
            })
            .unwrap_or(false)
    })
    .await;
    assert!(
        shell
            .read_with(&app, |s, cx| {
                s.content_hub.read(cx).find_my_creation("cr_obs_1").is_some()
            })
            .unwrap()
    );

    let notify_count = Arc::new(AtomicU64::new(0));
    let _shell_obs = app
        .update(|cx| {
            let c = notify_count.clone();
            cx.observe(&shell, move |_, _| {
                c.fetch_add(1, Ordering::SeqCst);
            })
        })
        .expect("register AppShell observer");
    let hub = shell
        .read_with(&app, |s, _| s.content_hub.clone())
        .unwrap();

    // 1. Async audit fetch notifies AppShell and populates audit_trail.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(valid_draft.clone(), cx);
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.audit_trail.clear();
        });
    });
    let before_audit = notify_count.load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.fetch_creation_audit_fenced("cr_obs_1".to_string(), cx);
    });
    wait_until(&mut app, 50, "R1 fetch_creation_audit_fenced loads audit_trail", |a| {
        shell
            .read_with(a, |s, cx| {
                !s.content_hub.read(cx).publish_modal.audit_trail.is_empty()
            })
            .unwrap_or(false)
    })
    .await;
    let audit_len = shell
        .read_with(&app, |s, cx| s.content_hub.read(cx).publish_modal.audit_trail.len())
        .unwrap();
    assert!(audit_len > 0, "audit_trail must be populated");
    assert!(
        notify_count.load(Ordering::SeqCst) > before_audit,
        "fetch_creation_audit_fenced MUST notify AppShell"
    );

    // 2. Direct ContentHub visibility pill click notifies AppShell via cx.observe(&self.content_hub).
    let before_vis = notify_count.load(Ordering::SeqCst);
    let _ = hub.update(&mut app, |h, hcx| {
        h.publish_modal.set_visibility(CreationVisibility::Unlisted);
        hcx.notify();
    });
    let vis_after = hub
        .read_with(&app, |h, _| h.publish_modal.visibility)
        .unwrap();
    assert_eq!(vis_after, CreationVisibility::Unlisted);
    assert!(
        notify_count.load(Ordering::SeqCst) > before_vis,
        "ContentHubWorkspace visibility toggle MUST trigger AppShell observer"
    );

    // 3. Direct Quick Drawer kind filter click notifies AppShell.
    let _ = shell.update(&mut app, |s, cx| {
        s.toggle_quick_creations_drawer(true, cx);
    });
    let before_filter = notify_count.load(Ordering::SeqCst);
    let _ = hub.update(&mut app, |h, hcx| {
        h.quick_drawer_kind_filter = Some(CreationKind::Book);
        hcx.notify();
    });
    let filter_after = hub
        .read_with(&app, |h, _| h.quick_drawer_kind_filter)
        .unwrap();
    assert_eq!(filter_after, Some(CreationKind::Book));
    assert!(
        notify_count.load(Ordering::SeqCst) > before_filter,
        "Quick Drawer kind filter MUST trigger AppShell observer"
    );

    // 4. Modal X close button emits ClosePublishReviewModal -> clears shell IME preedit & notifies AppShell.
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("zhong", cx);
    });
    assert_eq!(
        shell
            .read_with(&app, |s, _| s.ime.ime_preedit.clone())
            .unwrap()
            .as_deref(),
        Some("zhong")
    );
    let before_close = notify_count.load(Ordering::SeqCst);
    let _ = hub.update(&mut app, |h, hcx| {
        h.publish_modal.close();
        hcx.emit(ContentHubEvent::ClosePublishReviewModal);
        hcx.notify();
    });
    let (still_open, preedit_after) = shell
        .read_with(&app, |s, cx| {
            (
                s.content_hub.read(cx).publish_modal.is_open,
                s.ime.ime_preedit.clone(),
            )
        })
        .unwrap();
    assert!(!still_open, "PublishReviewModal must be closed");
    assert!(
        preedit_after.is_none(),
        "ClosePublishReviewModal must clear shell IME preedit, got {preedit_after:?}"
    );
    assert!(
        notify_count.load(Ordering::SeqCst) > before_close,
        "Modal close must trigger AppShell observer"
    );
}

#[test]
fn test_m5_gpui_reg_f2_b1_update_then_failed_submit_no_stale_409() {
    run_in_headless_subprocess(
        "test_m5_gpui_reg_f2_b1_update_then_failed_submit_no_stale_409",
        false,
    );
}

async fn body_reg_f2_b1_update_then_failed_submit_no_stale_409(mut app: AsyncApp) {
    let draft = sample_book_creation(
        "cr_rev_1",
        1001,
        "Starlight Atlas",
        "A valid sci-fi summary exceeding ten characters cleanly.",
        "Chapter one begins aboard the orbital observatory as distant beacons ignite across charted space.",
        CreationReviewStatus::Draft,
        3,
    );
    assert_eq!(draft.visibility, CreationVisibility::Private);
    let mock_state = Arc::new(MockM5State::new(false, vec![draft.clone()], Vec::new()));
    *mock_state.fail_next_submit_status.lock().unwrap() = Some(422);

    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
    });
    wait_until(&mut app, 60, "R2 initial sync loads cr_rev_1", |a| {
        shell
            .read_with(a, |s, cx| {
                s.content_hub.read(cx).find_my_creation("cr_rev_1").is_some()
            })
            .unwrap_or(false)
    })
    .await;

    // 1. Open modal (Private -> Public makes has_metadata_changes() == true), set review_notes, submit.
    let (upd1, sub1) = shell
        .update(&mut app, |s, cx| {
            s.open_publish_review_modal_for_creation(draft.clone(), cx);
            s.content_hub.update(cx, |h, _| {
                h.publish_modal.review_notes.set_text("keep me");
                assert!(h.publish_modal.has_metadata_changes());
                let upd = h.publish_modal.build_update_request();
                let sub = h.publish_modal.build_submit_request();
                h.publish_modal.is_submitting = true;
                (upd, sub)
            })
        })
        .unwrap();
    assert_eq!(upd1.as_ref().and_then(|u| u.expected_revision), Some(3));
    let _ = shell.update(&mut app, |s, cx| {
        s.submit_creation_for_review_fenced("cr_rev_1".to_string(), upd1, sub1, cx);
    });
    wait_until(
        &mut app,
        60,
        "R2 first submit fails with 422 after PUT bumps cloud_revision to 4",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let pm = &s.content_hub.read(cx).publish_modal;
                    !pm.is_submitting && pm.server_error.is_some()
                })
                .unwrap_or(false)
        },
    )
    .await;

    let (base_rev, my_rev, notes_after, val_errs, srv_err, submitting) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.publish_modal.base_creation.as_ref().map(|c| c.cloud_revision),
                h.find_my_creation("cr_rev_1").map(|c| c.cloud_revision),
                h.publish_modal.review_notes.text.clone(),
                h.publish_modal.validation_errors.clone(),
                h.publish_modal.server_error.clone(),
                h.publish_modal.is_submitting,
            )
        })
        .unwrap();
    assert_eq!(
        base_rev,
        Some(4),
        "base_creation MUST advance to R+1 (4) even when submit-review fails"
    );
    assert_eq!(my_rev, Some(4));
    assert_eq!(
        notes_after, "keep me",
        "upsert_my_creation must NOT clobber user's review_notes"
    );
    assert!(
        val_errs
            .iter()
            .any(|e| e.contains("Server policy requires extra chapter metadata")),
        "server 422 validation_errors must survive upsert_my_creation recompute, got {val_errs:?}"
    );
    assert!(srv_err.is_some());
    assert!(!submitting);

    // 2. Edit title and retry submit -> sends updated expected_revision = Some(4) -> succeeds (ZERO 409!).
    let (upd2, sub2) = shell
        .update(&mut app, |s, cx| {
            s.content_hub.update(cx, |h, _| {
                h.publish_modal.title.set_text("Starlight Atlas V2");
                assert!(h.publish_modal.has_metadata_changes());
                let upd = h.publish_modal.build_update_request();
                let sub = h.publish_modal.build_submit_request();
                h.publish_modal.is_submitting = true;
                (upd, sub)
            })
        })
        .unwrap();
    assert_eq!(
        upd2.as_ref().and_then(|u| u.expected_revision),
        Some(4),
        "second submit MUST send updated expected_revision = 4"
    );
    let _ = shell.update(&mut app, |s, cx| {
        s.submit_creation_for_review_fenced("cr_rev_1".to_string(), upd2, sub2, cx);
    });
    wait_until(
        &mut app,
        60,
        "R2 second submit succeeds with 200 PendingReview and zero 409",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub
                        .read(cx)
                        .find_my_creation("cr_rev_1")
                        .is_some_and(|c| c.review_status == CreationReviewStatus::PendingReview)
                })
                .unwrap_or(false)
        },
    )
    .await;

    let (status2, rev2, srv_err2) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            let c = h.find_my_creation("cr_rev_1");
            (
                c.map(|x| x.review_status),
                c.map(|x| x.cloud_revision),
                h.publish_modal.server_error.clone(),
            )
        })
        .unwrap();
    assert_eq!(status2, Some(CreationReviewStatus::PendingReview));
    assert_eq!(rev2, Some(6));
    assert!(srv_err2.is_none(), "second submit must NOT hit 409 Conflict: {srv_err2:?}");
    assert_eq!(mock_state.put_requests_received.lock().unwrap().len(), 2);

    // 3. Verify PUT 409 inside submit_creation_for_review_fenced formats localized conflict message.
    let _ = shell.update(&mut app, |s, cx| {
        let stale = UpdateCreationRequest {
            expected_revision: Some(1),
            title: Some("Conflict Trigger".to_string()),
            ..Default::default()
        };
        s.submit_creation_for_review_fenced(
            "cr_rev_1".to_string(),
            Some(stale),
            gpui_telegram_proto::creations::SubmitReviewRequest::default(),
            cx,
        );
    });
    wait_until(
        &mut app,
        50,
        "R2 PUT 409 inside submit_creation_for_review_fenced formats conflict error",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub
                        .read(cx)
                        .publish_modal
                        .server_error
                        .as_deref()
                        .is_some_and(|e| e.contains("409") || e.contains("冲突"))
                })
                .unwrap_or(false)
        },
    )
    .await;
    let conflict_err = shell
        .read_with(&app, |s, cx| s.content_hub.read(cx).publish_modal.server_error.clone())
        .unwrap()
        .expect("409 conflict message must populate");
    assert!(
        conflict_err.contains("409") || conflict_err.contains("冲突"),
        "expected localized 409 conflict message, got {conflict_err}"
    );
}

#[test]
fn test_m5_gpui_reg_f3_b2_per_section_catalog_gen_and_ws_query_isolation() {
    run_in_headless_subprocess(
        "test_m5_gpui_reg_f3_b2_per_section_catalog_gen_and_ws_query_isolation",
        false,
    );
}

async fn body_reg_f3_b2_per_section_catalog_gen_and_ws_query_isolation(mut app: AsyncApp) {
    let mock_state = Arc::new(MockM5State::new(false, Vec::new(), Vec::new()));
    mock_state
        .catalog_books
        .lock()
        .unwrap()
        .push(sample_public_catalog_item(
            "cr_book_ws",
            CreationKind::Book,
            "Orbital Atlas",
        ));
    mock_state
        .catalog_music
        .lock()
        .unwrap()
        .push(sample_public_catalog_item(
            "cr_music_ws",
            CreationKind::Music,
            "Midnight Synth",
        ));
    mock_state
        .catalog_appstore
        .lock()
        .unwrap()
        .apps
        .push(sample_public_catalog_item(
            "cr_app_ws",
            CreationKind::App,
            "AppOnlyQuery Tool",
        ));

    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.switch_to_content_hub(ContentHubSection::AppStore, cx);
    });
    wait_until(
        &mut app,
        60,
        "R3 initial AppStore switch completes before setting search query",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    !h.is_loading_catalog
                        && h.appstore_catalog
                            .apps
                            .iter()
                            .any(|ap| ap.creation_id == "cr_app_ws")
                })
                .unwrap_or(false)
        },
    )
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.catalog_search.set_text("AppOnlyQuery");
            h.books_catalog.clear();
            h.appstore_catalog = AppStoreCatalogResponse::default();
        });
    });
    mock_state.recorded_catalog_requests.lock().unwrap().clear();

    // 1. On AppStore with catalog_search="AppOnlyQuery", WS Book published triggers BOTH Books & AppStore refresh.
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::PublicCatalogChanged {
                creation_id: "cr_book_ws".to_string(),
                kind: CreationKind::Book,
                action: "published".to_string(),
            }),
            cx,
        );
    });
    wait_until(
        &mut app,
        60,
        "R3 both books_catalog and appstore_catalog refresh without self-cancellation",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.books_catalog.iter().any(|b| b.creation_id == "cr_book_ws")
                        && h.appstore_catalog
                            .apps
                            .iter()
                            .any(|ap| ap.creation_id == "cr_app_ws")
                })
                .unwrap_or(false)
        },
    )
    .await;

    let (books_ids, appstore_ids) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.books_catalog
                    .iter()
                    .map(|b| b.creation_id.clone())
                    .collect::<Vec<_>>(),
                h.appstore_catalog
                    .apps
                    .iter()
                    .map(|a| a.creation_id.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap();
    assert!(
        books_ids.contains(&"cr_book_ws".to_string()),
        "Books catalog refresh MUST NOT be cancelled by AppStore refresh"
    );
    assert!(
        appstore_ids.contains(&"cr_app_ws".to_string()),
        "AppStore catalog MUST also refresh"
    );
    let recorded = mock_state.recorded_catalog_requests.lock().unwrap().clone();
    let book_req = recorded
        .iter()
        .find(|r| r.starts_with("/api/v1/catalog?") && r.contains("kind=book"))
        .expect("expected GET /api/v1/catalog?kind=book request");
    assert!(
        !book_req.contains("q="),
        "background Books refresh must NOT inherit AppStore catalog_search query: {book_req}"
    );
    let appstore_req = recorded
        .iter()
        .find(|r| r.starts_with("/api/v1/catalog/appstore"))
        .expect("expected GET /api/v1/catalog/appstore request");
    assert!(
        appstore_req.contains("q=AppOnlyQuery"),
        "active AppStore refresh must preserve catalog_search query: {appstore_req}"
    );

    // 2. On Books with catalog_search="SciFiOnly", WS Music refresh must NOT inherit query or flip loading.
    let _ = shell.update(&mut app, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Books, cx);
    });
    wait_until(
        &mut app,
        60,
        "R3 initial Books switch completes before setting SciFiOnly search query",
        |a| {
            shell
                .read_with(a, |s, cx| !s.content_hub.read(cx).is_loading_catalog)
                .unwrap_or(false)
        },
    )
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.catalog_search.set_text("SciFiOnly");
            h.music_catalog.clear();
            h.is_loading_catalog = false;
        });
    });
    mock_state.recorded_catalog_requests.lock().unwrap().clear();
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_ws_server_frame(
            WsServerFrame::Update(UpdateEvent::PublicCatalogChanged {
                creation_id: "cr_music_ws".to_string(),
                kind: CreationKind::Music,
                action: "published".to_string(),
            }),
            cx,
        );
        assert!(
            !s.content_hub.read(cx).is_loading_catalog,
            "background WS refresh of inactive Music must NOT flip active Books loading spinner"
        );
    });
    wait_until(
        &mut app,
        60,
        "R3 background WS Music refresh populates music_catalog without Books query",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.content_hub
                        .read(cx)
                        .music_catalog
                        .iter()
                        .any(|m| m.creation_id == "cr_music_ws")
                })
                .unwrap_or(false)
        },
    )
    .await;
    let music_reqs = mock_state.recorded_catalog_requests.lock().unwrap().clone();
    let music_req = music_reqs
        .iter()
        .find(|r| r.starts_with("/api/v1/catalog?") && r.contains("kind=music"))
        .expect("expected GET /api/v1/catalog?kind=music request");
    assert!(
        !music_req.contains("q="),
        "background Music refresh must NOT inherit Books catalog_search query: {music_req}"
    );
}

#[test]
fn test_m5_gpui_reg_f4_b3_remote_delete_vertical_switch_and_detail_clear() {
    run_in_headless_subprocess(
        "test_m5_gpui_reg_f4_b3_remote_delete_vertical_switch_and_detail_clear",
        true,
    );
}

fn body_reg_f4_b3_remote_delete_vertical_switch_and_detail_clear(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
    });

    // (a) Remote soft-delete via set_my_creations closes Submit modal & clears owned selection.
    let owned = sample_book_creation(
        "cr_owned_del",
        1001,
        "Owned Draft",
        "Owned draft that will be soft-deleted remotely.",
        "Readable body exceeding fifty non-whitespace characters comfortably for validation.",
        CreationReviewStatus::Draft,
        2,
    );
    let mut tombstone = owned.clone();
    tombstone.cloud_revision = 3;
    tombstone.deleted_at_ms = Some(1_700_000_099_000);

    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.set_my_creations(vec![owned.clone()]);
            h.selected_creation_id = Some("cr_owned_del".to_string());
            h.selected_detail_creation = Some(owned.clone());
        });
        s.open_publish_review_modal_for_creation(owned.clone(), cx);
        assert!(s.is_publish_review_modal_active(cx));

        s.content_hub.update(cx, |h, hcx| {
            h.set_my_creations(vec![tombstone]);
            hcx.notify();
        });
        let h = s.content_hub.read(cx);
        assert!(
            !h.publish_modal.is_open,
            "remote soft-delete must close open Submit tab modal"
        );
        assert!(h.publish_modal.target_creation_id.is_none());
        assert!(h.publish_modal.base_creation.is_none());
        assert!(h.selected_detail_creation.is_none());
        assert!(h.selected_creation_id.is_none());
    });

    // (b) Open Reviewer Queue tab MUST stay open across set_my_creations(vec![])!
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.reviewer_access = ReviewerAccessState::Allowed;
        });
        s.open_reviewer_queue_modal(cx);
        assert!(s.content_hub.read(cx).publish_modal.is_open);
        assert!(s.content_hub.read(cx).publish_modal.show_reviewer_queue_tab);
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.target_creation_id = Some("cr_other_deleted".to_string());
            h.set_my_creations(Vec::new());
        });
        let pm = &s.content_hub.read(cx).publish_modal;
        assert!(
            pm.is_open && pm.show_reviewer_queue_tab,
            "open Reviewer Queue tab MUST NOT be closed by set_my_creations"
        );
        s.close_publish_review_modal(cx);
    });

    // (c) Non-owned third-party catalog item's selected_detail_creation is preserved across set_my_creations(vec![]).
    let third_party_cat = sample_public_book_item("cr_third_party", "Third Party Book");
    let third_party_dto = sample_book_creation(
        "cr_third_party",
        9999,
        "Third Party Book",
        "Public book authored by another creator.",
        "Readable body exceeding fifty non-whitespace characters comfortably for validation.",
        CreationReviewStatus::Published,
        7,
    );
    shell.update(cx, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub.update(cx, |h, _| {
            h.books_catalog = vec![third_party_cat.clone()];
            h.selected_creation_id = Some("cr_third_party".to_string());
            h.selected_catalog_item = Some(third_party_cat.clone());
            h.selected_detail_creation = Some(third_party_dto.clone());
            h.set_my_creations(Vec::new());
        });
        let h = s.content_hub.read(cx);
        assert_eq!(h.selected_creation_id.as_deref(), Some("cr_third_party"));
        assert!(
            h.selected_detail_creation.is_some(),
            "third-party public catalog detail must NOT be cleared by set_my_creations"
        );
    });

    // (d) Switching vertical (Books -> Music) clears selected_creation_id, selected_catalog_item, selected_detail_creation.
    shell.update(cx, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Music, cx);
        let h = s.content_hub.read(cx);
        assert!(
            h.selected_creation_id.is_none(),
            "vertical switch must clear selected_creation_id"
        );
        assert!(
            h.selected_catalog_item.is_none(),
            "vertical switch must clear selected_catalog_item"
        );
        assert!(
            h.selected_detail_creation.is_none(),
            "vertical switch must clear selected_detail_creation"
        );
    });

    // (e) Selecting catalog card B while card A's detail is loaded clears stale A detail.
    let book_a = sample_public_book_item("cr_a", "Book Alpha");
    let book_b = sample_public_book_item("cr_b", "Book Beta");
    let book_a_dto = sample_book_creation(
        "cr_a",
        2002,
        "Book Alpha",
        "Alpha summary long enough for validation.",
        "Readable body exceeding fifty non-whitespace characters comfortably for validation.",
        CreationReviewStatus::Published,
        2,
    );
    shell.update(cx, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub.update(cx, |h, _| {
            h.books_catalog = vec![book_a.clone(), book_b.clone()];
            h.selected_creation_id = Some("cr_a".to_string());
            h.selected_catalog_item = Some(book_a.clone());
            h.selected_detail_creation = Some(book_a_dto.clone());
        });
        s.handle_content_hub_event(
            &ContentHubEvent::OpenCreationDetail {
                section: ContentHubSection::Books,
                creation_id: "cr_b".to_string(),
                action: Some("open".to_string()),
            },
            cx,
        );
        let h = s.content_hub.read(cx);
        assert_eq!(h.selected_creation_id.as_deref(), Some("cr_b"));
        assert_eq!(
            h.selected_catalog_item
                .as_ref()
                .map(|c| c.creation_id.as_str()),
            Some("cr_b")
        );
        assert!(
            h.selected_detail_creation.is_none(),
            "selecting card B must NOT retain stale Book A selected_detail_creation"
        );
    });

    // (f) Deep-link into Music clears prior Books selection and preserves deep-link target cr_m1.
    shell.update(cx, |s, cx| {
        assert!(s.handle_apple_deep_link("tg://apple/music/cr_m1?action=play", cx));
        let h = s.content_hub.read(cx);
        assert_eq!(h.active_section, ContentHubSection::Music);
        assert_eq!(h.selected_creation_id.as_deref(), Some("cr_m1"));
        assert_eq!(h.active_action_hint.as_deref(), Some("play"));
        assert!(h.selected_detail_creation.is_none());
    });
}

#[test]
fn test_m5_gpui_reg_b4_ime_priority_quick_drawer_settings_modal_and_miniapp() {
    run_in_headless_subprocess(
        "test_m5_gpui_reg_b4_ime_priority_quick_drawer_settings_modal_and_miniapp",
        true,
    );
}

fn body_reg_b4_ime_priority_quick_drawer_settings_modal_and_miniapp(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
    });

    // (a) QuickDrawerSearch in Chats with open composer slash-autocomplete: Up/Down/Enter/Tab must NOT leak.
    shell.update(cx, |s, cx| {
        s.composer.update(cx, |c, _| {
            c.set_text("/".to_string());
            c.bot_ac_visible = true;
            c.bot_ac_matches = vec![
                BotSlashCommandItem {
                    command: "newbook".to_string(),
                    description: "Create a book".to_string(),
                    bot_username: "CreatorStudioBot".to_string(),
                    avatar_initials: "CS".to_string(),
                    avatar_color: 0x007AFF,
                },
                BotSlashCommandItem {
                    command: "newmusic".to_string(),
                    description: "Create music".to_string(),
                    bot_username: "CreatorStudioBot".to_string(),
                    avatar_initials: "CS".to_string(),
                    avatar_color: 0xFA2D48,
                },
            ];
            c.bot_ac_selected = 0;
        });
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    // Consecutive GPUI update flushes FocusInputTaken -> composer.is_input_focused == false.
    shell.update(cx, |s, cx| {
        assert!(s.handle_non_char_key("down", false, cx));
        assert!(s.handle_non_char_key("up", false, cx));
        assert!(s.handle_non_char_key("enter", false, cx));
        assert!(s.handle_non_char_key("tab", false, cx));

        let c = s.composer.read(cx);
        assert_eq!(c.bot_ac_selected, 0, "QuickDrawerSearch arrows must NOT move composer autocomplete");
        assert!(c.bot_ac_visible, "QuickDrawerSearch enter/tab must NOT dismiss composer autocomplete");
        assert_eq!(c.text(), "/", "QuickDrawerSearch must NOT commit autocomplete into composer");
        assert!(!c.is_input_focused);
        assert!(
            !s.sidebar.read(cx).search_focused,
            "Tab inside QuickDrawerSearch must NOT steal focus to ChatSidebar search"
        );
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::QuickDrawerSearch
        );
    });

    // (b) QuickDrawerSearch yields cleanly to Settings and modal_container (zero preedit/commit split, zero Cmd+A theft).
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.quick_drawer_search.clear();
            h.active_input = ContentHubInputTarget::QuickDrawerSearch;
        });
        s.navigate_settings_to(SettingsSectionType::Search, cx);
        assert!(s.is_settings_input_active(cx));
        assert!(
            !s.is_content_hub_input_active(cx),
            "QuickDrawerSearch must yield while Settings is active"
        );

        s.update_ime_marked_text("shezhi", cx);
        assert_eq!(s.active_text_and_preedit(cx).1.as_deref(), Some("shezhi"));
        assert!(
            s.content_hub
                .read(cx)
                .quick_drawer_search
                .ime_preedit
                .is_none(),
            "IME preedit in Settings must NOT bleed into QuickDrawerSearch"
        );
        s.insert_committed_text("设置", cx);
        assert_eq!(
            s.settings_router
                .read(cx)
                .active_input_text_and_preedit(&s.ime, cx)
                .map(|(t, _)| t)
                .as_deref(),
            Some("设置")
        );
        assert!(s.content_hub.read(cx).quick_drawer_search.text.is_empty());

        let _ = s.handle_shortcut_key("a", false, cx);
        assert!(
            !s.content_hub.read(cx).quick_drawer_search.select_all,
            "Cmd+A in Settings must NOT be stolen by QuickDrawerSearch"
        );

        // Close Settings, re-focus QuickDrawerSearch, open modal_container -> modal_container wins IME & commit.
        s.close_settings(cx);
        s.content_hub.update(cx, |h, hcx| {
            h.quick_drawer_search.clear();
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
        s.modal_container.update(cx, |mc, mcx| {
            mc.show_dialog(
                "New Group",
                ModalDialogKind::NewGroup,
                Vec::new(),
                420.0,
                400.0,
                LayerStackOption::Normal,
            );
            mcx.notify();
        });
        assert!(!s.is_content_hub_input_active(cx));
        s.update_ime_marked_text("qun", cx);
        assert!(
            s.content_hub
                .read(cx)
                .quick_drawer_search
                .ime_preedit
                .is_none(),
            "IME preedit in modal_container must NOT bleed into QuickDrawerSearch"
        );
        s.insert_committed_text("群组", cx);
        assert!(s.content_hub.read(cx).quick_drawer_search.text.is_empty());
        assert!(
            s.modal_container
                .read(cx)
                .dialog
                .active_field_text()
                .contains("群组"),
            "modal_container must receive committed text"
        );
        s.modal_container.update(cx, |mc, mcx| {
            mc.dismiss_top();
            mcx.notify();
        });
    });

    // (c) PublishReviewModal (Tier 4) beats BOTH miniapp_overlay AND create_bot_box across all IME/key/Escape ops.
    let draft = sample_book_creation(
        "cr_ime_mini",
        1001,
        "Init",
        "Summary text exceeding ten chars cleanly.",
        "Readable chapter body exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    shell.update(cx, |s, cx| {
        let mut mv = BotAttachWebView::new();
        mv.is_open = true;
        mv.focused_input = Some(BotWebViewInputTarget::CopilotPrompt);
        s.miniapps.miniapp_overlay = Some(cx.new(|_| mv));

        let bot_box = CreateManagedBotBoxPeers::new();
        s.miniapps.create_bot_box = Some(cx.new(|_| bot_box));

        s.open_publish_review_modal_for_creation(draft, cx);
        assert!(s.is_publish_review_modal_active(cx));

        // Marked text -> PublishReviewModal
        s.update_ime_marked_text("shuji", cx);
        assert_eq!(s.active_text_and_preedit(cx).1.as_deref(), Some("shuji"));
        assert_eq!(
            s.content_hub
                .read(cx)
                .publish_modal
                .title
                .ime_preedit
                .as_deref(),
            Some("shuji")
        );

        // Commit -> PublishReviewModal (miniapp_overlay & create_bot_box untouched)
        s.insert_committed_text("书籍", cx);
        assert!(s.content_hub.read(cx).publish_modal.title.text.ends_with("书籍"));
        assert!(
            s.miniapps
                .miniapp_overlay
                .as_ref()
                .unwrap()
                .read(cx)
                .copilot_prompt
                .is_empty(),
            "miniapp_overlay must NOT receive text while PublishReviewModal is on top"
        );
        assert!(
            s.miniapps
                .create_bot_box
                .as_ref()
                .unwrap()
                .read(cx)
                .active_field_text()
                .is_empty(),
            "create_bot_box must NOT receive text while PublishReviewModal is on top"
        );

        // Backspace -> removes "籍" from PublishReviewModal title
        assert!(s.handle_non_char_key("backspace", false, cx));
        assert!(s.content_hub.read(cx).publish_modal.title.text.ends_with('书'));

        // Cmd+A -> selects all in PublishReviewModal title; clear_active_field clears title
        assert!(s.handle_shortcut_key("a", false, cx));
        assert!(s.content_hub.read(cx).publish_modal.title.select_all);
        s.clear_active_field(cx);
        assert!(s.content_hub.read(cx).publish_modal.title.text.is_empty());

        // Escape -> closes PublishReviewModal FIRST while occluded miniapp_overlay & create_bot_box remain open!
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.is_publish_review_modal_active(cx),
            "Escape must close foreground PublishReviewModal first"
        );
        assert!(
            s.miniapps.miniapp_overlay.as_ref().unwrap().read(cx).is_open,
            "occluded miniapp_overlay must stay open when Escape closes PublishReviewModal"
        );
        assert!(
            s.miniapps.create_bot_box.as_ref().unwrap().read(cx).is_visible,
            "occluded create_bot_box must stay visible when Escape closes PublishReviewModal"
        );
    });
}

#[test]
fn test_m5_iter3_reg_editable_field_bare_tg_deeplink_and_reason_normalization() {
    // FIX-1: EditableFieldState OOB cursor_char == usize::MAX move_right (no panic) & move_left clamp.
    let mut f_emoji = EditableFieldState::new("hello 中文 🚀 👨‍👩‍👧‍👦");
    let expected_len = f_emoji.text.chars().count();
    f_emoji.cursor_char = usize::MAX;
    f_emoji.move_right();
    assert_eq!(f_emoji.cursor_char, expected_len);

    let mut f_abc = EditableFieldState::new("abc");
    f_abc.cursor_char = usize::MAX;
    f_abc.move_left();
    assert_eq!(f_abc.cursor_char, 2);

    let mut f_empty = EditableFieldState::new("");
    f_empty.cursor_char = usize::MAX;
    f_empty.move_left();
    assert_eq!(f_empty.cursor_char, 0);
    f_empty.cursor_char = usize::MAX;
    f_empty.move_right();
    assert_eq!(f_empty.cursor_char, 0);

    // FIX-7: Invisible/zero-width/BiDi/control-only rejection reasons blocked by normalizer & modal.
    for blank in [
        "",
        "   \t\r\n  ",
        "\u{200b}",
        "\u{feff}\u{200c}\u{200d}\u{2060}",
        "\0\x07\x1b\x7f",
        "\u{202e}\u{200b}\0",
        "\u{2066}\u{2069}\u{200f}\x08",
    ] {
        assert_eq!(
            normalize_non_empty_reason(blank),
            None,
            "invisible/control-only reason {blank:?} must normalize to None"
        );
        let mut modal = PublishReviewModalState::default();
        modal.reject_reason.set_text(blank);
        assert!(
            modal.build_reject_request("en").is_err(),
            "build_reject_request must reject invisible/control reason {blank:?}"
        );
        assert!(modal.reviewer_error.is_some());
    }
    assert_eq!(
        normalize_non_empty_reason(" \u{200b}Missing chapter 2\x07\u{feff} "),
        Some("Missing chapter 2".to_string())
    );
    assert_eq!(
        normalize_non_empty_reason("Line 1\nLine 2"),
        Some("Line 1\nLine 2".to_string())
    );

    // FIX-8: Bare tg:// auto-linking adjacent to multi-byte emoji and CJK words.
    let bare_cases = [
        ("tg://apple/books/cr_1🚀", "tg://apple/books/cr_1"),
        ("🔥tg://apple/music/cr_9🎵", "tg://apple/music/cr_9"),
        ("打开tg://apple/books/cr_42看看", "tg://apple/books/cr_42"),
        ("请看TG://APPLE/GAMES/cr_7。真好玩", "TG://APPLE/GAMES/cr_7"),
    ];
    for (input, expected_link) in bare_cases {
        let blocks = parse_rich_blocks(input, &[]);
        let entity_links: Vec<String> = blocks
            .into_iter()
            .flat_map(|b| match b {
                RichBlock::Paragraph(segs) => segs,
                _ => Vec::new(),
            })
            .filter_map(|s| s.flags.link_url)
            .collect();
        assert_eq!(
            entity_links,
            vec![expected_link.to_string()],
            "stage/entities.rs must extract exact ASCII tg:// URI for {input:?}"
        );

        let card_links: Vec<String> = parse_markdown_spans(input)
            .into_iter()
            .filter_map(|sp| match sp.kind {
                RichSpanKind::Link(u) => Some(u),
                _ => None,
            })
            .collect();
        assert_eq!(
            card_links,
            vec![expected_link.to_string()],
            "stage/cards/text_card.rs must extract exact ASCII tg:// URI for {input:?}"
        );
        assert!(
            parse_apple_deep_link(expected_link).is_some(),
            "extracted URI {expected_link:?} must parse via parse_apple_deep_link"
        );
    }

    // FIX-9: Invisible/BiDi & url:-prefixed Apple deep-link smuggling detected without false positives.
    for smuggled in [
        "\u{200b}tg://apple/books/<script>alert(1)</script>",
        "\u{202e}tg://apple/books/<script>alert(1)</script>",
        "\u{feff}creation:open:book:../secret",
        "url:tg://apple/books/<script>alert(1)</script>",
        "URL: \u{200b}tg://apple/books/<script>",
        "url:creation:open:book:../secret",
    ] {
        assert!(
            is_apple_deep_link_candidate(smuggled),
            "is_apple_deep_link_candidate must catch smuggled payload {smuggled:?}"
        );
    }
    for non_apple in [
        "tg://applepie/books/cr_1",
        "url:https://example.com/apple",
        "tg://resolve?domain=CreatorStudioBot",
    ] {
        assert!(
            !is_apple_deep_link_candidate(non_apple),
            "is_apple_deep_link_candidate must NOT false-positive on {non_apple:?}"
        );
    }
}

#[test]
fn test_m5_gpui_iter3_reg_focus_ime_escape_smuggling_and_unauth_guards() {
    run_in_headless_subprocess(
        "test_m5_gpui_iter3_reg_focus_ime_escape_smuggling_and_unauth_guards",
        true,
    );
}

fn body_iter3_reg_focus_ime_escape_smuggling_and_unauth_guards(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.quick_drawer_search.clear();
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });

    // FIX-2a: Clicking back into CanonicalComposer while Quick Drawer stays open blurs QuickDrawerSearch.
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("pre_c", cx);
        s.composer.update(cx, |c, ccx| {
            c.set_text(String::new());
            c.is_input_focused = true;
            ccx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        s.insert_committed_text("hello_composer", cx);
        assert_eq!(s.composer.read(cx).text(), "hello_composer");
        assert!(s.content_hub.read(cx).quick_drawer_search.text.is_empty());
    });

    // FIX-2b: Clicking back into ChatSidebar search while Quick Drawer stays open blurs QuickDrawerSearch.
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("pre_s", cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.set_search_query(String::new());
            sb.focus_search();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        s.insert_committed_text("sidebar_find", cx);
        assert_eq!(s.sidebar.read(cx).search_query, "sidebar_find");
        assert!(s.content_hub.read(cx).quick_drawer_search.text.is_empty());
    });

    // FIX-2c: Opening ConversationStage search while Quick Drawer stays open blurs QuickDrawerSearch.
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("pre_st", cx);
        s.stage.update(cx, |st, stcx| {
            st.search_query.clear();
            st.search_open = true;
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        s.insert_committed_text("stage_find", cx);
        assert_eq!(s.stage.read(cx).search_query, "stage_find");
        assert!(s.content_hub.read(cx).quick_drawer_search.text.is_empty());
        s.stage.update(cx, |st, stcx| {
            st.search_open = false;
            stcx.notify();
        });
    });

    // FIX-3: quick-drawer-close-btn (CloseQuickDrawer) clears shell.ime.ime_preedit cleanly.
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("nihao", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("nihao"));
        // Normal close button path emits CloseQuickDrawer without pre-wiping active_input.
        s.content_hub.update(cx, |_, hcx| {
            hcx.emit(ContentHubEvent::CloseQuickDrawer);
            hcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(!s.content_hub.read(cx).quick_drawer_open);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).quick_drawer_search.ime_preedit.is_none());
    });

    // FIX-5: Cmd+K from QuickDrawerSearch over Chats clears shell.ime.ime_preedit (zero bleed to sidebar).
    shell.update(cx, |s, cx| {
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, _| sb.set_search_query(String::new()));
        s.update_ime_marked_text("drawer_pre", cx);
        assert!(s.handle_shortcut_key("k", false, cx));
        assert!(s.sidebar.read(cx).search_focused);
        assert!(s.ime.ime_preedit.is_none());
        assert_eq!(s.active_text_and_preedit(cx), (String::new(), None));
    });

    // FIX-4: Passcode lock blocks update_ime_marked_text from leaking into occluded PublishReviewModal.
    let draft = sample_book_creation(
        "cr_iter3_1",
        1001,
        "Iter3 Atlas",
        "A valid summary longer than ten characters.",
        "Chapter one has plenty of readable text exceeding fifty non-whitespace characters.",
        CreationReviewStatus::Draft,
        1,
    );
    shell.update(cx, |s, cx| {
        s.open_publish_review_modal_for_creation(draft.clone(), cx);
        s.overlays.is_passcode_locked = true;
        s.update_ime_marked_text("9876", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("9876"));
        assert!(
            s.content_hub.read(cx).publish_modal.title.ime_preedit.is_none(),
            "passcode preedit must NOT mirror into occluded PublishReviewModal"
        );
        s.overlays.is_passcode_locked = false;
        s.ime.clear_preedit();
        s.close_publish_review_modal(cx);
    });

    // FIX-6: Background composer autocomplete & ime_machine never swallow Escape for ContentHub/QuickDrawer or PublishReviewModal.
    shell.update(cx, |s, cx| {
        s.composer.update(cx, |c, _| {
            c.set_text("/".to_string());
            c.bot_ac_visible = true;
            c.bot_ac_matches = vec![BotSlashCommandItem {
                command: "newbook".to_string(),
                description: "Create a book".to_string(),
                bot_username: "CreatorStudioBot".to_string(),
                avatar_initials: "CS".to_string(),
                avatar_color: 0x007AFF,
            }];
            c.bot_ac_selected = 0;
        });
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        // Even if composer autocomplete is re-populated while in ContentHub, Escape must close QuickDrawer.
        s.composer.update(cx, |c, _| {
            c.bot_ac_visible = true;
            c.bot_ac_matches = vec![BotSlashCommandItem {
                command: "newbook".to_string(),
                description: "Create a book".to_string(),
                bot_username: "CreatorStudioBot".to_string(),
                avatar_initials: "CS".to_string(),
                avatar_color: 0x007AFF,
            }];
        });
        s.toggle_quick_creations_drawer(true, cx);
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.content_hub.read(cx).quick_drawer_open,
            "hidden composer autocomplete must NOT swallow Escape for QuickDrawer in ContentHub"
        );

        // Background composer ime_machine preedit must NOT swallow Escape for PublishReviewModal over Chats.
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.composer.update(cx, |c, _| {
            c.ime_machine.on_ime_update("ni", 2);
        });
        s.open_publish_review_modal_for_creation(draft.clone(), cx);
        assert!(
            !s.composer.read(cx).ime_machine.is_composing(),
            "dismiss_transient_popovers_for_hub must cancel composer ime_machine"
        );
        // Even if composer preedit is manually re-injected while modal is open, Escape must close PublishReviewModal.
        s.composer.update(cx, |c, _| {
            c.ime_machine.on_ime_update("ni", 2);
        });
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.is_publish_review_modal_active(cx),
            "background composer ime_machine must NOT swallow Escape for PublishReviewModal"
        );
        s.composer.update(cx, |c, _| c.ime_machine.on_ime_cancel());
    });

    // FIX-9: Smuggled invisible/BiDi/url: Apple payloads produce zero leakage across all shell entry points.
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        let msg_count_before = s.stage.read(cx).messages.len();
        s.window_runtime.controller.clear_all_toasts();
        for smuggled in [
            "\u{200b}tg://apple/books/<script>alert(1)</script>",
            "\u{202e}tg://apple/books/<script>alert(1)</script>",
            "\u{feff}creation:open:book:../secret",
            "url:tg://apple/books/<script>alert(1)</script>",
        ] {
            s.execute_hermes_inline_callback(smuggled.to_string(), cx);
            s.handle_stage_bot_button(1, InlineButtonType::Url(smuggled.to_string()), cx);
            s.stage.update(cx, |st, stcx| {
                st.handle_rich_text_action(RichTextAction::OpenUrl(smuggled.to_string()), stcx);
                st.pending_media_url = Some((smuggled.to_string(), false));
                stcx.notify();
            });
            s.handle_stage_pending_actions(cx);
            assert_eq!(
                s.stage.read(cx).messages.len(),
                msg_count_before,
                "smuggled payload {smuggled:?} must NOT leak into outgoing messages"
            );
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "smuggled payload {smuggled:?} must NOT leak to clipboard toast"
            );
            assert!(
                s.overlays.media_viewer_overlay.is_none(),
                "smuggled payload {smuggled:?} must NOT leak to MediaViewer"
            );
        }
    });

    // FIX-12: Unauthenticated OpenCreationDetail, toggle_quick_creations_drawer(true), and SelectCreationKindFilter safely ignored.
    shell.update(cx, |s, cx| {
        s.logout(cx);
        assert!(!s.is_authenticated);
        s.handle_content_hub_event(
            &ContentHubEvent::OpenCreationDetail {
                section: ContentHubSection::Books,
                creation_id: "cr_unauth".to_string(),
                action: Some("read".to_string()),
            },
            cx,
        );
        s.toggle_quick_creations_drawer(true, cx);
        s.handle_apple_sidebar_event(
            &AppleSidebarEvent::SelectCreationKindFilter(Some(CreationKind::Book)),
            cx,
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        let h = s.content_hub.read(cx);
        assert!(!h.quick_drawer_open);
        assert!(h.selected_creation_id.is_none());
    });
}

#[test]
fn test_m5_gpui_iter3_reg_async_reject_audit_bot_profile_and_delete() {
    run_in_headless_subprocess(
        "test_m5_gpui_iter3_reg_async_reject_audit_bot_profile_and_delete",
        false,
    );
}

async fn body_iter3_reg_async_reject_audit_bot_profile_and_delete(mut app: AsyncApp) {
    let draft_a = sample_book_creation(
        "cr_race_a",
        1001,
        "Race Book A",
        "Race A summary exceeding ten characters cleanly.",
        "Chapter one body for race book A exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    let draft_b = sample_book_creation(
        "cr_race_b",
        1001,
        "Race Book B",
        "Race B summary exceeding ten characters cleanly.",
        "Chapter one body for race book B exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    let pending_q = sample_book_creation(
        "cr_q_1",
        2002,
        "Queued Item",
        "Pending review summary exceeding ten characters cleanly.",
        "Chapter one body for queued item exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        2,
    );

    let mock_state = Arc::new(MockM5State::new(
        true,
        vec![draft_a.clone(), draft_b.clone()],
        vec![pending_q.clone()],
    ));
    mock_state
        .serve_unstarted_creator_bot_profile
        .store(true, Ordering::Relaxed);
    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub_state.creator_bot_chat_in_flight = true;
        s.apply_authenticated_session(sample_auth(1001, "admin"), cx);
        // Polish Rev2 NB1: refresh_content_hub_on_login resets stale creator_bot_chat_in_flight.
        assert!(!s.content_hub_state.creator_bot_chat_in_flight);
    });
    wait_until(&mut app, 60, "iter3 initial creations and queue sync", |a| {
        shell
            .read_with(a, |s, cx| {
                let h = s.content_hub.read(cx);
                h.find_my_creation("cr_race_a").is_some()
                    && h.find_my_creation("cr_race_b").is_some()
                    && h.publish_modal.reviewer_access == ReviewerAccessState::Allowed
            })
            .unwrap_or(false)
    })
    .await;
    assert_eq!(
        shell
            .read_with(&app, |s, cx| s.content_hub.read(cx).publish_modal.reviewer_access)
            .unwrap(),
        ReviewerAccessState::Allowed
    );

    // FIX-7 (live HTTP): submit_review_decision_fenced blocks invisible/control-only reasons with 0 HTTP sent.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_reviewer_queue_modal(cx);
    });
    wait_until(&mut app, 60, "iter3 reviewer queue modal probe finished", |a| {
        shell
            .read_with(a, |s, cx| {
                let m = &s.content_hub.read(cx).publish_modal;
                !m.is_loading_queue && m.review_queue.len() == 1
            })
            .unwrap_or(false)
    })
    .await;
    for invisible in ["\u{200b}", "\u{feff}\u{200c}\u{200d}\u{2060}", "\0\x07\x1b\x7f", "\u{202e}\u{200b}\0"] {
        let _ = shell.update(&mut app, |s, cx| {
            s.content_hub.update(cx, |h, _| {
                h.publish_modal.is_submitting = true;
                h.publish_modal.reviewer_error = None;
            });
            s.submit_review_decision_fenced(
                "cr_q_1".to_string(),
                ReviewDecisionRequest {
                    decision: ReviewDecision::Reject,
                    visibility: None,
                    reason: Some(invisible.to_string()),
                    notes: None,
                },
                cx,
            );
            let m = &s.content_hub.read(cx).publish_modal;
            assert!(!m.is_submitting, "blocked reject must reset is_submitting = false");
            assert!(m.reviewer_error.is_some(), "blocked reject must populate reviewer_error");
        });
    }
    assert!(
        mock_state.review_decisions_received.lock().unwrap().is_empty(),
        "zero HTTP review requests may be sent for invisible/control-only reasons"
    );

    // Valid reject reason with surrounding invisible/control noise sends normalized clean text.
    let _ = shell.update(&mut app, |s, cx| {
        s.submit_review_decision_fenced(
            "cr_q_1".to_string(),
            ReviewDecisionRequest {
                decision: ReviewDecision::Reject,
                visibility: None,
                reason: Some(" \u{200b}Need cover artwork\x07\u{feff} ".to_string()),
                notes: None,
            },
            cx,
        );
    });
    wait_until(
        &mut app,
        60,
        "normalized rejection sent to mock server and pruned locally",
        |a| {
            !mock_state.review_decisions_received.lock().unwrap().is_empty()
                && shell
                    .read_with(a, |s, cx| {
                        s.content_hub.read(cx).publish_modal.review_queue.is_empty()
                    })
                    .unwrap_or(false)
        },
    )
    .await;
    {
        let recs = mock_state.review_decisions_received.lock().unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].0, "cr_q_1");
        assert_eq!(recs[0].1.reason.as_deref(), Some("Need cover artwork"));
    }

    // FIX-10: Background submit on cr_race_a MUST NOT clobber active cr_race_b modal's audit_request_gen.
    // Make cr_race_a submit finish (~30ms) BEFORE cr_race_b's /audit response (~120ms).
    mock_state.submit_delay_ms.store(30, Ordering::Relaxed);
    mock_state.audit_delay_ms.store(120, Ordering::Relaxed);
    let _ = shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(draft_a.clone(), cx);
        s.submit_creation_for_review_fenced(
            "cr_race_a".to_string(),
            None,
            SubmitReviewRequest {
                visibility: Some(CreationVisibility::Public),
                notes: None,
            },
            cx,
        );
        // Immediately switch PublishReviewModal to cr_race_b (which dispatches LoadCreationAudit for cr_race_b).
        s.open_publish_review_modal_for_creation(draft_b.clone(), cx);
    });
    wait_until(&mut app, 80, "cr_race_a finishes submitting AND cr_race_b audit lands intact", |a| {
        shell
            .read_with(a, |s, cx| {
                let h = s.content_hub.read(cx);
                let a_pending = h
                    .find_my_creation("cr_race_a")
                    .is_some_and(|c| c.review_status == CreationReviewStatus::PendingReview);
                a_pending && !h.publish_modal.audit_trail.is_empty()
            })
            .unwrap_or(false)
    })
    .await;
    let (target_id, audit_ids): (Option<String>, Vec<String>) = shell
        .read_with(&app, |s, cx| {
            let m = &s.content_hub.read(cx).publish_modal;
            (
                m.target_creation_id.clone(),
                m.audit_trail.iter().map(|a| a.creation_id.clone()).collect(),
            )
        })
        .unwrap();
    assert_eq!(target_id.as_deref(), Some("cr_race_b"));
    assert_eq!(
        audit_ids,
        vec!["cr_race_b".to_string()],
        "cr_race_a background submit must NOT clobber cr_race_b's audit_request_gen"
    );
    mock_state.submit_delay_ms.store(0, Ordering::Relaxed);
    mock_state.audit_delay_ms.store(0, Ordering::Relaxed);

    // Polish NB3: delete_creation_fenced closing active PublishReviewModal clears shell.ime.ime_preedit.
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("del_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("del_pre"));
        s.delete_creation_fenced("cr_race_b".to_string(), cx);
    });
    wait_until(&mut app, 60, "delete_creation_fenced closes cr_race_b modal", |a| {
        shell
            .read_with(a, |s, cx| !s.is_publish_review_modal_active(cx))
            .unwrap_or(false)
    })
    .await;
    let (modal_open_after_del, preedit_after_del) = shell
        .read_with(&app, |s, cx| {
            (s.is_publish_review_modal_active(cx), s.ime.ime_preedit.clone())
        })
        .unwrap();
    assert!(!modal_open_after_del);
    assert!(
        preedit_after_del.is_none(),
        "delete_creation_fenced closing PublishReviewModal must clear shell.ime.ime_preedit"
    );

    // FIX-11: Unstarted @CreatorStudioBot profile (has_started_chat: false) must NOT re-enable show_bot_start_bar or hide /newbook prefill.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_creator_studio_bot_chat(Some("/newbook "), cx);
    });
    wait_until(
        &mut app,
        80,
        "CreatorStudioBot chat opened and unstarted bot profile cached",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    s.composer.read(cx).text() == "/newbook "
                        && s.miniapps.bot_profiles_cache.contains_key("creatorstudiobot")
                })
                .unwrap_or(false)
        },
    )
    .await;
    // Also explicitly re-run sync_composer_and_inspector_miniapp_state after profile cache arrives.
    let _ = shell.update(&mut app, |s, cx| {
        s.sync_composer_and_inspector_miniapp_state(cx);
    });
    let (comp_text, show_start_bar, hide_composer, has_started_flag) = shell
        .read_with(&app, |s, cx| {
            (
                s.composer.read(cx).text().to_string(),
                s.stage.read(cx).show_bot_start_bar,
                s.should_hide_composer(cx),
                s.miniapps
                    .bot_profiles_cache
                    .get("creatorstudiobot")
                    .map(|p| p.has_started_chat),
            )
        })
        .unwrap();
    assert_eq!(has_started_flag, Some(false));
    assert_eq!(comp_text, "/newbook ");
    assert!(
        !show_start_bar,
        "unstarted @CreatorStudioBot profile must NOT re-enable show_bot_start_bar when composer has prefill"
    );
    assert!(
        !hide_composer,
        "unstarted @CreatorStudioBot profile must NOT hide prefilled composer"
    );
}

#[test]
fn test_m5_iter4_reg_unicode_ignorable_metadata_bare_tg_and_url_unwrap() {
    // C2-I3-1: Comprehensive Unicode Default_Ignorable + invisible/blank-filler rejection.
    let ignorable_blanks = [
        "\u{00AD}",
        "\u{070F}",
        "\u{115F}",
        "\u{1160}",
        "\u{17B4}\u{17B5}",
        "\u{180B}\u{180C}\u{180D}\u{180E}\u{180F}",
        "\u{2800}",
        "\u{3164}",
        "\u{FE00}",
        "\u{FE0F}",
        "\u{FFA0}",
        "\u{FFF9}\u{FFFA}\u{FFFB}",
        "\u{13430}",
        "\u{1BCA0}",
        "\u{1D173}",
        "\u{E0001}\u{E0020}",
        "\u{E0100}",
    ];
    for blank in ignorable_blanks {
        assert_eq!(
            normalize_non_empty_reason(blank),
            None,
            "ignorable/blank reason {blank:?} must normalize to None"
        );
        let mut modal = PublishReviewModalState::default();
        modal.reject_reason.set_text(blank);
        assert!(
            modal.build_reject_request("en").is_err(),
            "build_reject_request must reject ignorable/blank {blank:?}"
        );
        assert!(modal.reviewer_error.is_some());

        for smuggled in [
            format!("{blank}tg://apple/books/<script>alert(1)</script>"),
            format!("url:{blank}tg://apple/books/cr_1"),
            format!("{blank}url:tg://apple/books/cr_1"),
            format!("t{blank}g://apple/books/cr_1"),
            format!("tg://a{blank}pple/books/cr_1"),
        ] {
            assert!(
                is_apple_deep_link_candidate(&smuggled),
                "is_apple_deep_link_candidate must catch smuggled {smuggled:?}"
            );
            assert!(
                parse_apple_deep_link(unwrap_apple_callback_uri(&smuggled)).is_none(),
                "smuggled payload {smuggled:?} must never parse as valid Apple deep link"
            );
        }
    }

    // Emoji / script joiners inside visible rejection reasons remain intact.
    assert_eq!(
        normalize_non_empty_reason("Missing cover ❤️ and family 👨‍👩‍👧‍👦"),
        Some("Missing cover ❤️ and family 👨‍👩‍👧‍👦".to_string())
    );
    assert_eq!(
        normalize_non_empty_reason("\u{202e}spoofed\u{202c} text"),
        Some("spoofed text".to_string())
    );

    // Canonical single-layer url: unwrap contract.
    assert_eq!(
        unwrap_apple_callback_uri("url:tg://apple/books/cr_1"),
        "tg://apple/books/cr_1"
    );
    assert_eq!(
        unwrap_apple_callback_uri("  URL:  creation:open:music:cr_2  "),
        "creation:open:music:cr_2"
    );
    assert!(parse_apple_deep_link(unwrap_apple_callback_uri("url:url:tg://apple/books/cr_1")).is_none());
    assert!(is_apple_deep_link_candidate("url:url:tg://apple/books/cr_1"));

    // Modal metadata visible-character validation, genre filtering, obscured XSS, and emoji preservation.
    let base = sample_book_creation(
        "cr_meta_1",
        1001,
        "Base Title",
        "A perfectly valid summary longer than ten characters.",
        "Chapter one has plenty of readable text exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    let mut modal = PublishReviewModalState::default();
    modal.open_for_creation(base);

    modal.title.set_text("\u{00AD}\u{3164}\u{2800}");
    modal.recompute_validation();
    assert!(
        modal.validation_errors.iter().any(|e| e.contains("title")),
        "invisible-only title must fail validation: {:?}",
        modal.validation_errors
    );

    modal.title.set_text("书 🚀 👨‍👩‍👧‍👦");
    modal.author_display_name.set_text("\u{3164}\u{FFA0}\u{FE0F}");
    modal.recompute_validation();
    assert!(
        modal
            .validation_errors
            .iter()
            .any(|e| e.contains("author_display_name")),
        "invisible-only author must fail validation: {:?}",
        modal.validation_errors
    );

    modal.author_display_name.set_text("作者 ❤️");
    modal.summary.set_text(format!("Hi{}", "\u{200b}".repeat(12)));
    modal.recompute_validation();
    assert!(
        modal.validation_errors.iter().any(|e| e.contains("summary")),
        "invisible-padded short summary must fail validation: {:?}",
        modal.validation_errors
    );

    // Invisible-only genres are dropped from parsed_genres, while visible emoji genres stay intact.
    modal.genres.set_text("\u{200b}, \u{3164}, \u{2800}");
    assert!(modal.parsed_genres().is_empty());
    modal.genres.set_text("Sci-Fi ❤️, \u{3164}, Space");
    assert_eq!(
        modal.parsed_genres(),
        vec!["Sci-Fi ❤️".to_string(), "Space".to_string()]
    );

    // Obscured XSS inside title is rejected.
    modal.summary.set_text("这是一段足够长的作品简介包含 👨‍👩‍👧‍👦 和 ❤️ 完整内容");
    modal.title.set_text("Hello <scr\u{00AD}ipt>alert(1)</script>");
    modal.recompute_validation();
    assert!(
        modal
            .validation_errors
            .iter()
            .any(|e| e.contains("disallowed script")),
        "soft-hyphen obscured XSS must be rejected: {:?}",
        modal.validation_errors
    );

    // Valid visible emoji/ZWJ/VS16 metadata passes cleanly and preserves code points.
    modal.title.set_text("书 🚀 👨‍👩‍👧‍👦");
    modal.review_notes.set_text("\u{200b}\u{3164}");
    modal.recompute_validation();
    assert!(
        modal.validation_errors.is_empty(),
        "valid emoji metadata must pass validation: {:?}",
        modal.validation_errors
    );
    let preview = modal.preview_dto().expect("preview dto");
    assert_eq!(preview.title, "书 🚀 👨‍👩‍👧‍👦");
    assert_eq!(preview.author_display_name, "作者 ❤️");
    assert_eq!(preview.review_notes, None);
    assert_eq!(modal.build_submit_request().notes, None);

    // C2-I3-3: Bare tg:// quote/bracket delimiters and empty/non-ASCII authority rejection.
    let quoted_cases = [
        ("Check \"tg://apple/books/cr_1\" now", "tg://apple/books/cr_1"),
        ("Check 'tg://apple/music/cr_2' now", "tg://apple/music/cr_2"),
        ("<tg://apple/games/cr_3>", "tg://apple/games/cr_3"),
        ("Try tg://apple/appstore/cr_4`!", "tg://apple/appstore/cr_4"),
    ];
    for (input, expected) in quoted_cases {
        let entity_links: Vec<String> = parse_rich_blocks(input, &[])
            .into_iter()
            .flat_map(|b| match b {
                RichBlock::Paragraph(segs) => segs,
                _ => Vec::new(),
            })
            .filter_map(|s| s.flags.link_url)
            .collect();
        assert_eq!(
            entity_links,
            vec![expected.to_string()],
            "entities.rs must strip quotes/brackets for {input:?}"
        );

        let card_links: Vec<String> = parse_markdown_spans(input)
            .into_iter()
            .filter_map(|sp| match sp.kind {
                RichSpanKind::Link(u) => Some(u),
                _ => None,
            })
            .collect();
        assert_eq!(
            card_links,
            vec![expected.to_string()],
            "text_card.rs must strip quotes/brackets for {input:?}"
        );
        assert!(parse_apple_deep_link(expected).is_some());
    }

    for invalid_bare in [
        "tg://",
        "tg://中文",
        "tg://🚀",
        "open tg:// now",
        "tg://\"",
        "tg:///",
        "[bad](tg://)",
        "[bad](tg://中文)",
    ] {
        let entity_links: Vec<String> = parse_rich_blocks(invalid_bare, &[])
            .into_iter()
            .flat_map(|b| match b {
                RichBlock::Paragraph(segs) => segs,
                _ => Vec::new(),
            })
            .filter_map(|s| s.flags.link_url)
            .collect();
        assert!(
            entity_links.is_empty(),
            "entities.rs must emit 0 links for {invalid_bare:?}, got {entity_links:?}"
        );

        let card_links: Vec<String> = parse_markdown_spans(invalid_bare)
            .into_iter()
            .filter_map(|sp| match sp.kind {
                RichSpanKind::Link(u) => Some(u),
                _ => None,
            })
            .collect();
        assert!(
            card_links.is_empty(),
            "text_card.rs must emit 0 links for {invalid_bare:?}, got {card_links:?}"
        );
    }
}

#[test]
fn test_m5_gpui_iter4_reg_ime_sync_deeplink_webapp_and_unauth_guards() {
    run_in_headless_subprocess(
        "test_m5_gpui_iter4_reg_ime_sync_deeplink_webapp_and_unauth_guards",
        true,
    );
}

fn body_iter4_reg_ime_sync_deeplink_webapp_and_unauth_guards(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    let draft = sample_book_creation(
        "cr_iter4_draft",
        1001,
        "Iter4 Title",
        "Line1",
        "Chapter one has plenty of readable text exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );

    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
        s.preferences.local_passcode_hash =
            Some(SettingsLocalPasscodeSections::hash_passcode_hex("1234"));
        s.content_hub.update(cx, |h, hcx| {
            h.upsert_my_creation(draft.clone());
            h.publish_modal.reviewer_access = ReviewerAccessState::Allowed;
            hcx.notify();
        });
    });

    // C1-1 (R2-1): PublishReviewModal Tab & Shift+Tab clear BOTH shell.ime and field preedit.
    shell.update(cx, |s, cx| {
        s.open_publish_review_modal_for_creation(draft.clone(), cx);
        s.update_ime_marked_text("zhong", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("zhong"));
        assert_eq!(
            s.content_hub.read(cx).publish_modal.title.ime_preedit.as_deref(),
            Some("zhong")
        );

        assert!(s.handle_non_char_key("tab", false, cx));
        assert!(s.ime.ime_preedit.is_none(), "Tab must clear shell.ime.ime_preedit");
        assert!(
            s.content_hub.read(cx).publish_modal.title.ime_preedit.is_none(),
            "Tab must clear title.ime_preedit"
        );
        assert_eq!(
            s.content_hub.read(cx).publish_modal.focused_field,
            Some(PublishModalField::Subtitle)
        );
        let subtitle_before = s.content_hub.read(cx).publish_modal.subtitle.text.clone();
        assert!(s.handle_non_char_key("enter", false, cx));
        assert_eq!(
            s.content_hub.read(cx).publish_modal.subtitle.text,
            subtitle_before,
            "Enter after Tab must NOT commit stale 'zhong' into Subtitle"
        );

        // Shift+Tab with preedit also clears both and moves back to Title.
        s.update_ime_marked_text("wen", cx);
        assert!(s.handle_non_char_key("tab", true, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(!s.content_hub.read(cx).publish_modal.has_any_preedit());
        assert_eq!(
            s.content_hub.read(cx).publish_modal.focused_field,
            Some(PublishModalField::Title)
        );

        // 1st Escape closes modal cleanly (not swallowed by ghost preedit).
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.is_publish_review_modal_active(cx));

        // Reviewer Queue tab: Tab also clears RejectReason preedit + shell.ime.
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.reviewer_access = ReviewerAccessState::Allowed;
        });
        s.open_reviewer_queue_modal(cx);
        s.update_ime_marked_text("tuihui", cx);
        assert!(s.handle_non_char_key("tab", false, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(
            s.content_hub
                .read(cx)
                .publish_modal
                .reject_reason
                .ime_preedit
                .is_none()
        );
        s.close_publish_review_modal(cx);
    });

    // C1-4: Multiline insert_committed_text("\n" / "\r\n" / "\x07\x00") & single-line "\n" absorption.
    shell.update(cx, |s, cx| {
        s.open_publish_review_modal_for_creation(draft.clone(), cx);
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.focused_field = Some(PublishModalField::Summary);
        });
        s.update_ime_marked_text("zhong", cx);
        s.insert_committed_text("\n", cx);
        assert_eq!(s.content_hub.read(cx).publish_modal.summary.text, "Line1\n");
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).publish_modal.summary.ime_preedit.is_none());

        s.update_ime_marked_text("zhong2", cx);
        s.insert_committed_text("\r\n", cx);
        assert_eq!(s.content_hub.read(cx).publish_modal.summary.text, "Line1\n\n");
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).publish_modal.summary.ime_preedit.is_none());

        s.update_ime_marked_text("zhong3", cx);
        s.insert_committed_text("\x07\x00", cx);
        assert_eq!(s.content_hub.read(cx).publish_modal.summary.text, "Line1\n\n");
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).publish_modal.summary.ime_preedit.is_none());

        // Single-line Title absorbs "\n" without mutating Title and clears both preedits.
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.focused_field = Some(PublishModalField::Title);
        });
        s.update_ime_marked_text("biaoti", cx);
        s.insert_committed_text("\n", cx);
        assert_eq!(s.content_hub.read(cx).publish_modal.title.text, "Iter4 Title");
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).publish_modal.title.ime_preedit.is_none());

        // C1-2: lock_app_now clears BOTH shell.ime and PublishReviewModal preedit while keeping modal open.
        s.update_ime_marked_text("lock_pre", cx);
        assert!(s.lock_app_now(cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(!s.content_hub.read(cx).publish_modal.has_any_preedit());
        assert!(s.try_unlock_with_passcode("1234", cx));
        assert!(s.is_publish_review_modal_active(cx));
        assert!(!s.content_hub.read(cx).publish_modal.has_any_preedit());
        s.close_publish_review_modal(cx);
    });

    // C1-2: focus_input switch, section switches, and handle_apple_deep_link (same & cross section).
    shell.update(cx, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
        s.update_ime_marked_text("cat_pre", cx);
        assert_eq!(
            s.content_hub.read(cx).catalog_search.ime_preedit.as_deref(),
            Some("cat_pre")
        );
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).catalog_search.ime_preedit.is_none());

        // Books -> Music clears QuickDrawerSearch preedit cleanly.
        s.update_ime_marked_text("qd_switch", cx);
        s.select_apple_sidebar_section(AppleSidebarSection::Music, cx);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).quick_drawer_search.ime_preedit.is_none());

        // Same-section handle_apple_deep_link (Music -> Music) clears CatalogSearch preedit.
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("same_sec_dl", cx);
        assert!(s.handle_apple_deep_link("tg://apple/music/cr_same", cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).catalog_search.ime_preedit.is_none());
    });

    // C1-3: Cmd+1, Cmd+J, Cmd+,, Cmd+N, OpenCreationDetail clear BOTH shell.ime and hub preedit & blur hub.
    shell.update(cx, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.composer.update(cx, |c, _| c.set_text(String::new()));
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("cmd1_pre", cx);
        assert!(s.handle_shortcut_key("1", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).catalog_search.ime_preedit.is_none());
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        s.commit_ime_unmark_text(cx);
        assert!(
            s.composer.read(cx).text().is_empty(),
            "commit_ime_unmark_text after Cmd+1 must NOT commit stale hub preedit into Composer"
        );

        // Cmd+J (Contacts modal) from ContentHub CatalogSearch
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("cmdj_pre", cx);
        assert!(s.handle_shortcut_key("j", false, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).catalog_search.ime_preedit.is_none());
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.handle_non_char_key("escape", false, cx));

        // Cmd+, (Settings) from ContentHub CatalogSearch
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("cmdcomma_pre", cx);
        assert!(s.handle_shortcut_key(",", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).catalog_search.ime_preedit.is_none());
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        s.close_settings(cx);

        // OpenCreationDetail clears preedit & blurs hub input
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("detail_pre", cx);
        s.handle_content_hub_event(
            &ContentHubEvent::OpenCreationDetail {
                section: ContentHubSection::Books,
                creation_id: "cr_iter4_draft".to_string(),
                action: Some("read".to_string()),
            },
            cx,
        );
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).catalog_search.ime_preedit.is_none());
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
    });

    // C1-5: Opening ChatSidebar Contacts modal over focused QuickDrawerSearch blurs QuickDrawerSearch & clears preedits.
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.quick_drawer_search.clear();
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("qd_contacts", cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.open_contacts_modal();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).quick_drawer_search.ime_preedit.is_none());
        s.update_ime_marked_text("bo", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("bo"));
        s.insert_committed_text("bob", cx);
        assert_eq!(s.sidebar.read(cx).contacts_search_query, "bob");
        s.sidebar.update(cx, |sb, scx| {
            sb.close_contacts_modal();
            scx.notify();
        });
    });

    // C1-6: Re-focusing ConversationStage search after FocusInputTaken (and zero focus steal on incoming messages).
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.search_query.clear();
            st.search_open = true;
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.quick_drawer_search.clear();
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::QuickDrawerSearch
        );
        assert!(
            !s.stage.read(cx).search_open,
            "FocusInputTaken must reset stage.search_open so re-opening fires cleanly"
        );
        // Simulate incoming chat notification on stage: must NOT steal focus from QuickDrawerSearch.
        s.stage.update(cx, |_st, stcx| stcx.notify());
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::QuickDrawerSearch,
            "stage notify from incoming message must NOT steal focus from QuickDrawerSearch"
        );
        s.update_ime_marked_text("qd_stage_pre", cx);
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).quick_drawer_search.ime_preedit.is_none());
        s.insert_committed_text("needle", cx);
        assert_eq!(s.stage.read(cx).search_query, "needle");
        s.stage.update(cx, |st, stcx| {
            st.search_open = false;
            stcx.notify();
        });
    });

    // R2-I4-2: Bidirectional Apple Compose (is_plus_menu_open) <-> QuickDrawerSearch focus, IME, Escape & return clicks.
    shell.update(cx, |s, cx| {
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
        assert!(s.theme_state.palette.is_apple_style());
    });
    shell.update(cx, |s, cx| {
        // Direct un-flushed is_plus_menu_open = true + FocusInputTaken must NOT spuriously notify sidebar and steal focus back.
        s.sidebar.update(cx, |sb, _| {
            sb.is_plus_menu_open = true;
            sb.search_focused = false;
        });
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.quick_drawer_search.clear();
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::QuickDrawerSearch,
            "FocusInputTaken must not emit spurious sidebar notify that steals QuickDrawerSearch focus"
        );
        assert!(s.is_content_hub_input_active(cx));
        assert!(!s.is_apple_compose_input_active(cx));
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    // Flushed Apple Compose -> QuickDrawerSearch -> Escape -> Return-click To: & Group name:.
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.is_apple_compose_input_active(cx));
        s.insert_committed_text("bob", cx);
        assert_eq!(s.sidebar.read(cx).compose_recipient_query, "bob");
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.quick_drawer_search.clear();
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.sidebar.read(cx).is_plus_menu_open);
        assert!(s.is_content_hub_input_active(cx));
        assert!(!s.is_apple_compose_input_active(cx));
        s.update_ime_marked_text("xing", cx);
        assert_eq!(s.active_text_and_preedit(cx).1.as_deref(), Some("xing"));
        assert_eq!(
            s.content_hub.read(cx).quick_drawer_search.ime_preedit.as_deref(),
            Some("xing")
        );
        s.insert_committed_text("星际", cx);
        s.insert_committed_text("Atlas", cx);
        assert!(s.handle_non_char_key("backspace", false, cx));
        assert_eq!(s.content_hub.read(cx).quick_drawer_search.text, "星际Atla");
        assert_eq!(s.sidebar.read(cx).compose_recipient_query, "bob");
        assert!(s.sidebar.read(cx).compose_draft.custom_group_title.is_empty());
        assert!(s.handle_shortcut_key("a", false, cx));
        s.insert_committed_text("替换", cx);
        assert_eq!(s.content_hub.read(cx).quick_drawer_search.text, "替换");
        assert_eq!(s.sidebar.read(cx).compose_recipient_query, "bob");

        // Even with compose_title_focused == true, QuickDrawerSearch keeps typing/IME ownership.
        s.sidebar.update(cx, |sb, _| sb.compose_title_focused = true);
        assert!(!s.is_apple_compose_input_active(cx));
        assert!(s.is_content_hub_input_active(cx));
        s.update_ime_marked_text("qd_t", cx);
        assert_eq!(
            s.content_hub.read(cx).quick_drawer_search.ime_preedit.as_deref(),
            Some("qd_t")
        );
        s.clear_all_ime_preedits(cx);

        // Escape with no preedit while QuickDrawerSearch is focused closes Quick Drawer, leaving Apple Compose intact.
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.content_hub.read(cx).quick_drawer_open);
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.sidebar.read(cx).is_plus_menu_open);
        assert_eq!(s.sidebar.read(cx).compose_recipient_query, "bob");
        assert!(s.is_apple_compose_input_active(cx));

        // Return click to Apple Compose To: field blurs QuickDrawerSearch & clears IME.
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("qd_to_pre", cx);
        s.focus_apple_compose_to_field(cx);
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).quick_drawer_search.ime_preedit.is_none());
        assert!(s.is_apple_compose_input_active(cx));
        assert!(!s.sidebar.read(cx).compose_title_focused);
        s.insert_committed_text("_to", cx);
        assert_eq!(s.sidebar.read(cx).compose_recipient_query, "bob_to");
        assert_eq!(s.content_hub.read(cx).quick_drawer_search.text, "替换");

        // Return click to Apple Compose Group name: field blurs QuickDrawerSearch & clears IME.
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("qd_title_pre", cx);
        s.focus_apple_compose_title_field(cx);
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).quick_drawer_search.ime_preedit.is_none());
        assert!(s.is_apple_compose_input_active(cx));
        assert!(s.sidebar.read(cx).compose_title_focused);
        s.insert_committed_text("太空群", cx);
        assert_eq!(s.sidebar.read(cx).compose_draft.custom_group_title, "太空群");
        assert_eq!(s.content_hub.read(cx).quick_drawer_search.text, "替换");
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });

    // R2-I4-P1: Stage search falling-edge close clears IME (incl. pre-start bot chat), while deferred flush_effects never clobbers fresh QuickDrawerSearch preedit.
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.show_bot_start_bar = false;
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("中文", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("中文"));
        s.stage.update(cx, |st, stcx| {
            st.apply_topbar_action(TopBarUiAction::SearchClear);
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "closing stage search via TopBarUiAction::SearchClear must clear shell IME preedit"
        );
        // Repeat in pre-start bot chat (show_bot_start_bar == true -> should_hide_composer == true).
        s.stage.update(cx, |st, stcx| {
            st.show_bot_start_bar = true;
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("bot_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("bot_pre"));
        s.stage.update(cx, |st, stcx| {
            st.apply_topbar_action(TopBarUiAction::SearchClear);
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "closing stage search in pre-start bot chat must still clear shell IME preedit"
        );
        // GPUI flush_effects() timing proof: flushed stage.search_open -> single-update FocusInputTaken + synchronous QuickDrawerSearch IME.
        s.stage.update(cx, |st, stcx| {
            st.show_bot_start_bar = false;
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.stage.read(cx).search_open);
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, _| {
            h.active_input = ContentHubInputTarget::QuickDrawerSearch;
        });
        s.handle_content_hub_event(&ContentHubEvent::FocusInputTaken, cx);
        s.update_ime_marked_text("keep_me", cx);
    });
    shell.update(cx, |s, cx| {
        assert!(!s.stage.read(cx).search_open);
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::QuickDrawerSearch
        );
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("keep_me"),
            "deferred Effect::Notify(stage) falling edge must NOT wipe fresh QuickDrawerSearch preedit"
        );
        assert_eq!(
            s.content_hub.read(cx).quick_drawer_search.ime_preedit.as_deref(),
            Some("keep_me")
        );

        // LaunchMiniApp blur catch: launching MiniApp blurs occluded QuickDrawerSearch & clears IME preedits.
        s.handle_content_hub_event(
            &ContentHubEvent::LaunchMiniApp {
                bot_username: "demo_bot".to_string(),
                short_name: "demo_app".to_string(),
                web_url: Some("https://example.com/miniapp".to_string()),
            },
            cx,
        );
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.content_hub.read(cx).quick_drawer_search.ime_preedit.is_none());
        s.miniapps.miniapp_overlay = None;
        s.miniapps.is_miniapp_minimized = false;
    });

    // C2-I3-2 + C2-I3-1 Zero-Leak: valid url:-prefixed callbacks route cleanly; WebApp & smuggled payloads swallow cleanly.
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.execute_hermes_inline_callback("url:tg://apple/books/cr_valid_url_cb".to_string(), cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_valid_url_cb")
        );

        s.execute_hermes_inline_callback(
            "URL:creation:open:music:cr_valid_alias_cb".to_string(),
            cx,
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Music)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_valid_alias_cb")
        );

        s.handle_stage_bot_button(
            1,
            InlineButtonType::Callback("url:tg://apple/games/cr_valid_btn_cb".to_string()),
            cx,
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Games)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_valid_btn_cb")
        );

        // Zero-leak tests for WebApp, external URL, media_gal, show_media_viewer, iv_url, and handle_deep_link.
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        let msg_count_before = s.stage.read(cx).messages.len();
        s.window_runtime.controller.clear_all_toasts();
        for bad in [
            "tg://apple/books/<script>alert(1)</script>",
            "\u{00AD}tg://apple/books/cr_1",
            "\u{3164}tg://apple/books/cr_1",
            "\u{2800}tg://apple/books/cr_1",
            "url:url:tg://apple/books/cr_1",
        ] {
            s.handle_stage_bot_button(1, InlineButtonType::WebApp(bad.to_string()), cx);
            s.open_external_url_with_toast(bad, cx);
            s.show_media_viewer(bad, false, cx);
            s.stage.update(cx, |st, _| {
                st.pending_media_viewer = Some((vec![(bad.to_string(), false, None)], 0));
                st.pending_iv_url = Some((bad.to_string(), "Bad IV".to_string()));
            });
            s.handle_stage_pending_actions(cx);
            assert!(
                !s.handle_deep_link(bad, cx),
                "handle_deep_link must return false for malformed/smuggled {bad:?}"
            );
            assert_eq!(s.stage.read(cx).messages.len(), msg_count_before);
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "zero toast leak for {bad:?}"
            );
            assert!(s.overlays.media_viewer_overlay.is_none());
            assert!(s.overlays.iv_viewer_overlay.is_none());
            assert!(s.miniapps.miniapp_overlay.is_none());
        }
    });

    // C2-I3-4 + Unauthenticated siblings: zero mutation of AuthWizard preedit, modal errors, or request gens.
    shell.update(cx, |s, cx| {
        s.logout(cx);
        assert!(!s.is_authenticated);
        s.ime.ime_preedit = Some("auth_pre".to_string());
        let creations_gen_before = s.content_hub_state.creations_request_gen;
        let review_gen_before = s.content_hub_state.review_queue_request_gen;

        s.submit_review_decision_fenced(
            "cr_unauth".to_string(),
            ReviewDecisionRequest {
                decision: ReviewDecision::Reject,
                visibility: None,
                reason: None,
                notes: None,
            },
            cx,
        );
        s.save_creation_draft_fenced(
            "cr_unauth".to_string(),
            UpdateCreationRequest::default(),
            cx,
        );
        s.submit_creation_for_review_fenced(
            "cr_unauth".to_string(),
            None,
            SubmitReviewRequest {
                visibility: Some(CreationVisibility::Public),
                notes: None,
            },
            cx,
        );
        s.handle_content_hub_event(&ContentHubEvent::FocusInputTaken, cx);
        s.handle_content_hub_event(&ContentHubEvent::CloseQuickDrawer, cx);
        s.handle_content_hub_event(&ContentHubEvent::ClosePublishReviewModal, cx);
        s.handle_content_hub_event(
            &ContentHubEvent::LaunchMiniApp {
                bot_username: "bot".to_string(),
                short_name: "app".to_string(),
                web_url: Some("https://example.com".to_string()),
            },
            cx,
        );
        s.handle_apple_sidebar_event(&AppleSidebarEvent::ToggleCreationsDrawer(false), cx);
        assert!(!s.open_publish_review_modal_for_id("cr_iter4_draft", cx));
        s.refresh_my_creations(cx);
        s.probe_reviewer_queue(cx, true);

        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("auth_pre"),
            "unauthenticated hub/sidebar handlers must NOT wipe shell.ime preedit"
        );
        assert!(
            s.content_hub.read(cx).publish_modal.reviewer_error.is_none(),
            "unauthenticated submit_review_decision_fenced must NOT mutate reviewer_error"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert!(!s.content_hub.read(cx).quick_drawer_open);
        assert_eq!(s.content_hub_state.creations_request_gen, creations_gen_before);
        assert_eq!(s.content_hub_state.review_queue_request_gen, review_gen_before);
    });
}

#[test]
fn test_m5_gpui_iter4_reg_async_review_queue_prune_and_ws_monotonicity() {
    run_in_headless_subprocess(
        "test_m5_gpui_iter4_reg_async_review_queue_prune_and_ws_monotonicity",
        false,
    );
}

async fn body_iter4_reg_async_review_queue_prune_and_ws_monotonicity(mut app: AsyncApp) {
    let own_del = sample_book_creation(
        "cr_own_del",
        1001,
        "Own Pending Delete",
        "Pending creation owned by admin that will be deleted.",
        "Chapter one body for own pending delete exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        2,
    );
    let own_wd = sample_book_creation(
        "cr_own_wd",
        1001,
        "Own Pending Withdraw",
        "Pending creation owned by admin that will be withdrawn.",
        "Chapter one body for own pending withdraw exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        2,
    );
    let other_q = sample_book_creation(
        "cr_other_q",
        2002,
        "Foreign Pending Item",
        "Pending creation owned by another user in the admin queue.",
        "Chapter one body for foreign pending item exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        3,
    );
    let own_pub = sample_book_creation(
        "cr_pub_own",
        1001,
        "Own Published Creation",
        "Published creation owned by admin at cloud revision 5.",
        "Chapter one body for published creation exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Published,
        5,
    );

    let mock_state = Arc::new(MockM5State::new(
        true,
        vec![own_del.clone(), own_wd.clone(), own_pub.clone()],
        vec![own_del.clone(), own_wd.clone(), other_q.clone()],
    ));
    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "admin"), cx);
    });
    wait_until(
        &mut app,
        60,
        "iter4 initial creations and 3-item review queue sync",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.find_my_creation("cr_own_del").is_some()
                        && h.find_my_creation("cr_own_wd").is_some()
                        && h.find_my_creation("cr_pub_own").is_some()
                        && h.publish_modal.review_queue.len() == 3
                        && s.apple_sidebar.read(cx).pending_review_count == 3
                })
                .unwrap_or(false)
        },
    )
    .await;

    // C2-I3-1 Live HTTP: ignorable/blank-filler rejection reasons send 0 HTTP requests.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_reviewer_queue_modal(cx);
    });
    wait_until(&mut app, 60, "iter4 reviewer queue modal probe finished", |a| {
        shell
            .read_with(a, |s, cx| {
                let m = &s.content_hub.read(cx).publish_modal;
                !m.is_loading_queue && m.review_queue.len() == 3
            })
            .unwrap_or(false)
    })
    .await;
    for ignorable in ["\u{00AD}", "\u{3164}", "\u{2800}", "\u{FE0F}", "\u{E0001}"] {
        let _ = shell.update(&mut app, |s, cx| {
            s.content_hub.update(cx, |h, _| {
                h.publish_modal.is_submitting = true;
                h.publish_modal.reviewer_error = None;
            });
            s.submit_review_decision_fenced(
                "cr_other_q".to_string(),
                ReviewDecisionRequest {
                    decision: ReviewDecision::Reject,
                    visibility: None,
                    reason: Some(ignorable.to_string()),
                    notes: None,
                },
                cx,
            );
            let m = &s.content_hub.read(cx).publish_modal;
            assert!(!m.is_submitting);
            assert!(m.reviewer_error.is_some());
        });
    }
    assert!(
        mock_state.review_decisions_received.lock().unwrap().is_empty(),
        "0 HTTP review decisions may be sent for ignorable/blank-filler reasons"
    );

    // C2-I3-5a (1): delete_creation_fenced on own pending creation prunes review_queue, reconciles
    // selected_review_id, updates pending_review_count badge, closes Submit modal & clears IME.
    let _ = shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(own_del.clone(), cx);
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.selected_review_id = Some("cr_own_del".to_string());
        });
        s.update_ime_marked_text("del_own_pre", cx);
        s.delete_creation_fenced("cr_own_del".to_string(), cx);
    });
    wait_until(&mut app, 60, "delete_creation_fenced prunes cr_own_del from creations & review_queue", |a| {
        shell
            .read_with(a, |s, cx| {
                let h = s.content_hub.read(cx);
                h.find_my_creation("cr_own_del").is_none()
                    && !h.publish_modal.review_queue.iter().any(|c| c.creation_id == "cr_own_del")
            })
            .unwrap_or(false)
    })
    .await;
    let (q_len_1, sel_1, badge_1, modal_open_1, shell_ime_1, modal_pre_1) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.publish_modal.review_queue.len(),
                h.publish_modal.selected_review_id.clone(),
                s.apple_sidebar.read(cx).pending_review_count,
                h.publish_modal.is_open,
                s.ime.ime_preedit.clone(),
                h.publish_modal.has_any_preedit(),
            )
        })
        .unwrap();
    assert_eq!(q_len_1, 2);
    assert_eq!(sel_1.as_deref(), Some("cr_own_wd"));
    assert_eq!(badge_1, 2);
    assert!(!modal_open_1);
    assert!(shell_ime_1.is_none());
    assert!(!modal_pre_1);

    // C2-I3-5a (2): withdraw_creation_review_fenced prunes cr_own_wd from review_queue & reconciles selection.
    let _ = shell.update(&mut app, |s, cx| {
        s.withdraw_creation_review_fenced("cr_own_wd".to_string(), cx);
    });
    wait_until(&mut app, 60, "withdraw_creation_review_fenced transitions cr_own_wd to Draft and prunes queue", |a| {
        shell
            .read_with(a, |s, cx| {
                let h = s.content_hub.read(cx);
                h.find_my_creation("cr_own_wd")
                    .is_some_and(|c| c.review_status == CreationReviewStatus::Draft)
                    && h.publish_modal.review_queue.len() == 1
            })
            .unwrap_or(false)
    })
    .await;
    let (sel_2, badge_2) = shell
        .read_with(&app, |s, cx| {
            (
                s.content_hub.read(cx).publish_modal.selected_review_id.clone(),
                s.apple_sidebar.read(cx).pending_review_count,
            )
        })
        .unwrap();
    assert_eq!(sel_2.as_deref(), Some("cr_other_q"));
    assert_eq!(badge_2, 1);

    // C2-I3-5a (3): set_my_creations remote disappearance (no tombstone) prunes vanished owned pending item while preserving foreign cr_other_q.
    let own_remote_gone = sample_book_creation(
        "cr_own_remote_gone",
        1001,
        "Own Remote Gone",
        "Pending creation that will disappear from full sync without tombstone.",
        "Chapter one body for remote disappeared creation exceeding fifty non-whitespace characters.",
        CreationReviewStatus::PendingReview,
        4,
    );
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.my_creations.push(own_remote_gone.clone());
            h.publish_modal.review_queue.insert(0, own_remote_gone.clone());
            h.publish_modal.selected_review_id = Some("cr_own_remote_gone".to_string());
            // Full sync returns active items without cr_own_remote_gone (no tombstone).
            let remaining: Vec<UserCreationDto> = h
                .my_creations
                .iter()
                .filter(|c| c.creation_id != "cr_own_remote_gone")
                .cloned()
                .collect();
            h.set_my_creations(remaining);
            hcx.notify();
        });
        s.sync_apple_sidebar_badges(cx);
        let h = s.content_hub.read(cx);
        assert_eq!(h.publish_modal.review_queue.len(), 1);
        assert_eq!(h.publish_modal.review_queue[0].creation_id, "cr_other_q");
        assert_eq!(
            h.publish_modal.selected_review_id.as_deref(),
            Some("cr_other_q")
        );
        assert_eq!(s.apple_sidebar.read(cx).pending_review_count, 1);
    });

    // C2-I3-5b + Siblings: Out-of-order WS UserCreationChanged and stale upsert_my_creation never regress revision/status.
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_ws_user_creation_changed(
            "cr_pub_own".to_string(),
            1001,
            CreationKind::Book,
            CreationReviewStatus::PendingReview,
            2,
            cx,
        );
        let cur = s
            .content_hub
            .read(cx)
            .find_my_creation("cr_pub_own")
            .cloned()
            .expect("cr_pub_own exists");
        assert_eq!(
            cur.review_status,
            CreationReviewStatus::Published,
            "stale rev 2 WS frame must NOT regress Published rev 5 creation"
        );
        assert_eq!(cur.cloud_revision, 5);

        // upsert_my_creation ignores stale rev 3 Draft DTO, accepts rev 6 Unpublished DTO.
        let mut stale_dto = cur.clone();
        stale_dto.review_status = CreationReviewStatus::Draft;
        stale_dto.cloud_revision = 3;
        s.content_hub.update(cx, |h, _| h.upsert_my_creation(stale_dto));
        let after_stale = s
            .content_hub
            .read(cx)
            .find_my_creation("cr_pub_own")
            .cloned()
            .expect("cr_pub_own exists");
        assert_eq!(after_stale.review_status, CreationReviewStatus::Published);
        assert_eq!(after_stale.cloud_revision, 5);

        let mut fresh_dto = cur;
        fresh_dto.review_status = CreationReviewStatus::Unpublished;
        fresh_dto.cloud_revision = 6;
        s.content_hub.update(cx, |h, _| h.upsert_my_creation(fresh_dto));
        let after_fresh = s
            .content_hub
            .read(cx)
            .find_my_creation("cr_pub_own")
            .cloned()
            .expect("cr_pub_own exists");
        assert_eq!(after_fresh.review_status, CreationReviewStatus::Unpublished);
        assert_eq!(after_fresh.cloud_revision, 6);

        // Non-stale WS UserCreationChanged(Draft, rev 6) for queued cr_other_q prunes review_queue immediately.
        s.handle_ws_user_creation_changed(
            "cr_other_q".to_string(),
            2002,
            CreationKind::Book,
            CreationReviewStatus::Draft,
            6,
            cx,
        );
        assert!(
            s.content_hub.read(cx).publish_modal.review_queue.is_empty(),
            "WS transition of queued item to Draft must prune review_queue immediately"
        );
        assert_eq!(s.apple_sidebar.read(cx).pending_review_count, 0);

        // WS PublicCatalogChanged("removed") for Book while viewing Books also bumps appstore_catalog_gen.
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        let appstore_gen_before = s.content_hub_state.appstore_catalog_gen;
        s.handle_ws_public_catalog_changed(
            "cr_pub_own".to_string(),
            CreationKind::Book,
            "removed".to_string(),
            cx,
        );
        assert!(
            s.content_hub_state.appstore_catalog_gen > appstore_gen_before,
            "removed WS event must bump appstore_catalog_gen even when AppStore is not active"
        );
    });
}

#[test]
fn test_m5_gpui_iter5_reg_deterministic_async_fencing_and_detail_monotonicity() {
    run_in_headless_subprocess(
        "test_m5_gpui_iter5_reg_deterministic_async_fencing_and_detail_monotonicity",
        false,
    );
}

async fn drain_async_cycles(app: &mut AsyncApp) {
    for _ in 0..5 {
        app.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

async fn body_iter5_reg_deterministic_async_fencing_and_detail_monotonicity(mut app: AsyncApp) {
    let own_del = sample_book_creation(
        "cr_own_del",
        1001,
        "Own Pending Delete",
        "Pending creation owned by admin that will be deleted.",
        "Chapter one body for own pending delete exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        2,
    );
    let own_wd = sample_book_creation(
        "cr_own_wd",
        1001,
        "Own Pending Withdraw",
        "Pending creation owned by admin that will be withdrawn.",
        "Chapter one body for own pending withdraw exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        2,
    );
    let other_q = sample_book_creation(
        "cr_other_q",
        2002,
        "Foreign Pending Item",
        "Pending creation owned by another user in the admin queue.",
        "Chapter one body for foreign pending item exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        3,
    );
    let pub_unpub = sample_book_creation(
        "cr_pub_unpub",
        1001,
        "Published Unpub Book",
        "Published creation owned by admin ready to be unpublished live.",
        "Chapter one body for published unpub book exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Published,
        4,
    );
    let detail_mono = sample_book_creation(
        "cr_detail_mono",
        1001,
        "Detail Mono Draft",
        "Draft creation owned by admin for detail and sync monotonicity testing.",
        "Chapter one body for detail mono draft exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        2,
    );

    let mock_state = Arc::new(MockM5State::new(
        true,
        vec![
            own_del.clone(),
            own_wd.clone(),
            pub_unpub.clone(),
            detail_mono.clone(),
        ],
        vec![own_del.clone(), own_wd.clone(), other_q.clone()],
    ));
    mock_state
        .catalog_books
        .lock()
        .unwrap()
        .push(sample_public_book_item("cr_pub_unpub", "Published Unpub Book"));

    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "admin"), cx);
    });
    wait_until(
        &mut app,
        60,
        "iter5 initial creations and 3-item review queue sync",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    !h.is_loading_creations
                        && !h.publish_modal.is_loading_queue
                        && h.find_my_creation("cr_own_del").is_some()
                        && h.find_my_creation("cr_own_wd").is_some()
                        && h.find_my_creation("cr_pub_unpub").is_some()
                        && h.find_my_creation("cr_detail_mono").is_some()
                        && h.publish_modal.review_queue.len() == 3
                        && s.apple_sidebar.read(cx).pending_review_count == 3
                })
                .unwrap_or(false)
        },
    )
    .await;

    // Phase 1A (R1-I4-1 / R2-I4-1): Delayed pre-delete GET /review-queue landing after delete_creation_fenced is fenced out.
    mock_state
        .review_queue_delay_ms
        .store(120, Ordering::SeqCst);
    let s0 = mock_state
        .review_queue_requests_started
        .load(Ordering::SeqCst);
    let r0 = mock_state
        .review_queue_responses_served
        .load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.probe_reviewer_queue(cx, true);
    });
    wait_until(&mut app, 60, "phase 1A delayed review-queue started", |_| {
        mock_state
            .review_queue_requests_started
            .load(Ordering::SeqCst)
            > s0
    })
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        s.delete_creation_fenced("cr_own_del".to_string(), cx);
    });
    wait_until(
        &mut app,
        60,
        "phase 1A delete_creation_fenced prunes cr_own_del and resets is_loading_queue",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.find_my_creation("cr_own_del").is_none()
                        && !h
                            .publish_modal
                            .review_queue
                            .iter()
                            .any(|c| c.creation_id == "cr_own_del")
                        && !h.publish_modal.is_loading_queue
                })
                .unwrap_or(false)
        },
    )
    .await;
    wait_until(&mut app, 60, "phase 1A stale review-queue served", |_| {
        mock_state
            .review_queue_responses_served
            .load(Ordering::SeqCst)
            > r0
    })
    .await;
    drain_async_cycles(&mut app).await;
    let (q_ids_1a, badge_1a, loading_1a, own_del_1a) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.publish_modal
                    .review_queue
                    .iter()
                    .map(|c| c.creation_id.clone())
                    .collect::<Vec<_>>(),
                s.apple_sidebar.read(cx).pending_review_count,
                h.publish_modal.is_loading_queue,
                h.find_my_creation("cr_own_del").is_some(),
            )
        })
        .unwrap();
    assert_eq!(q_ids_1a.len(), 2);
    assert!(!q_ids_1a.iter().any(|id| id == "cr_own_del"));
    assert_eq!(badge_1a, 2);
    assert!(!loading_1a);
    assert!(!own_del_1a);

    // Phase 1B: Delayed pre-withdraw GET /review-queue landing after withdraw_creation_review_fenced is fenced out.
    let s1 = mock_state
        .review_queue_requests_started
        .load(Ordering::SeqCst);
    let r1 = mock_state
        .review_queue_responses_served
        .load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.probe_reviewer_queue(cx, true);
    });
    wait_until(&mut app, 60, "phase 1B delayed review-queue started", |_| {
        mock_state
            .review_queue_requests_started
            .load(Ordering::SeqCst)
            > s1
    })
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        s.withdraw_creation_review_fenced("cr_own_wd".to_string(), cx);
    });
    wait_until(
        &mut app,
        60,
        "phase 1B withdraw_creation_review_fenced transitions cr_own_wd to Draft",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.find_my_creation("cr_own_wd")
                        .is_some_and(|c| c.review_status == CreationReviewStatus::Draft)
                        && h.publish_modal.review_queue.len() == 1
                        && !h.publish_modal.is_loading_queue
                })
                .unwrap_or(false)
        },
    )
    .await;
    wait_until(&mut app, 60, "phase 1B stale review-queue served", |_| {
        mock_state
            .review_queue_responses_served
            .load(Ordering::SeqCst)
            > r1
    })
    .await;
    drain_async_cycles(&mut app).await;
    let (wd_status_1b, q_ids_1b, badge_1b, loading_1b) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.find_my_creation("cr_own_wd").map(|c| c.review_status),
                h.publish_modal
                    .review_queue
                    .iter()
                    .map(|c| c.creation_id.clone())
                    .collect::<Vec<_>>(),
                s.apple_sidebar.read(cx).pending_review_count,
                h.publish_modal.is_loading_queue,
            )
        })
        .unwrap();
    assert_eq!(wd_status_1b, Some(CreationReviewStatus::Draft));
    assert_eq!(q_ids_1b, vec!["cr_other_q".to_string()]);
    assert_eq!(badge_1b, 1);
    assert!(!loading_1b);

    // Phase 1C: Delayed pre-decision GET /review-queue landing after submit_review_decision_fenced (foreign cr_other_q) is fenced out.
    let s2 = mock_state
        .review_queue_requests_started
        .load(Ordering::SeqCst);
    let r2 = mock_state
        .review_queue_responses_served
        .load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.probe_reviewer_queue(cx, true);
    });
    wait_until(&mut app, 60, "phase 1C delayed review-queue started", |_| {
        mock_state
            .review_queue_requests_started
            .load(Ordering::SeqCst)
            > s2
    })
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        s.submit_review_decision_fenced(
            "cr_other_q".to_string(),
            ReviewDecisionRequest {
                decision: ReviewDecision::Reject,
                visibility: None,
                reason: Some("Needs metadata".to_string()),
                notes: None,
            },
            cx,
        );
    });
    wait_until(
        &mut app,
        60,
        "phase 1C submit_review_decision_fenced prunes foreign cr_other_q",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.publish_modal.review_queue.is_empty() && !h.publish_modal.is_loading_queue
                })
                .unwrap_or(false)
        },
    )
    .await;
    wait_until(&mut app, 60, "phase 1C stale review-queue served", |_| {
        mock_state
            .review_queue_responses_served
            .load(Ordering::SeqCst)
            > r2
    })
    .await;
    drain_async_cycles(&mut app).await;
    let (q_empty_1c, badge_1c, loading_1c) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.publish_modal.review_queue.is_empty(),
                s.apple_sidebar.read(cx).pending_review_count,
                h.publish_modal.is_loading_queue,
            )
        })
        .unwrap();
    assert!(q_empty_1c);
    assert_eq!(badge_1c, 0);
    assert!(!loading_1c);
    mock_state.review_queue_delay_ms.store(0, Ordering::SeqCst);

    // Phase 1D: Live unpublish_creation_fenced via POST /unpublish bumps creations/books/appstore gens & evicts catalog.
    let (creations_gen_0, books_gen_0, appstore_gen_0) = shell
        .update(&mut app, |s, cx| {
            s.content_hub.update(cx, |h, hcx| {
                h.books_catalog = vec![sample_public_book_item(
                    "cr_pub_unpub",
                    "Published Unpub Book",
                )];
                // Verify set_section resets is_loading_catalog to prevent stranded spinner.
                h.is_loading_catalog = true;
                h.set_section(ContentHubSection::MyCreations);
                assert!(!h.is_loading_catalog);
                hcx.notify();
            });
            let before = (
                s.content_hub_state.creations_request_gen,
                s.content_hub_state.books_catalog_gen,
                s.content_hub_state.appstore_catalog_gen,
            );
            s.unpublish_creation_fenced("cr_pub_unpub".to_string(), cx);
            before
        })
        .unwrap();
    wait_until(
        &mut app,
        60,
        "phase 1D unpublish_creation_fenced updates status & evicts books_catalog",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.find_my_creation("cr_pub_unpub")
                        .is_some_and(|c| c.review_status == CreationReviewStatus::Unpublished)
                        && h.books_catalog.is_empty()
                })
                .unwrap_or(false)
        },
    )
    .await;
    let (creations_gen_1, books_gen_1, appstore_gen_1) = shell
        .read_with(&app, |s, _| {
            (
                s.content_hub_state.creations_request_gen,
                s.content_hub_state.books_catalog_gen,
                s.content_hub_state.appstore_catalog_gen,
            )
        })
        .unwrap();
    assert!(creations_gen_1 > creations_gen_0);
    assert!(books_gen_1 > books_gen_0);
    assert!(appstore_gen_1 > appstore_gen_0);
    assert!(
        !mock_state
            .catalog_books
            .lock()
            .unwrap()
            .iter()
            .any(|i| i.creation_id == "cr_pub_unpub"),
        "mock POST /unpublish must prune catalog_books"
    );

    // Phase 1E: Delayed pre-save GET /creations/sync landing after save_creation_draft_fenced is fenced out.
    mock_state
        .creations_sync_delay_ms
        .store(120, Ordering::SeqCst);
    let cs0 = mock_state
        .creations_sync_requests_started
        .load(Ordering::SeqCst);
    let cr0 = mock_state
        .creations_sync_responses_served
        .load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.refresh_my_creations(cx);
    });
    wait_until(&mut app, 60, "phase 1E delayed creations/sync started", |_| {
        mock_state
            .creations_sync_requests_started
            .load(Ordering::SeqCst)
            > cs0
    })
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        s.save_creation_draft_fenced(
            "cr_detail_mono".to_string(),
            UpdateCreationRequest {
                expected_revision: Some(2),
                title: Some("Saved Rev 3 Title".to_string()),
                ..Default::default()
            },
            cx,
        );
    });
    wait_until(
        &mut app,
        60,
        "phase 1E save_creation_draft_fenced advances cr_detail_mono to rev 3",
        |a| {
            shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.find_my_creation("cr_detail_mono").is_some_and(|c| {
                        c.cloud_revision == 3 && c.title == "Saved Rev 3 Title"
                    }) && !h.is_loading_creations
                })
                .unwrap_or(false)
        },
    )
    .await;
    wait_until(&mut app, 60, "phase 1E stale creations/sync served", |_| {
        mock_state
            .creations_sync_responses_served
            .load(Ordering::SeqCst)
            > cr0
    })
    .await;
    drain_async_cycles(&mut app).await;
    let (saved_rev, saved_title, loading_creations_1e) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            let c = h.find_my_creation("cr_detail_mono").expect("cr_detail_mono");
            (c.cloud_revision, c.title.clone(), h.is_loading_creations)
        })
        .unwrap();
    assert_eq!(saved_rev, 3);
    assert_eq!(saved_title, "Saved Rev 3 Title");
    assert!(!loading_creations_1e);
    mock_state
        .creations_sync_delay_ms
        .store(0, Ordering::SeqCst);

    // Phase 2A (R1-I4-2): Owned stale detail GET (rev 3) landing after rev 5 Published does NOT regress selected_detail_creation.
    mock_state.detail_delay_ms.store(120, Ordering::SeqCst);
    let d0 = mock_state.detail_requests_started.load(Ordering::SeqCst);
    let dr0 = mock_state.detail_responses_served.load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.selected_creation_id = Some("cr_detail_mono".to_string());
            h.selected_detail_creation = None;
        });
        s.fetch_creation_detail_fenced("cr_detail_mono".to_string(), cx);
    });
    wait_until(&mut app, 60, "phase 2A delayed owned detail started", |_| {
        mock_state.detail_requests_started.load(Ordering::SeqCst) > d0
    })
    .await;
    let rev5_owned = sample_book_creation(
        "cr_detail_mono",
        1001,
        "Detail Mono Published Rev 5",
        "Published rev 5 summary exceeding ten characters cleanly.",
        "Chapter one body for detail mono rev 5 exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Published,
        5,
    );
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.upsert_my_creation(rev5_owned.clone());
            hcx.notify();
        });
        let cur_det = s
            .content_hub
            .read(cx)
            .selected_detail_creation
            .as_ref()
            .expect("detail populated at rev 5");
        assert_eq!(cur_det.cloud_revision, 5);
        assert_eq!(cur_det.review_status, CreationReviewStatus::Published);
    });
    wait_until(&mut app, 60, "phase 2A stale owned detail served", |_| {
        mock_state.detail_responses_served.load(Ordering::SeqCst) > dr0
    })
    .await;
    drain_async_cycles(&mut app).await;
    let (det_rev_2a, det_st_2a, own_rev_2a, own_st_2a) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            let det = h.selected_detail_creation.as_ref().expect("detail still present");
            let own = h.find_my_creation("cr_detail_mono").expect("owned still present");
            (det.cloud_revision, det.review_status, own.cloud_revision, own.review_status)
        })
        .unwrap();
    assert_eq!(det_rev_2a, 5);
    assert_eq!(det_st_2a, CreationReviewStatus::Published);
    assert_eq!(own_rev_2a, 5);
    assert_eq!(own_st_2a, CreationReviewStatus::Published);

    // Phase 2B (R1-I4-2): Non-owned stale detail GET (rev 2 Draft) landing after rev 5 Published does NOT regress selected_detail_creation.
    let foreign_rev2 = sample_book_creation(
        "cr_foreign_mono",
        9999,
        "Foreign Stale Rev 2",
        "Foreign stale rev 2 summary exceeding ten characters cleanly.",
        "Chapter one body for foreign rev 2 exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        2,
    );
    let foreign_rev5 = sample_book_creation(
        "cr_foreign_mono",
        9999,
        "Foreign Fresh Rev 5",
        "Foreign fresh rev 5 summary exceeding ten characters cleanly.",
        "Chapter one body for foreign rev 5 exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Published,
        5,
    );
    mock_state
        .detail_overrides
        .lock()
        .unwrap()
        .insert("cr_foreign_mono".to_string(), foreign_rev2);
    let d1 = mock_state.detail_requests_started.load(Ordering::SeqCst);
    let dr1 = mock_state.detail_responses_served.load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.selected_creation_id = Some("cr_foreign_mono".to_string());
            h.selected_detail_creation = None;
        });
        s.fetch_creation_detail_fenced("cr_foreign_mono".to_string(), cx);
    });
    wait_until(&mut app, 60, "phase 2B delayed foreign detail started", |_| {
        mock_state.detail_requests_started.load(Ordering::SeqCst) > d1
    })
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.selected_detail_creation = Some(foreign_rev5.clone());
            hcx.notify();
        });
    });
    wait_until(&mut app, 60, "phase 2B stale foreign detail served", |_| {
        mock_state.detail_responses_served.load(Ordering::SeqCst) > dr1
    })
    .await;
    drain_async_cycles(&mut app).await;
    let (det_rev_2b, det_st_2b, in_own_2b) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            let det = h.selected_detail_creation.as_ref().expect("foreign detail present");
            (
                det.cloud_revision,
                det.review_status,
                h.find_my_creation("cr_foreign_mono").is_some(),
            )
        })
        .unwrap();
    assert_eq!(det_rev_2b, 5);
    assert_eq!(det_st_2b, CreationReviewStatus::Published);
    assert!(!in_own_2b);

    // Phase 2C (R1-I4-2): (i) Non-owned deleted detail clears selection; (ii) fresh owned detail populates both my_creations & detail; (iii) late pre-delete owned detail cannot resurrect deleted creation.
    mock_state.detail_delay_ms.store(0, Ordering::SeqCst);
    let foreign_live_rev3 = sample_book_creation(
        "cr_foreign_del",
        9999,
        "Foreign Live Rev 3",
        "Foreign live rev 3 summary exceeding ten characters cleanly.",
        "Chapter one body for foreign live rev 3 exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Published,
        3,
    );
    let mut foreign_deleted_rev4 = foreign_live_rev3.clone();
    foreign_deleted_rev4.cloud_revision = 4;
    foreign_deleted_rev4.deleted_at_ms = Some(1_700_000_099_000);
    mock_state
        .detail_overrides
        .lock()
        .unwrap()
        .insert("cr_foreign_del".to_string(), foreign_deleted_rev4);
    let dr2 = mock_state.detail_responses_served.load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.selected_creation_id = Some("cr_foreign_del".to_string());
            h.selected_detail_creation = Some(foreign_live_rev3.clone());
        });
        s.fetch_creation_detail_fenced("cr_foreign_del".to_string(), cx);
    });
    wait_until(&mut app, 60, "phase 2C(i) non-owned deleted detail clears selection", |a| {
        mock_state.detail_responses_served.load(Ordering::SeqCst) > dr2
            && shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.selected_detail_creation.is_none() && h.selected_creation_id.is_none()
                })
                .unwrap_or(false)
    })
    .await;
    assert!(
        shell
            .read_with(&app, |s, cx| {
                s.content_hub.read(cx).find_my_creation("cr_foreign_del").is_none()
            })
            .unwrap()
    );

    // 2C(ii): Fresh owned detail not yet in my_creations populates BOTH my_creations and selected_detail_creation.
    let fresh_own = sample_book_creation(
        "cr_fresh_own",
        1001,
        "Fresh Owned Book",
        "Fresh owned book fetched via detail before list sync.",
        "Chapter one body for fresh owned book exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    mock_state
        .detail_overrides
        .lock()
        .unwrap()
        .insert("cr_fresh_own".to_string(), fresh_own);
    let dr3 = mock_state.detail_responses_served.load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.selected_creation_id = Some("cr_fresh_own".to_string());
            h.selected_detail_creation = None;
        });
        s.fetch_creation_detail_fenced("cr_fresh_own".to_string(), cx);
    });
    wait_until(&mut app, 60, "phase 2C(ii) fresh owned detail populates selection", |a| {
        mock_state.detail_responses_served.load(Ordering::SeqCst) > dr3
            && shell
                .read_with(a, |s, cx| {
                    let h = s.content_hub.read(cx);
                    h.selected_detail_creation
                        .as_ref()
                        .is_some_and(|d| d.creation_id == "cr_fresh_own")
                        && h.find_my_creation("cr_fresh_own").is_some()
                })
                .unwrap_or(false)
    })
    .await;

    // 2C(iii): Late pre-delete owned detail (rev 2 alive) cannot resurrect cr_own_del (deleted at rev 3 in Phase 1A).
    mock_state
        .detail_overrides
        .lock()
        .unwrap()
        .insert("cr_own_del".to_string(), own_del.clone());
    let dr4 = mock_state.detail_responses_served.load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.selected_creation_id = Some("cr_own_del".to_string());
            h.selected_detail_creation = None;
        });
        s.fetch_creation_detail_fenced("cr_own_del".to_string(), cx);
    });
    wait_until(&mut app, 60, "phase 2C(iii) late pre-delete detail served", |_| {
        mock_state.detail_responses_served.load(Ordering::SeqCst) > dr4
    })
    .await;
    drain_async_cycles(&mut app).await;
    let (resurrected_list, resurrected_detail) = shell
        .read_with(&app, |s, cx| {
            let h = s.content_hub.read(cx);
            (
                h.find_my_creation("cr_own_del").is_some(),
                h.selected_detail_creation.is_some(),
            )
        })
        .unwrap();
    assert!(!resurrected_list, "late pre-delete owned detail must NOT resurrect deleted creation in my_creations");
    assert!(!resurrected_detail, "late pre-delete owned detail must NOT populate selected_detail_creation");

    // Phase 2D: Lagging probe_reviewer_queue returning stale rev 4 cr_ws_lag after WS Draft rev 6 cannot resurrect it.
    let ws_lag_item = sample_book_creation(
        "cr_ws_lag",
        2002,
        "Lagging Queue Item",
        "Foreign pending item testing resolved_review_revisions guard.",
        "Chapter one body for lagging queue item exceeding fifty non-whitespace characters comfortably.",
        CreationReviewStatus::PendingReview,
        4,
    );
    mock_state
        .review_queue
        .lock()
        .unwrap()
        .push(ws_lag_item.clone());
    let rq_served_0 = mock_state
        .review_queue_responses_served
        .load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.review_queue.push(ws_lag_item.clone());
        });
        s.handle_ws_user_creation_changed(
            "cr_ws_lag".to_string(),
            2002,
            CreationKind::Book,
            CreationReviewStatus::Draft,
            6,
            cx,
        );
    });
    wait_until(&mut app, 60, "phase 2D lagging probe_reviewer_queue served", |_| {
        mock_state
            .review_queue_responses_served
            .load(Ordering::SeqCst)
            > rq_served_0
    })
    .await;
    drain_async_cycles(&mut app).await;
    let ws_lag_still_pruned = shell
        .read_with(&app, |s, cx| {
            !s.content_hub
                .read(cx)
                .publish_modal
                .review_queue
                .iter()
                .any(|q| q.creation_id == "cr_ws_lag")
        })
        .unwrap();
    assert!(
        ws_lag_still_pruned,
        "lagging GET /review-queue returning rev 4 must NOT resurrect cr_ws_lag resolved at WS rev 6"
    );
}

#[test]
fn test_m5_gpui_iter6_reg_inspector_media_compose_hub_and_observer_ime() {
    run_in_headless_subprocess(
        "test_m5_gpui_iter6_reg_inspector_media_compose_hub_and_observer_ime",
        true,
    );
}

fn body_iter6_reg_inspector_media_compose_hub_and_observer_ime(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();
    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));

    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
        s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
        s.preferences.local_passcode_hash =
            Some(SettingsLocalPasscodeSections::hash_passcode_hex("1234"));
    });

    // === 1. C2-I5-1 (+ Siblings #16-#18): InspectorAction::OpenMedia & Dual-Path Observer Guard ===
    shell.update(cx, |s, cx| {
        let msgs_before = s.stage.read(cx).messages.len();
        s.window_runtime.controller.clear_all_toasts();
        cx.write_to_clipboard(ClipboardItem::new_string("sentinel_clip".to_string()));
        let clip_before = cx.read_from_clipboard().and_then(|c| c.text());
        let bad = "\u{00AD}tg://apple/books/<script>alert(1)</script>";

        // 1a. Direct InspectorAction::OpenMedia with smuggled Apple XSS -> zero leak.
        s.handle_inspector_action(
            InspectorAction::OpenMedia {
                url: bad.to_string(),
                is_video: false,
            },
            cx,
        );
        assert!(
            s.overlays.media_viewer_overlay.is_none(),
            "C2-I5-1: OpenMedia must block Apple XSS"
        );
        assert!(
            s.window_runtime.controller.current_toast.is_none(),
            "C2-I5-1: zero toast leak"
        );
        assert_eq!(
            s.stage.read(cx).messages.len(),
            msgs_before,
            "C2-I5-1: zero outgoing msg leak"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()),
            clip_before,
            "C2-I5-1: zero clipboard leak"
        );

        // 1b. Siblings #16 (DownloadDoc), #17 (ContentHubEvent::LaunchMiniApp & confirm custom_url),
        //     and #18 (IvViewer::pending_share_url) -> zero leak.
        s.stage.update(cx, |st, _| {
            st.pending_download_url = Some((bad.to_string(), "evil.pdf".to_string()));
        });
        s.handle_stage_pending_actions(cx);
        s.handle_content_hub_event(
            &ContentHubEvent::LaunchMiniApp {
                bot_username: "demo_bot".to_string(),
                short_name: "demo_app".to_string(),
                web_url: Some(bad.to_string()),
            },
            cx,
        );
        s.request_launch_miniapp_with_confirm_and_url(
            "demo_bot",
            "demo_app",
            Some(5001),
            MiniAppLaunchMode::DirectLink,
            None,
            Some(bad.to_string()),
            cx,
        );
        assert!(
            s.miniapps.miniapp_overlay.is_none(),
            "Sibling #17: smuggled web_url/custom_url must not mount MiniApp"
        );
        assert!(
            s.miniapps.pending_miniapp_confirm.is_none(),
            "Sibling #17: smuggled custom_url must not open MiniApp ToS confirm modal"
        );
        assert!(s.window_runtime.controller.current_toast.is_none());

        // Sibling #18: open normal InstantView, inject smuggled pending_share_url -> zero outgoing msg leak.
        s.stage.update(cx, |st, _| {
            st.pending_iv_url = Some((
                "https://example.com/article".to_string(),
                "Article".to_string(),
            ));
        });
        s.handle_stage_pending_actions(cx);
        let iv = s
            .overlays
            .iv_viewer_overlay
            .clone()
            .expect("normal InstantView URL must mount iv_viewer_overlay");
        iv.update(cx, |v, vcx| {
            v.pending_share_url = Some(bad.to_string());
            vcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.stage.read(cx).messages.len(),
            0,
            "Sibling #18: IvViewer::pending_share_url must block smuggled Apple XSS"
        );
        assert!(s.window_runtime.controller.current_toast.is_none());
        if let Some(iv) = s.overlays.iv_viewer_overlay.clone() {
            iv.update(cx, |v, vcx| {
                v.should_close = true;
                vcx.notify();
            });
        }
    });
    shell.update(cx, |s, _cx| {
        assert!(s.overlays.iv_viewer_overlay.is_none());
    });

    // 1c. Real dual-path inspector observer click on valid tg://apple/books/cr_insp -> routes ONCE, 0 MediaViewer.
    let books_gen_before = shell.read(cx).content_hub_state.books_catalog_gen;
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.inspector.update(cx, |i, icx| {
            i.pending_media_open = Some(("tg://apple/books/cr_insp".to_string(), false));
            i.pending_action = Some(InspectorAction::OpenMedia {
                url: "tg://apple/books/cr_insp".to_string(),
                is_video: false,
            });
            icx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_insp")
        );
        assert!(
            s.overlays.media_viewer_overlay.is_none(),
            "C2-I5-1: dual-path inspector click must NOT mount stray MediaViewer over Books hub"
        );
        assert_eq!(
            s.content_hub_state.books_catalog_gen,
            books_gen_before + 1,
            "C2-I5-1: dual-path inspector observer must dispatch deep-link ONCE, not twice"
        );

        // 1d. Normal media URL via InspectorAction::OpenMedia STILL opens themed, close-observed MediaViewer.
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.handle_inspector_action(
            InspectorAction::OpenMedia {
                url: "https://cdn.example.com/photo.jpg".to_string(),
                is_video: false,
            },
            cx,
        );
        let mv = s
            .overlays
            .media_viewer_overlay
            .clone()
            .expect("normal HTTPS media URL must still open MediaViewer");
        assert_eq!(
            mv.read(cx).palette,
            s.theme_state.palette,
            "MediaViewer must inherit shell palette"
        );
        assert_eq!(
            mv.read(cx).lang_id,
            s.active_lang_id(),
            "MediaViewer must inherit shell locale"
        );
        assert_eq!(
            mv.read(cx).current_item().map(|i| i.url.as_str()),
            Some("https://cdn.example.com/photo.jpg")
        );
        mv.update(cx, |m, mcx| {
            m.should_close = true;
            mcx.notify();
        });
    });
    shell.update(cx, |s, _cx| {
        assert!(
            s.overlays.media_viewer_overlay.is_none(),
            "MediaViewer opened via InspectorAction::OpenMedia must close via attached close observer"
        );
    });

    // === 2. C1-I5-1: Chats (AppleCompose + QuickDrawerSearch) -> Books via Sidebar & Deep-Link ===
    for via_deep_link in [false, true] {
        shell.update(cx, |s, cx| {
            s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
            s.sidebar.update(cx, |sb, scx| {
                sb.exit_compose_mode();
                sb.toggle_plus_menu();
                scx.notify();
            });
        });
        shell.update(cx, |s, cx| {
            assert!(
                s.composer.read(cx).is_compose_isolated,
                "Apple Compose must enter isolation"
            );
            s.toggle_quick_creations_drawer(true, cx);
            s.content_hub.update(cx, |h, hcx| {
                h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
            });
        });
        shell.update(cx, |s, cx| {
            assert_eq!(
                s.content_hub.read(cx).active_input,
                ContentHubInputTarget::QuickDrawerSearch
            );
            if via_deep_link {
                assert!(s.handle_apple_deep_link("tg://apple/books", cx));
            } else {
                s.select_apple_sidebar_section(AppleSidebarSection::Books, cx);
            }
        });
        shell.update(cx, |s, cx| {
            assert_eq!(
                s.active_panel,
                ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
            );
            assert!(
                !s.composer.read(cx).is_compose_isolated,
                "C1-I5-1 (deep_link={via_deep_link}): compose isolation must exit synchronously on Books hub"
            );
            assert!(
                !s.composer.read(cx).is_input_focused,
                "C1-I5-1 (deep_link={via_deep_link}): hidden Composer must stay unfocused on Books hub"
            );
            assert_eq!(
                s.content_hub.read(cx).active_input,
                ContentHubInputTarget::QuickDrawerSearch,
                "C1-I5-1 (deep_link={via_deep_link}): QuickDrawerSearch focus must survive Chats(AppleCompose)->Books"
            );
            assert!(s.is_content_hub_input_active(cx));
        });
    }

    // Sibling S3 check: Escape on unfocused ContentHub(Books) must NOT focus hidden Composer.
    shell.update(cx, |s, cx| {
        s.toggle_quick_creations_drawer(false, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.blur_input();
            hcx.notify();
        });
        assert!(!s.handle_non_char_key("escape", false, cx));
        assert!(!s.composer.read(cx).is_input_focused);
    });

    // === 3. C1-I5-2: Composer Click Clears Sidebar/BotCard/StageSearch IME + Syncs Falling Edges + All-17-Owner IME Guard ===
    // 3a. ADV-2a + ADV-2c: ChatSidebar search -> Composer click -> no "sb_ghost" commit -> no phantom falling edge on stage.search_open.
    shell.update(cx, |s, cx| {
        s.toggle_quick_creations_drawer(false, cx);
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.composer.update(cx, |c, ccx| {
            c.clear();
            c.is_input_focused = false;
            ccx.notify();
        });
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = true;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("sb_ghost", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("sb_ghost"));
        s.composer.update(cx, |c, ccx| {
            c.is_input_focused = true;
            ccx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(!s.sidebar.read(cx).search_focused);
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I5-2 (ADV-2a): Composer click must clear sidebar search IME"
        );
        s.commit_ime_unmark_text(cx);
        assert!(
            s.composer.read(cx).text().is_empty(),
            "C1-I5-2 (ADV-2a): zero 'sb_ghost' commit into Composer"
        );
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("stage_live", cx);
        // Unrelated incoming unread-badge notify on sidebar must NOT trigger phantom falling edge:
        s.sidebar.update(cx, |_, scx| scx.notify());
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("stage_live"),
            "C1-I5-2 (ADV-2c): unrelated sidebar notify must NOT wipe active stage.search_open IME"
        );
        s.stage.update(cx, |st, stcx| {
            st.apply_topbar_action(TopBarUiAction::SearchClear);
            stcx.notify();
        });
    });

    // 3b. ADV-2b + ADV-2d: BotAssistCard -> Composer click -> clears shell.ime & bot_assist_ime_preedit -> no phantom falling edge on sidebar.search_focused.
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.active_bot_card_field = Some(("card_1".to_string(), BotAssistLoginField::Username));
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("card_ghost", cx);
        assert_eq!(
            s.stage.read(cx).bot_assist_ime_preedit.as_deref(),
            Some("card_ghost")
        );
        s.composer.update(cx, |c, ccx| {
            c.is_input_focused = true;
            ccx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.stage.read(cx).active_bot_card_field.is_none());
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I5-2 (ADV-2b): Composer click must clear BotAssistCard shell.ime"
        );
        assert!(
            s.stage.read(cx).bot_assist_ime_preedit.is_none(),
            "C1-I5-2 (ADV-2b): must clear bot_assist_ime_preedit"
        );
        s.commit_ime_unmark_text(cx);
        assert!(s.composer.read(cx).text().is_empty());
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = true;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("sb_live", cx);
        // Unrelated incoming stage message notify must NOT trigger phantom falling edge:
        s.stage.update(cx, |_, stcx| stcx.notify());
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("sb_live"),
            "C1-I5-2 (ADV-2d): unrelated stage notify must NOT wipe active sidebar.search_focused IME"
        );
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = false;
            scx.notify();
        });
    });

    // 3c. Clean mutual blur + real true->false falling-edge protection (ADV-4 + all non-composer owners)
    //     + direct has_active_non_composer_ime_owner assertions.
    // (i) Passcode Lock (ADV-4) over flushed stage.search_open -> real true->false falling edge preserves Passcode IME.
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.lock_app_now(cx));
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.update_ime_marked_text("pass_pre", cx);
        s.stage.update(cx, |st, stcx| {
            st.apply_topbar_action(TopBarUiAction::SearchClear);
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("pass_pre"),
            "C1-I5-2 (ADV-4): stage falling edge must NOT wipe Passcode IME"
        );
        assert!(s.try_unlock_with_passcode("1234", cx));
    });

    // (ii) Clean mutual blur on AppleCompose opening + real true->false falling edge protection for AppleCompose.
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(
            !s.stage.read(cx).search_open,
            "opening Apple Compose must cleanly mutually blur stage.search_open"
        );
        assert!(s.is_apple_compose_input_active(cx));
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.update_ime_marked_text("ac_owned", cx);
        s.stage.update(cx, |_, stcx| stcx.notify());
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("ac_owned"),
            "C1-I5-2: unrelated stage notify must NOT wipe Apple Compose IME"
        );
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });
    // Real true->false stage falling edge while AppleCompose owns IME:
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, _| {
            sb.is_plus_menu_open = true;
            sb.search_focused = false;
        });
        assert!(s.is_apple_compose_input_active(cx));
        s.update_ime_marked_text("ac_edge", cx);
        s.stage.update(cx, |st, stcx| {
            st.apply_topbar_action(TopBarUiAction::SearchClear);
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("ac_edge"),
            "C1-I5-2: real stage true->false falling edge must NOT wipe Apple Compose IME"
        );
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });

    // (iii) Clean mutual blur + real true->false falling edge between sidebar.search_focused & stage.search_open,
    //       plus ModalContainer / ContactsModal / CreationModal / PublishReviewModal / QuickDrawerSearch.
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, _| {
            sb.search_focused = true;
        });
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.update_ime_marked_text("sb_edge", cx);
        s.stage.update(cx, |st, stcx| {
            st.apply_topbar_action(TopBarUiAction::SearchClear);
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("sb_edge"),
            "C1-I5-2: real stage falling edge must NOT wipe sidebar.search_focused IME"
        );
        // Clean mutual blur: opening stage.search_open blurs sidebar.search_focused with notify.
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(
            !s.sidebar.read(cx).search_focused,
            "opening stage.search_open must cleanly mutually blur sidebar.search_focused"
        );
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.update_ime_marked_text("st_owned", cx);
        s.sidebar.update(cx, |_, scx| scx.notify());
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("st_owned"),
            "C1-I5-2: unrelated sidebar notify must NOT wipe stage.search_open IME"
        );
        // Real sidebar falling edge while stage.search_open owns IME:
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = true;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, _| {
            st.focus_search();
        });
        s.update_ime_marked_text("st_edge", cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = false;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("st_edge"),
            "C1-I5-2: real sidebar falling edge must NOT wipe stage.search_open IME"
        );
        s.stage.update(cx, |st, stcx| {
            st.apply_topbar_action(TopBarUiAction::SearchClear);
            stcx.notify();
        });
    });

    // Direct owner matrix checks: ModalContainer, ContactsModal, CreationModal, PublishReviewModal, QuickDrawerSearch,
    // plus negative checks on unfocused Chats & unfocused ContentHub(Books).
    shell.update(cx, |s, cx| {
        assert!(!s.has_active_non_composer_ime_owner(cx));

        s.open_modal_dialog("Info", ModalDialogKind::AboutTelegram, 380.0, 260.0, cx);
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.modal_container.update(cx, |m, mcx| {
            m.hide_all();
            mcx.notify();
        });

        s.sidebar.update(cx, |sb, scx| {
            sb.open_contacts_modal();
            scx.notify();
        });
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.sidebar.update(cx, |sb, scx| {
            sb.close_contacts_modal();
            scx.notify();
        });

        s.sidebar.update(cx, |sb, scx| {
            sb.open_create_group_modal();
            scx.notify();
        });
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.sidebar.update(cx, |sb, scx| {
            sb.close_creation_modal();
            scx.notify();
        });

        s.open_reviewer_queue_modal(cx);
        assert!(s.has_active_non_composer_ime_owner(cx));
        s.close_publish_review_modal(cx);

        s.select_apple_sidebar_section(AppleSidebarSection::Books, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.blur_input();
            hcx.notify();
        });
        assert!(
            !s.has_active_non_composer_ime_owner(cx),
            "unfocused ContentHub(Books) must NOT claim IME ownership"
        );
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
    });

    // (iv) CloseQuickDrawer unfocused vs focused:
    //      Unfocused QuickDrawer close preserves active IME in sidebar.search_focused & ModalContainer,
    //      while focused QuickDrawerSearch close (even over AppleCompose) clears IME.
    shell.update(cx, |s, cx| {
        s.toggle_quick_creations_drawer(true, cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = true;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("sb_keep", cx);
        s.handle_content_hub_event(&ContentHubEvent::CloseQuickDrawer, cx);
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("sb_keep"),
            "CloseQuickDrawer on unfocused drawer must preserve sidebar.search_focused IME"
        );
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = false;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.toggle_quick_creations_drawer(true, cx);
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("mc_keep", cx);
        s.handle_content_hub_event(&ContentHubEvent::CloseQuickDrawer, cx);
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("mc_keep"),
            "CloseQuickDrawer on unfocused drawer must preserve ModalContainer IME"
        );
        s.modal_container.update(cx, |m, mcx| {
            m.hide_all();
            mcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("qd_clear", cx);
        s.handle_content_hub_event(&ContentHubEvent::CloseQuickDrawer, cx);
        assert!(
            s.ime.ime_preedit.is_none(),
            "CloseQuickDrawer when QuickDrawerSearch owned IME must clear shell.ime even over AppleCompose"
        );
        assert!(
            s.content_hub
                .read(cx)
                .quick_drawer_search
                .ime_preedit
                .is_none()
        );
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_compose_mode();
            scx.notify();
        });
    });

    // (v) Hidden stage/inspector on ContentHub(Books) must NOT blur QuickDrawerSearch or wipe its IME.
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Books, cx);
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("hub_qd_live", cx);
        s.stage.update(cx, |st, stcx| {
            st.active_bot_card_field = Some(("bg_card".to_string(), BotAssistLoginField::Username));
            stcx.notify();
        });
        s.inspector.update(cx, |ins, icx| {
            ins.open();
            ins.profile.is_bot = false;
            ins.open_add_member_picker(Vec::new());
            icx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::QuickDrawerSearch,
            "hidden stage/inspector notifications on ContentHub(Books) must NOT blur QuickDrawerSearch"
        );
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("hub_qd_live"),
            "hidden stage/inspector notifications on ContentHub(Books) must NOT wipe QuickDrawerSearch IME"
        );
        // Cleanup hidden flags and return to Chats.
        s.stage.update(cx, |st, stcx| {
            st.blur_bot_card_input();
            stcx.notify();
        });
        s.inspector.update(cx, |ins, icx| {
            ins.close_add_member_picker();
            ins.is_open = false;
            icx.notify();
        });
        s.toggle_quick_creations_drawer(false, cx);
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
    });

    // (vi) R2-I4-P1: TopBarUiAction::SearchClear STILL clears IME when no other input owns IME
    //      for BOTH show_bot_start_bar = false AND true.
    for bot_start in [false, true] {
        shell.update(cx, |s, cx| {
            s.stage.update(cx, |st, stcx| {
                st.show_bot_start_bar = bot_start;
                st.focus_search();
                stcx.notify();
            });
        });
        shell.update(cx, |s, cx| {
            s.update_ime_marked_text("clr_me", cx);
            s.stage.update(cx, |st, stcx| {
                st.apply_topbar_action(TopBarUiAction::SearchClear);
                stcx.notify();
            });
        });
        shell.update(cx, |s, cx| {
            assert!(
                s.ime.ime_preedit.is_none(),
                "C1-I5-2/R2-I4-P1 (bot_start={bot_start}): SearchClear must still clear IME when no other input owns IME"
            );
            assert!(
                !s.has_active_non_composer_ime_owner(cx),
                "R2-I4-P1 (bot_start={bot_start}): pre-start bot chat must NOT claim non-composer IME ownership"
            );
        });
    }
}

#[test]
fn test_m5_gpui_iter7_reg_zorder_observers_passcode_unicode_and_ws() {
    run_in_headless_subprocess(
        "test_m5_gpui_iter7_reg_zorder_observers_passcode_unicode_and_ws",
        true,
    );
}

fn body_iter7_reg_zorder_observers_passcode_unicode_and_ws(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();
    let shell: Entity<AppShell> = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));

    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
        s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
        s.preferences.local_passcode_hash =
            Some(SettingsLocalPasscodeSections::hash_passcode_hex("1234"));
    });

    // === 1. R1-I6-1 / R2-I6-1 + R2-I6-3: Rising-Edge observe_composer Same-Frame Race, MediaViewer Sync & Composer IME Cancel ===
    // (a) Same-frame Composer IME notify + click QuickDrawerSearch -> QuickDrawerSearch keeps focus, Composer blurred.
    shell.update(cx, |s, cx| {
        s.toggle_quick_creations_drawer(true, cx);
        s.composer.update(cx, |c, ccx| {
            c.is_input_focused = true;
            ccx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("comp_pre", cx); // enqueues Notify(composer)
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::QuickDrawerSearch
        );
        assert!(
            !s.composer.read(cx).is_input_focused,
            "R1-I6-1: QuickDrawerSearch must blur Composer"
        );
        assert!(
            !s.composer.read(cx).ime_machine.is_composing(),
            "R2-I6-3: composer IME must cancel"
        );
        s.toggle_quick_creations_drawer(false, cx);
    });

    // (b) Same-frame Composer notify + SidebarSearch / StageSearch / BotAssistCard / Inspector + R2-I6-3 IME cancel.
    for target in ["sb", "stage", "card", "insp"] {
        shell.update(cx, |s, cx| {
            s.composer.update(cx, |c, ccx| {
                c.is_input_focused = true;
                ccx.notify();
            });
        });
        shell.update(cx, |s, cx| {
            s.update_ime_marked_text("comp_live", cx);
            match target {
                "sb" => s.sidebar.update(cx, |sb, scx| {
                    sb.search_focused = true;
                    scx.notify();
                }),
                "stage" => s.stage.update(cx, |st, stcx| {
                    st.focus_search();
                    stcx.notify();
                }),
                "card" => s.stage.update(cx, |st, stcx| {
                    st.active_bot_card_field =
                        Some(("c_sf".to_string(), BotAssistLoginField::Username));
                    stcx.notify();
                }),
                _ => s.inspector.update(cx, |ins, icx| {
                    ins.open();
                    ins.profile.is_bot = false;
                    ins.open_add_member_picker(Vec::new());
                    icx.notify();
                }),
            }
        });
        shell.update(cx, |s, cx| {
            assert!(
                !s.composer.read(cx).is_input_focused,
                "R1-I6-1 ({target}): Composer must NOT steal focus"
            );
            assert!(
                !s.composer.read(cx).ime_machine.is_composing(),
                "R2-I6-3 ({target}): Composer ime_machine must cancel"
            );
            match target {
                "sb" => {
                    assert!(s.sidebar.read(cx).search_focused);
                    s.sidebar.update(cx, |sb, scx| {
                        sb.search_focused = false;
                        scx.notify();
                    });
                }
                "stage" => {
                    assert!(s.stage.read(cx).search_open);
                    s.stage.update(cx, |st, stcx| {
                        st.apply_topbar_action(TopBarUiAction::SearchClear);
                        stcx.notify();
                    });
                }
                "card" => {
                    assert!(s.stage.read(cx).active_bot_card_field.is_some());
                    s.stage.update(cx, |st, stcx| {
                        st.blur_bot_card_input();
                        stcx.notify();
                    });
                }
                _ => {
                    assert!(s.inspector.read(cx).is_bot_input_active());
                    s.inspector.update(cx, |ins, icx| {
                        ins.close_add_member_picker();
                        ins.is_open = false;
                        icx.notify();
                    });
                }
            }
        });
    }

    // (c) Exp2 Extra: MediaViewer open/close keeps prev_composer_focused synced so subsequent Composer click fires rising edge and blurs SidebarSearch.
    shell.update(cx, |s, cx| {
        s.composer.update(cx, |c, ccx| {
            c.is_input_focused = true;
            ccx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.show_media_viewer("https://cdn.example.com/sync_edge.jpg", false, cx);
        s.sync_compose_state_to_composer(false, cx);
    });
    shell.update(cx, |s, cx| {
        assert!(!s.composer.read(cx).is_input_focused);
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.overlays.media_viewer_overlay.is_none());
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = true;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.sidebar.read(cx).search_focused);
        s.composer.update(cx, |c, ccx| {
            c.is_input_focused = true;
            ccx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(
            !s.sidebar.read(cx).search_focused,
            "R2-I6-1: MediaViewer close -> Composer click must still fire rising edge and blur SidebarSearch"
        );
    });

    // === 2. R1-I6-3 / R2-I6-2 + C1-I6-7: ContactsModal & AccountPopover Over CatalogSearch, CreateBotBox/IvViewer Close IME, StageSearch<->BotCard Symmetry, Occluded Chats Observers & Forbidden Queue ===
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Books, cx);
        s.content_hub
            .update(cx, |h, hcx| h.focus_input(ContentHubInputTarget::CatalogSearch, hcx));
    });
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.open_contacts_modal();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::None,
            "R1-I6-3/R2-I6-2: ContactsModal must blur CatalogSearch"
        );
        s.update_ime_marked_text("contact_pre", cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.close_contacts_modal();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "R1-I6-3/R2-I6-2: closing ContactsModal must clear IME with zero leak"
        );
        // Exp2 Extra: AccountPopover open/close over focused CatalogSearch blurs CatalogSearch & leaks zero IME.
        s.content_hub
            .update(cx, |h, hcx| h.focus_input(ContentHubInputTarget::CatalogSearch, hcx));
    });
    shell.update(cx, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.is_account_popover_open = true;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::None,
            "R1-I6-3/Exp2: AccountPopover over CatalogSearch must blur CatalogSearch"
        );
        s.update_ime_marked_text("pop_ghost", cx);
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I6-3: AccountPopover must NOT store phantom IME"
        );
        s.sidebar.update(cx, |sb, scx| {
            sb.is_account_popover_open = false;
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.ime.ime_preedit.is_none());
        // Exp2 Extra: CreateManagedBotBox close clears IME cleanly with zero leak into CatalogSearch.
        s.content_hub
            .update(cx, |h, hcx| h.focus_input(ContentHubInputTarget::CatalogSearch, hcx));
        s.open_nl_creator_modal(cx);
        s.update_ime_marked_text("bot_box_pre", cx);
        let box_ent = s.miniapps.create_bot_box.clone().unwrap();
        box_ent.update(cx, |b, bcx| {
            b.pending_close = true;
            bcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.miniapps.create_bot_box.is_none());
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I6-3: CreateManagedBotBox close must clear IME"
        );
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
    });

    // C1-I6-7a: BotAssistCard opening blurs StageSearch symmetrically (no IME leak on card blur).
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.active_bot_card_field =
                Some(("c_sym".to_string(), BotAssistLoginField::Username));
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(
            !s.stage.read(cx).search_open,
            "C1-I6-7: BotAssistCard must blur StageSearch"
        );
        s.update_ime_marked_text("card_sym_pre", cx);
        s.stage.update(cx, |st, stcx| {
            st.blur_bot_card_input();
            stcx.notify();
        });
    });
    shell.update(cx, |s, _cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I6-7: blurring BotAssistCard must clear IME"
        );
    });

    // C1-I6-7b/c: Occluded Chats observer notify never wipes PublishReviewModal IME; Forbidden open_reviewer_queue stays on Submit tab.
    let clean_book = sample_book_creation(
        "cr_iter7_1",
        1001,
        "Iter7 Atlas",
        "A valid summary longer than ten characters.",
        "Chapter one body has plenty of readable characters exceeding fifty non-whitespace characters.",
        CreationReviewStatus::Draft,
        1,
    );
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.reviewer_access = ReviewerAccessState::Forbidden;
        });
        s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
        s.open_reviewer_queue_modal(cx);
        assert!(
            !s.content_hub.read(cx).publish_modal.show_reviewer_queue_tab,
            "C1-I6-7: Forbidden must stay on Submit tab"
        );
        assert_eq!(
            s.content_hub.read(cx).publish_modal.focused_field,
            Some(PublishModalField::Title)
        );
        assert!(s.handle_non_char_key("tab", false, cx));
        assert_eq!(
            s.content_hub.read(cx).publish_modal.focused_field,
            Some(PublishModalField::Subtitle)
        );
    });
    // Flush FocusInputTaken emitted by open_reviewer_queue_modal, then verify occluded Chats stage notify preserves PublishReviewModal IME.
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("pub_protected", cx);
        s.stage.update(cx, |st, stcx| {
            st.focus_search();
            stcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("pub_protected"),
            "C1-I6-7: occluded Chats stage notify must NOT wipe PublishReviewModal IME"
        );
        s.stage.update(cx, |st, stcx| {
            st.search_open = false;
            stcx.notify();
        });
        s.close_publish_review_modal(cx);
    });

    // === 3. C1-I6-1 + C1-I6-2 + R1-I6-2 + C1-I6-3: ModalContainer Key Absorption, Fullscreen Occlusion & Zero Phantom IME ===
    shell.update(cx, |s, cx| {
        let msgs_before = s.stage.read(cx).messages.len();
        s.composer.update(cx, |c, ccx| {
            c.set_text("SECRET_UNSENT_DRAFT".to_string());
            c.is_input_focused = true;
            ccx.notify();
        });
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
        assert!(s.handle_non_char_key("enter", false, cx));
        assert_eq!(
            s.stage.read(cx).messages.len(),
            msgs_before,
            "C1-I6-1: Enter in ModalContainer must NOT send chat draft"
        );
        assert_eq!(s.composer.read(cx).text(), "SECRET_UNSENT_DRAFT");
        assert!(s.handle_non_char_key("tab", false, cx));
        assert!(
            !s.sidebar.read(cx).search_focused,
            "C1-I6-1: Tab in ModalContainer must NOT focus Sidebar search"
        );

        // C1-I6-2: ComputerWorkbench (non-terminal tab) above ModalContainer & PublishReviewModal absorbs all text/IME/keys.
        s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
        s.show_remote_computer(cx);
        s.insert_committed_text("_CW_LEAK", cx);
        s.update_ime_marked_text("cw_ghost", cx);
        assert_eq!(
            s.content_hub.read(cx).publish_modal.title.text,
            "Iter7 Atlas",
            "C1-I6-2: non-terminal CW must block PublishReviewModal typing"
        );
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I6-3: non-terminal CW must NOT store phantom ime_preedit"
        );
        assert!(
            s.modal_container.read(cx).dialog.primary_input.is_empty(),
            "C1-I6-2: CW must block ModalContainer typing"
        );

        // C1-I6-4: Z-order Escape dismisses ComputerWorkbench (#15) -> PublishReviewModal (#14) -> ModalContainer (#13) -> MediaViewer (#3).
        s.show_media_viewer("https://cdn.example.com/zorder.jpg", false, cx);
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            s.overlays.computer_overlay.is_none(),
            "C1-I6-4: Escape #1 must close top-Z ComputerWorkbench"
        );
        assert!(
            s.is_publish_review_modal_active(cx),
            "C1-I6-4: PublishReviewModal must stay open until Escape #2"
        );
        assert!(
            s.overlays.media_viewer_overlay.is_some(),
            "C1-I6-4: lower-Z MediaViewer must NOT close underneath CW"
        );
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.is_publish_review_modal_active(cx),
            "C1-I6-4: Escape #2 must close PublishReviewModal"
        );
        assert!(s.modal_container.read(cx).is_modal_active());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.modal_container.read(cx).is_modal_active(),
            "C1-I6-4: Escape #3 must close ModalContainer"
        );
        assert!(s.overlays.media_viewer_overlay.is_some());

        // R1-I6-2 + C1-I6-2 + C1-I6-3: MediaViewer blocks QuickDrawerSearch, InspectorBot, Cmd+K, AND stores zero phantom IME.
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx)
        });
        s.inspector.update(cx, |ins, icx| {
            ins.open();
            ins.profile.is_bot = false;
            ins.open_add_member_picker(Vec::new());
            icx.notify();
        });
        assert!(
            !s.is_content_hub_input_active(cx),
            "R1-I6-2: MediaViewer must occlude QuickDrawerSearch"
        );
        s.insert_committed_text("mv_leak", cx);
        s.update_ime_marked_text("mv_ghost_ime", cx);
        assert!(
            s.content_hub.read(cx).quick_drawer_search.text.is_empty(),
            "R1-I6-2: MediaViewer must block QuickDrawerSearch typing"
        );
        assert!(
            s.inspector.read(cx).add_member_query.is_empty(),
            "C1-I6-2: MediaViewer must block Inspector typing"
        );
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I6-3: MediaViewer must NOT write phantom self.ime.ime_preedit"
        );
        assert_eq!(s.active_text_and_preedit(cx), (String::new(), None));
        assert!(s.handle_shortcut_key("k", false, cx));
        assert!(
            !s.sidebar.read(cx).search_focused,
            "R1-I6-2: Cmd+K behind MediaViewer must be blocked"
        );

        // C1-I6-4: Background Composer IME & autocomplete never eat MediaViewer Escape.
        s.composer.update(cx, |c, _| {
            c.bot_ac_visible = true;
            c.bot_ac_matches = vec![BotSlashCommandItem {
                command: "newbook".to_string(),
                description: "Create".to_string(),
                bot_username: "CreatorStudioBot".to_string(),
                avatar_initials: "CS".to_string(),
                avatar_color: 0x007AFF,
            }];
        });
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            s.overlays.media_viewer_overlay.is_none(),
            "C1-I6-4: Escape must close MediaViewer despite background autocomplete"
        );
        s.inspector.update(cx, |ins, icx| {
            ins.close_add_member_picker();
            ins.is_open = false;
            icx.notify();
        });
        s.toggle_quick_creations_drawer(false, cx);
    });

    // === 4. C1-I6-5: Global Shortcuts Guarded Behind Modals/Overlays + Cmd+E Hub Blur + Cmd+D AppleCompose Cleanup ===
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Books, cx);
        s.content_hub
            .update(cx, |h, hcx| h.focus_input(ContentHubInputTarget::CatalogSearch, hcx));
        s.open_modal_dialog("Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
        s.update_ime_marked_text("poll_ime", cx);
        assert!(s.handle_shortcut_key("k", false, cx));
        assert!(s.handle_shortcut_key("n", false, cx));
        assert!(s.handle_shortcut_key("3", false, cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books),
            "C1-I6-5: Cmd+N/3 behind ModalContainer must not switch panels"
        );
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("poll_ime"),
            "R1-I6-2/C1-I6-5: blocked shortcuts behind ModalContainer must preserve modal IME"
        );
        s.modal_container.update(cx, |m, mcx| {
            m.hide_all();
            mcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        // Cmd+E on ContentHub(Books) blurs CatalogSearch.
        s.content_hub
            .update(cx, |h, hcx| h.focus_input(ContentHubInputTarget::CatalogSearch, hcx));
        assert!(s.handle_shortcut_key("e", false, cx));
        assert!(s.sidebar.read(cx).is_account_popover_open);
        assert_eq!(
            s.content_hub.read(cx).active_input,
            ContentHubInputTarget::None,
            "C1-I6-5: Cmd+E must blur CatalogSearch"
        );
        assert!(s.handle_shortcut_key("e", false, cx)); // close AccountPopover

        // Cmd+D switching Day (Apple) -> Classic during AppleCompose clears IME and exits compose isolation.
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.toggle_plus_menu();
            scx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("ac_theme_pre", cx);
        s.apply_theme_selection(TdesktopThemeMode::Classic, None, None, cx);
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I6-5: Day->Classic theme switch must clear AppleCompose IME"
        );
        assert!(
            !s.composer.read(cx).is_compose_isolated,
            "C1-I6-5: Day->Classic must exit AppleCompose isolation"
        );
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
    });

    // === 5. C1-I6-6: Cross-Account CanonicalComposer Secret Draft, Undo Stack, Attachments, Isolation & Modal Wipe on Logout & Account Switch ===
    shell.update(cx, |s, cx| {
        s.composer.update(cx, |c, ccx| {
            c.set_text("USER_A_SECRET_DRAFT".to_string());
            c.editor.push_undo(&c.ime_machine);
            c.replying_to = Some((42, "Alice".to_string(), "Secret reply".to_string()));
            c.editor
                .attachments
                .push(std::path::PathBuf::from("/tmp/secret_alice.png"));
            c.set_send_as_options(
                vec![SendAsPeer::new(999, "Alice Channel".to_string(), false)],
                Some(999),
            );
            c.is_compose_isolated = true;
            c.is_input_focused = true;
            ccx.notify();
        });
        s.composer.update(cx, |c, ccx| {
            c.ime_machine.on_ime_update("user_a_pre", 10);
            ccx.notify();
        });
        s.open_modal_dialog("Info", ModalDialogKind::AboutTelegram, 380.0, 260.0, cx);
        s.logout(cx);
        {
            let c = s.composer.read(cx);
            assert!(c.text().is_empty(), "C1-I6-6: logout must wipe composer draft");
            assert!(
                c.editor.undo_stack.is_empty() && c.editor.redo_stack.is_empty(),
                "C1-I6-6: logout must wipe composer undo/redo stacks"
            );
            assert!(c.replying_to.is_none());
            assert!(c.editor.attachments.is_empty());
            assert!(!c.is_compose_isolated);
            assert!(c.send_as_options.is_empty());
            assert!(!c.is_input_focused);
            assert!(
                !c.ime_machine.is_composing(),
                "C1-I6-6: logout must cancel composer IME"
            );
        }
        assert!(!s.modal_container.read(cx).is_modal_active());
        s.apply_authenticated_session(sample_auth(1002, "bob"), cx);
        s.open_or_select_chat(5002, Some("Bob Chat".to_string()), false, cx);
        assert!(
            s.composer.read(cx).text().is_empty(),
            "C1-I6-6: Bob must not inherit Alice draft"
        );
        s.composer.update(cx, |c, ccx| {
            c.set_text("BOB_SECRET".to_string());
            c.editor.push_undo(&c.ime_machine);
            c.set_send_as_options(
                vec![SendAsPeer::new(888, "Bob Channel".to_string(), false)],
                Some(888),
            );
            c.is_compose_isolated = true;
            c.is_input_focused = true;
            c.ime_machine.on_ime_update("bob_pre", 7);
            ccx.notify();
        });
        s.open_modal_dialog("Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
        s.apply_authenticated_session(sample_auth(1003, "carol"), cx);
        {
            let c = s.composer.read(cx);
            assert!(
                c.text().is_empty(),
                "C1-I6-6: direct account switch must wipe composer draft"
            );
            assert!(
                c.editor.undo_stack.is_empty(),
                "C1-I6-6: direct account switch must wipe undo_stack"
            );
            assert!(!c.is_compose_isolated);
            assert!(c.send_as_options.is_empty());
            assert!(
                !c.is_input_focused,
                "C1-I6-6: direct account switch must blur composer"
            );
            assert!(
                !c.ime_machine.is_composing(),
                "C1-I6-6: direct account switch must cancel composer IME"
            );
        }
        assert!(
            !s.modal_container.read(cx).is_modal_active(),
            "C1-I6-6: direct account switch must close open ModalContainer"
        );
        // Re-auth as Alice (1001) for remaining checks.
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
        s.preferences.local_passcode_hash =
            Some(SettingsLocalPasscodeSections::hash_passcode_hex("1234"));
    });

    // === 6. C2-I6-1: Passcode-Lock Line-1 Guard Across Deep-Links & All Hub/Sidebar Entry Points ===
    shell.update(cx, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.window_runtime.controller.clear_all_toasts();
        let msgs_before = s.stage.read(cx).messages.len();
        let books_gen0 = s.content_hub_state.books_catalog_gen;
        cx.write_to_clipboard(ClipboardItem::new_string("lock_clip_sentinel".to_string()));
        let clip_before = cx.read_from_clipboard().and_then(|c| c.text());

        assert!(s.lock_app_now(cx));
        s.update_ime_marked_text("pass_pre", cx);

        assert!(!s.handle_apple_deep_link("tg://apple/books/cr_lock1", cx));
        assert!(!s.handle_deep_link("tg://apple/music/cr_lock2", cx));
        assert!(!s.handle_deep_link("tg://chat?id=5001", cx));
        // try_handle_or_swallow_apple_deep_link MUST swallow (return true) with ZERO state/IME/toast/clipboard leak!
        assert!(s.try_handle_or_swallow_apple_deep_link("tg://apple/books/cr_lock3", false, cx));
        s.show_media_viewer("tg://apple/books/cr_lock4", false, cx);
        s.open_external_url_with_toast("tg://apple/music/cr_lock5", cx);
        s.handle_apple_sidebar_event(&AppleSidebarEvent::ToggleCreationsDrawer(true), cx);
        s.handle_apple_sidebar_event(
            &AppleSidebarEvent::SelectSection(AppleSidebarSection::Books),
            cx,
        );
        s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
        s.open_reviewer_queue_modal(cx);
        s.open_creator_studio_bot_chat(Some("/newbook "), cx);

        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::Chats,
            "C2-I6-1: locked deep-link/hub must NOT switch panels"
        );
        assert!(
            !s.content_hub.read(cx).quick_drawer_open,
            "C2-I6-1: locked ToggleCreationsDrawer must be no-op"
        );
        assert!(
            !s.content_hub.read(cx).publish_modal.is_open,
            "C2-I6-1: locked modal open must be no-op"
        );
        assert!(s.overlays.media_viewer_overlay.is_none());
        assert!(s.window_runtime.controller.current_toast.is_none());
        assert_eq!(s.stage.read(cx).messages.len(), msgs_before);
        assert_eq!(
            s.content_hub_state.books_catalog_gen, books_gen0,
            "C2-I6-1: zero fetches while locked"
        );
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), clip_before);
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("pass_pre"),
            "C2-I6-1: Passcode IME must survive locked deep-link/hub calls"
        );

        assert!(s.try_unlock_with_passcode("1234", cx));
    });

    // === 7. C2-I6-2: Complete Unicode Plane-14 / U+FFFC / Percent / Backslash / Single-Slash / Invalid-UTF8 Smuggling ===
    assert!(is_invisible_format_char('\u{E0080}'));
    assert!(is_invisible_format_char('\u{E01F0}'));
    assert!(is_invisible_format_char('\u{FFFC}'));
    assert_eq!(visible_char_count("\u{E0080}\u{E01F0}\u{FFFC}\u{2800}\u{3164}"), 0);
    assert_eq!(normalize_non_empty_reason("\u{E0080}\u{E01F0}\u{FFFC}"), None);
    let valid_unicode = "书 🚀 👨‍👩‍👧‍👦 ❤️ مرحبا";
    assert_eq!(
        normalize_non_empty_reason(valid_unicode).as_deref(),
        Some(valid_unicode)
    );

    for bad_cand in [
        "\u{E0080}tg://apple/books/cr_1",
        "\u{E01F0}tg://apple/books/cr_1",
        "\u{FFFC}tg://apple/books/cr_1",
        "crea\u{E0080}tion:open:book:cr_1",
        "ur\u{E0080}l:tg://apple/books/cr_1",
        "tg://%61pple/books",
        "tg://%41pple/books/<script>",
        "tg://%2561pple/books",
        "tg%3A//apple/books",
        "creation:open%3Abook:cr_1",
        "tg://\\apple/books",
        "tg:/apple/books",
        "tg://a%C2%ADpple/books",
    ] {
        assert!(
            parse_apple_deep_link(bad_cand).is_none(),
            "C2-I6-2: parser must reject {bad_cand:?}"
        );
        assert!(
            is_apple_deep_link_candidate(bad_cand),
            "C2-I6-2: candidate detector must catch {bad_cand:?}"
        );
    }
    for legit_non_apple in [
        "https://apple.com/books",
        "https://example.com/file%20name.pdf",
        "tg://resolve?domain=apple",
        "tg://chat?id=5001",
        "tg://applepay/checkout",
        "tg://apple_bot/app",
        "creation:opened:123",
    ] {
        assert!(
            !is_apple_deep_link_candidate(legit_non_apple),
            "C2-I6-2: zero false positive for {legit_non_apple:?}"
        );
    }

    // Verify zero leaks across MediaViewer, open_external_url_with_toast, MiniApp OpenInBrowser (0 xdg-open!), and non-UTF8 BotKeyboard Callback.
    let mut cb_msgs_before = 0usize;
    shell.update(cx, |s, cx| {
        s.window_runtime.controller.clear_all_toasts();
        let msgs_before = s.stage.read(cx).messages.len();
        cb_msgs_before = msgs_before;
        let ov = cx.new(|_| {
            let mut w = BotAttachWebView::new();
            w.is_open = true;
            w
        });
        s.miniapps.miniapp_overlay = Some(ov.clone());
        s.miniapps.is_miniapp_minimized = false;

        for payload in [
            "\u{E0080}tg://apple/books/cr_1",
            "tg://%61pple/books",
            "tg://\\apple/books",
        ] {
            s.show_media_viewer(payload, false, cx);
            s.open_external_url_with_toast(payload, cx);
            s.handle_stage_bot_button(1, InlineButtonType::Url(payload.to_string()), cx);
            ov.update(cx, |w, _| {
                w.pending_actions
                    .push_back(BotWebViewPendingAction::OpenInBrowser {
                        path: payload.to_string(),
                    })
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(s.overlays.media_viewer_overlay.is_none());
            assert!(
                s.window_runtime.controller.current_toast.is_none(),
                "C2-I6-2: zero toast/browser leak for {payload:?}"
            );
            assert_eq!(
                s.stage.read(cx).messages.len(),
                msgs_before,
                "C2-I6-2: zero outgoing msg leak for {payload:?}"
            );
            assert!(
                !s.miniapps.is_miniapp_minimized,
                "C2-I6-4: smuggled payload must NOT minimize MiniApp"
            );
        }
        s.miniapps.miniapp_overlay = None;

        // Invalid UTF-8 BotKeyboard Callback -> swallowed, btn.text NOT sent to chat!
        s.composer.update(cx, |c, ccx| {
            c.pending_bot_keyboard_press = Some(BotKeyboardButton::new(
                "Do Not Leak This Label",
                BotKeyboardButtonKind::Callback(b"tg://apple/books\xFF".to_vec()),
            ));
            ccx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.stage.read(cx).messages.len(),
            cb_msgs_before,
            "C2-I6-2: invalid-UTF8 Apple callback must NOT leak btn.text"
        );
    });

    // === 8. C2-I6-3: PublishReviewModal Recursive Validation (Tags Blank/XSS, Cover, BookPayload, AppStore web_url, %XX XSS) ===
    assert!(contains_obscured_xss_pattern(
        "<\u{E0080}script>alert(1)</script>"
    ));
    assert!(contains_obscured_xss_pattern(
        "https://app.example.com/?q=java%73cript:alert(1)"
    ));
    assert!(contains_obscured_xss_pattern(
        "https://app.example.com/?q=java%2573cript:alert(1)"
    ));
    assert!(!contains_obscured_xss_pattern(valid_unicode));
    assert!(!contains_obscured_xss_pattern(
        "https://cdn.example.com/cover%20art.png?v=100%25"
    ));
    {
        let mut bad_book = clean_book.clone();
        bad_book.tags = vec![
            "\u{2800}\u{3164}".to_string(),
            "<\u{00AD}script>alert(1)</script>".to_string(),
        ];
        bad_book.cover.thumbnail_data_url =
            Some("https://cdn.example.com/ok.png?x=java\u{00AD}script:alert(1)".to_string());
        if let CreationPayload::Book(ref mut b) = bad_book.payload {
            b.chapters[0].pages[0].blocks[0] = BookRichBlock::Paragraph {
                text: "Safe prefix <\u{E0080}script>alert(2)</script> padding to exceed fifty visible chars easily.".to_string(),
            };
        }
        let mut m = PublishReviewModalState::default();
        m.open_for_creation(bad_book);
        assert!(
            m.validation_errors.iter().any(|e| e.contains("tags[0]")),
            "C2-I6-3: blank filler tag must fail"
        );
        assert!(
            m.validation_errors.iter().any(|e| e.contains("tags[1]")),
            "C2-I6-3: obscured XSS tag must fail"
        );
        assert!(
            m.validation_errors
                .iter()
                .any(|e| e.contains("thumbnail_data_url")),
            "C2-I6-3: cover XSS must fail"
        );
        assert!(
            m.validation_errors.iter().any(|e| e.contains("payload.")),
            "C2-I6-3: BookPayload XSS must fail"
        );

        let mut bad_app = clean_book.clone();
        bad_app.kind = CreationKind::App;
        bad_app.payload = CreationPayload::App(AppStoreListingPayload {
            developer_name: "Dev Studio".to_string(),
            version_string: "1.0.0".to_string(),
            release_notes: "Initial release".to_string(),
            category: "Productivity".to_string(),
            long_description: "A clean app listing description exceeding minimum required length easily.".to_string(),
            screenshots: Vec::new(),
            size_label: "12 MB".to_string(),
            price_label: "Free".to_string(),
            permissions: Vec::new(),
            linked_miniapp: Some(LinkedMiniAppRef {
                bot_username: "demo_bot".to_string(),
                short_name: "demo_app".to_string(),
                web_url: Some("https://app.example.com/?q=java\u{00AD}script:alert(1)".to_string()),
                launch_param: None,
            }),
            ui_schema_json: None,
        });
        let mut m_app = PublishReviewModalState::default();
        m_app.open_for_creation(bad_app);
        assert!(
            m_app.validation_errors.iter().any(|e| e.contains("web_url")),
            "C2-I6-3: AppStore linked_miniapp.web_url XSS must fail"
        );

        // Positive valid Unicode preservation check:
        let mut good_book = clean_book.clone();
        good_book.title = valid_unicode.to_string();
        let mut m_good = PublishReviewModalState::default();
        m_good.open_for_creation(good_book);
        assert!(
            m_good.validation_errors.is_empty(),
            "C2-I6-2/3: valid ZWJ/VS16/CJK/RTL must produce 0 errors, got {:?}",
            m_good.validation_errors
        );
    }

    // === 9. C2-I6-4 + C2-I6-6: Valid Deep-Link Overlay Dismissal + Sibling #17/#18 IME Hygiene & IvViewer Close Clear ===
    shell.update(cx, |s, cx| {
        // C2-I6-6a: launch_miniapp_with_url with smuggled Apple URL preserves active QuickDrawerSearch IME!
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.toggle_quick_creations_drawer(true, cx);
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx)
        });
    });
    shell.update(cx, |s, cx| {
        s.update_ime_marked_text("qd_live", cx);
        s.launch_miniapp_with_url(
            "demo_bot",
            "demo_app",
            Some(5001),
            MiniAppLaunchMode::DirectLink,
            None,
            Some("tg://apple/<script>".to_string()),
            cx,
        );
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("qd_live"),
            "C2-I6-6: smuggled MiniApp URL must NOT wipe IME"
        );
        assert!(s.miniapps.miniapp_overlay.is_none());

        // C2-I6-6b: Opening normal IvViewer via pending_iv_url clears BOTH shell.ime and quick_drawer_search.ime_preedit.
        s.stage.update(cx, |st, _| {
            st.pending_iv_url = Some(("https://example.com/doc".to_string(), "Doc".to_string()));
        });
        s.handle_stage_pending_actions(cx);
        assert!(s.overlays.iv_viewer_overlay.is_some());
        assert!(
            s.ime.ime_preedit.is_none(),
            "C2-I6-6: opening IvViewer must clear shell.ime"
        );
        assert!(
            s.content_hub
                .read(cx)
                .quick_drawer_search
                .ime_preedit
                .is_none(),
            "C2-I6-6: opening IvViewer must clear hub preedit"
        );

        // Exp2 Extra: IvViewer observer should_close clears IME cleanly.
        let iv_close = s.overlays.iv_viewer_overlay.clone().unwrap();
        iv_close.update(cx, |v, vcx| {
            v.should_close = true;
            vcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert!(s.overlays.iv_viewer_overlay.is_none());
        assert!(s.ime.ime_preedit.is_none());

        // Re-open IvViewer for C2-I6-4a: Valid Apple deep-link from IvViewer::pending_share_url dismisses IvViewer!
        s.stage.update(cx, |st, _| {
            st.pending_iv_url = Some(("https://example.com/doc2".to_string(), "Doc2".to_string()));
        });
        s.handle_stage_pending_actions(cx);
        let iv = s.overlays.iv_viewer_overlay.clone().unwrap();
        iv.update(cx, |v, vcx| {
            v.pending_share_url = Some("tg://apple/books/cr_iv1".to_string());
            vcx.notify();
        });
    });
    shell.update(cx, |s, cx| {
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_iv1")
        );
        assert!(
            s.overlays.iv_viewer_overlay.is_none(),
            "C2-I6-4: valid Apple deep-link must dismiss IvViewer"
        );

        // C2-I6-4b: Valid Apple deep-link from MiniAppOverlay (OpenTelegramLink & OpenInBrowser) minimizes MiniApp & clears focused_input!
        let ov = cx.new(|_| {
            let mut w = BotAttachWebView::new();
            w.is_open = true;
            w.focused_input = Some(BotWebViewInputTarget::CopilotPrompt);
            w
        });
        s.miniapps.miniapp_overlay = Some(ov.clone());
        s.miniapps.is_miniapp_minimized = false;
        ov.update(cx, |w, _| {
            w.pending_actions
                .push_back(BotWebViewPendingAction::OpenTelegramLink(
                    "tg://apple/games/cr_wv1".to_string(),
                ))
        });
        s.drain_miniapp_overlay_actions(&ov, cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Games)
        );
        assert!(
            s.miniapps.is_miniapp_minimized,
            "C2-I6-4: OpenTelegramLink valid Apple deep-link must minimize MiniApp"
        );
        assert!(
            !ov.read(cx).is_open,
            "R1-I7-1/R2-I7-1: OpenTelegramLink valid Apple deep-link must set w.is_open = false via w.minimize()"
        );
        assert!(
            ov.read(cx).focused_input.is_none(),
            "C2-I6-4: minimized MiniApp must blur focused_input"
        );

        s.miniapps.is_miniapp_minimized = false;
        ov.update(cx, |w, _| {
            w.is_open = true;
            w.focused_input = Some(BotWebViewInputTarget::CopilotPrompt);
            w.pending_actions
                .push_back(BotWebViewPendingAction::OpenInBrowser {
                    path: "tg://apple/music/cr_wv2".to_string(),
                })
        });
        s.drain_miniapp_overlay_actions(&ov, cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Music)
        );
        assert!(
            s.miniapps.is_miniapp_minimized,
            "C2-I6-4: OpenInBrowser valid Apple deep-link must minimize MiniApp"
        );
        assert!(
            !ov.read(cx).is_open,
            "R1-I7-1/R2-I7-1: OpenInBrowser valid Apple deep-link must set w.is_open = false via w.minimize()"
        );
        s.miniapps.miniapp_overlay = None;
        s.toggle_quick_creations_drawer(false, cx);
    });

    // === 10. C2-I6-5: WS UserCreationChanged Same-Revision Monotonicity ===
    shell.update(cx, |s, cx| {
        let mut pub_rev5 = clean_book.clone();
        pub_rev5.creation_id = "cr_ws_mono".to_string();
        pub_rev5.review_status = CreationReviewStatus::Published;
        pub_rev5.cloud_revision = 5;
        s.content_hub.update(cx, |h, _| {
            h.upsert_my_creation(pub_rev5.clone());
            h.selected_creation_id = Some("cr_ws_mono".to_string());
            h.selected_detail_creation = Some(pub_rev5);
        });
        // Same-revision conflicting status (Draft @ rev=5) MUST be rejected!
        s.handle_ws_user_creation_changed(
            "cr_ws_mono".to_string(),
            1001,
            CreationKind::Book,
            CreationReviewStatus::Draft,
            5,
            cx,
        );
        assert_eq!(
            s.content_hub
                .read(cx)
                .find_my_creation("cr_ws_mono")
                .map(|c| c.review_status),
            Some(CreationReviewStatus::Published),
            "C2-I6-5: same-rev conflicting WS Draft must NOT regress Published @ rev=5"
        );
        // Strictly newer revision (Unpublished @ rev=6) MUST be accepted!
        s.handle_ws_user_creation_changed(
            "cr_ws_mono".to_string(),
            1001,
            CreationKind::Book,
            CreationReviewStatus::Unpublished,
            6,
            cx,
        );
        assert_eq!(
            s.content_hub
                .read(cx)
                .find_my_creation("cr_ws_mono")
                .map(|c| (c.review_status, c.cloud_revision)),
            Some((CreationReviewStatus::Unpublished, 6)),
            "C2-I6-5: strictly newer rev=6 WS transition must succeed"
        );
    });
}

#[test]
fn test_m5_gpui_iter8_reg_final_hardening_closure() {
    run_in_headless_subprocess("test_m5_gpui_iter8_reg_final_hardening_closure", true);
}

async fn body_iter8_reg_final_hardening_closure(mut app: AsyncApp) {
    let clean_book = sample_book_creation(
        "cr_iter8_clean",
        1001,
        "Iter8 Clean Book",
        "A valid summary longer than ten visible characters.",
        "Chapter one body has plenty of readable text exceeding fifty visible characters comfortably.",
        CreationReviewStatus::Draft,
        1,
    );
    let queue_book = sample_book_creation(
        "cr_iter8_queue",
        2002,
        "Iter8 Queue Book",
        "Pending review item in the local loopback mock reviewer queue.",
        "Chapter one body for review queue item exceeding fifty visible characters comfortably.",
        CreationReviewStatus::PendingReview,
        2,
    );

    let mock_state = Arc::new(MockM5State::new(
        true,
        vec![clean_book.clone()],
        vec![queue_book.clone()],
    ));
    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");

    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
            s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
            s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
            s.preferences.local_passcode_hash =
                Some(SettingsLocalPasscodeSections::hash_passcode_hex("1234"));
        });
    })
    .unwrap();

    // === 1. R1-I7-1 / R2-I7-1: MiniApp Deep-Link Minimization (`!w.is_open`) & Escape Not Deadlocked ===
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            let ov = cx.new(|_| {
                let mut w = BotAttachWebView::new();
                w.is_open = true;
                w.menu_open = true;
                w.confirm_close_open = true;
                w.focused_input = Some(BotWebViewInputTarget::CopilotPrompt);
                w
            });
            s.miniapps.miniapp_overlay = Some(ov.clone());
            s.miniapps.is_miniapp_minimized = false;

            assert!(s.handle_apple_deep_link("tg://apple/games/cr_r1i7", cx));
            assert_eq!(
                s.active_panel,
                ActiveWorkspacePanel::ContentHub(ContentHubSection::Games)
            );
            assert!(s.miniapps.is_miniapp_minimized);
            assert!(
                !ov.read(cx).is_open,
                "R1-I7-1/R2-I7-1: handle_apple_deep_link must call w.minimize() -> w.is_open == false"
            );
            assert!(!ov.read(cx).menu_open && !ov.read(cx).confirm_close_open);
            assert!(ov.read(cx).focused_input.is_none());

            // Escape Step 12 on open MiniApp (with no popups/focused_input) must call w.minimize() -> !w.is_open && is_miniapp_minimized.
            ov.update(cx, |w, _| {
                w.is_open = true;
                w.menu_open = false;
                w.confirm_close_open = false;
                w.focused_input = None;
            });
            s.miniapps.is_miniapp_minimized = false;
            assert!(s.handle_non_char_key("escape", false, cx));
            assert!(
                !ov.read(cx).is_open && s.miniapps.is_miniapp_minimized,
                "R1-I7-1: Escape Step 12 must cleanly minimize MiniApp without deadlocking"
            );
            s.miniapps.miniapp_overlay = None;
            s.miniapps.is_miniapp_minimized = false;
        });
    })
    .unwrap();

    // === 2. R1-I7-2: Downward Modal-Close IME Clear Over Lower-Z Non-Composer Inputs ===
    // (a) ModalContainer (CreatePoll, Z #13) over ContactsModal (Z #6)
    shell.update(&mut app, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.open_contacts_modal();
            scx.notify();
        });
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("poll_to_contacts", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("poll_to_contacts"));
        s.modal_container.update(cx, |mc, mcx| {
            mc.dismiss_top();
            mcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "R1-I7-2: ModalContainer close over ContactsModal must clear IME"
        );
        assert_eq!(s.active_text_and_preedit(cx), (String::new(), None));
        s.sidebar.update(cx, |sb, scx| {
            sb.close_contacts_modal();
            scx.notify();
        });
    }).unwrap();

    // (b) ModalContainer (CreatePoll, Z #13) over SettingsInput (SettingsSearch, Z #9)
    shell.update(&mut app, |s, cx| {
        s.active_panel = ActiveWorkspacePanel::Settings;
        s.settings_router.update(cx, |r, rcx| {
            let _ = r.ensure_search(rcx);
            r.navigate_to(SettingsSectionType::Search, true, rcx);
        });
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("poll_to_settings", cx);
        s.modal_container.update(cx, |mc, mcx| {
            mc.dismiss_top();
            mcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "R1-I7-2: ModalContainer close over SettingsInput must clear IME"
        );
        s.settings_router.update(cx, |r, rcx| r.close(rcx));
        s.active_panel = ActiveWorkspacePanel::Chats;
    }).unwrap();

    // (c) ContactsModal (Z #6) over CreateBotBox (Z #5b) & CreationModal (Z #7) over MiniAppInput (Z #5a)
    shell.update(&mut app, |s, cx| {
        s.open_nl_creator_modal(cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.open_contacts_modal();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("contacts_to_botbox", cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.close_contacts_modal();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "R1-I7-2: ContactsModal close over CreateBotBox must clear IME"
        );
        s.miniapps.create_bot_box = None;

        let ov = cx.new(|_| {
            let mut w = BotAttachWebView::new();
            w.is_open = true;
            w.focused_input = Some(BotWebViewInputTarget::CopilotPrompt);
            w
        });
        s.miniapps.miniapp_overlay = Some(ov);
        s.miniapps.is_miniapp_minimized = false;
        s.sidebar.update(cx, |sb, scx| {
            sb.open_create_group_modal();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("creation_to_miniapp", cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.close_creation_modal();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, _cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "R1-I7-2: CreationModal close over MiniAppInput must clear IME"
        );
        s.miniapps.miniapp_overlay = None;
    }).unwrap();

    // === 3. R1-I7-3: Occluded ContentHubEvent::FocusInputTaken Preserves Higher-Z IME ===
    // (a) ContentHub behind ModalContainer
    shell.update(&mut app, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Books, cx);
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("mc_protected", cx);
        s.handle_content_hub_event(&ContentHubEvent::FocusInputTaken, cx);
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("mc_protected"),
            "R1-I7-3: occluded ContentHub FocusInputTaken must NOT wipe ModalContainer IME"
        );
        s.modal_container.update(cx, |mc, mcx| {
            mc.hide_all();
            mcx.notify();
        });
    }).unwrap();

    // (b) PublishReviewModal (Z #14) behind ComputerTerminal (Z #15)
    shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
        s.show_remote_computer(cx);
        if let Some(cw) = s.overlays.computer_overlay.clone() {
            cw.update(cx, |w, wcx| {
                w.active_tab = WorkbenchTab::RemoteTerminal;
                w.terminal_focused = true;
                wcx.notify();
            });
        }
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("cw_focus_safe", cx);
        s.handle_content_hub_event(&ContentHubEvent::FocusInputTaken, cx);
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("cw_focus_safe"),
            "R1-I7-3: occluded PublishReviewModal FocusInputTaken must NOT wipe Z #15 ComputerTerminal IME"
        );
        let cw_pre = s
            .overlays
            .computer_overlay
            .as_ref()
            .and_then(|cw| cw.read(cx).terminal_text_and_preedit(cx).1);
        assert_eq!(cw_pre.as_deref(), Some("cw_focus_safe"));
        s.hide_remote_computer(cx);
        s.close_publish_review_modal(cx);
    }).unwrap();

    // === 4. R2-I7-2: Z #5a MiniApp Close / Minimize Preserves Z #5b CreateManagedBotBox IME ===
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            let ov = cx.new(|_| {
                let mut w = BotAttachWebView::new();
                w.is_open = true;
                w
            });
            s.miniapps.miniapp_overlay = Some(ov.clone());
            s.miniapps.is_miniapp_minimized = false;
            s.open_nl_creator_modal(cx);
            s.update_ime_marked_text("botbox_z5b_live", cx);

            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::Minimize);
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert_eq!(
                s.ime.ime_preedit.as_deref(),
                Some("botbox_z5b_live"),
                "R2-I7-2: Z #5a MiniApp Minimize must preserve Z #5b CreateManagedBotBox IME"
            );

            ov.update(cx, |w, _| {
                w.pending_actions.push_back(BotWebViewPendingAction::Close);
            });
            s.drain_miniapp_overlay_actions(&ov, cx);
            assert!(s.miniapps.miniapp_overlay.is_none());
            assert_eq!(
                s.ime.ime_preedit.as_deref(),
                Some("botbox_z5b_live"),
                "R2-I7-2: Z #5a MiniApp Close must preserve Z #5b CreateManagedBotBox IME"
            );
            s.miniapps.create_bot_box = None;
            s.clear_all_ime_preedits(cx);
        });
    })
    .unwrap();

    // === 5. R2-I7-3: Async `probe_reviewer_queue` Field Switch (Title <-> RejectReason) Clears `shell.ime` ===
    let r_before_ok = mock_state
        .review_queue_responses_served
        .load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.reviewer_access = ReviewerAccessState::Unknown;
        });
        s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.pending_open_reviewer_queue = true;
            h.publish_modal.focused_field = Some(PublishModalField::Title);
        });
        s.update_ime_marked_text("title_before_queue_switch", cx);
        s.probe_reviewer_queue(cx, true);
    });
    wait_until(&mut app, 60, "R2-I7-3 200 OK reviewer queue switch", |a| {
        mock_state
            .review_queue_responses_served
            .load(Ordering::SeqCst)
            > r_before_ok
            && shell
                .read_with(a, |s, cx| {
                    let m = &s.content_hub.read(cx).publish_modal;
                    !m.is_loading_queue && m.focused_field == Some(PublishModalField::RejectReason)
                })
                .unwrap_or(false)
    })
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "R2-I7-3: 200 OK probe_reviewer_queue Title->RejectReason switch must clear shell.ime"
        );
        assert_eq!(
            s.content_hub
                .read(cx)
                .publish_modal
                .active_text_and_preedit(&s.ime)
                .1,
            None
        );

        // Now flip mock to 403 Forbidden while composing in RejectReason -> must switch back to Title & clear shell.ime!
        mock_state.admin_allowed.store(false, Ordering::SeqCst);
        s.update_ime_marked_text("reject_before_403", cx);
    });
    let r_before_403 = mock_state
        .review_queue_responses_served
        .load(Ordering::SeqCst);
    let _ = shell.update(&mut app, |s, cx| {
        s.probe_reviewer_queue(cx, false);
    });
    wait_until(&mut app, 60, "R2-I7-3 403 Forbidden reviewer queue reset", |a| {
        mock_state
            .review_queue_responses_served
            .load(Ordering::SeqCst)
            > r_before_403
            && shell
                .read_with(a, |s, cx| {
                    let m = &s.content_hub.read(cx).publish_modal;
                    !m.is_loading_queue
                        && m.reviewer_access == ReviewerAccessState::Forbidden
                        && m.focused_field == Some(PublishModalField::Title)
                })
                .unwrap_or(false)
    })
    .await;
    let _ = shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "R2-I7-3: 403 Forbidden probe_reviewer_queue RejectReason->Title reset must clear shell.ime"
        );
        s.close_publish_review_modal(cx);
    });

    // === 6. C2-I7-1 / C1-I7-3: `handle_apple_deep_link` Dismisses ALL 7 Modals + Settings + `is_compose_isolated` (Preserved on Invalid) ===
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            let bp_dto = BotPublicProfileDto {
                bot: BotRegistryEntry {
                    bot_user_id: 777_100,
                    username: "privacy_bot".to_string(),
                    display_name: "Privacy Bot".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let bp_ent = cx.new(|_| BotProfileBox::new(bp_dto.clone(), s.theme_state.palette, "en"));

            s.open_modal_dialog("Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
            s.sidebar.update(cx, |sb, scx| {
                sb.open_create_group_modal();
                sb.is_contacts_modal_open = true;
                scx.notify();
            });
            s.open_nl_creator_modal(cx);
            s.miniapps.pending_miniapp_confirm = Some(PendingMiniAppConfirm {
                bot_id: 777_100,
                bot_username: "privacy_bot".to_string(),
                bot_display_name: "Privacy Bot".to_string(),
                app_id: None,
                short_name: "app".to_string(),
                app_title: "App".to_string(),
                source_chat_id: Some(5001),
                launch_mode: MiniAppLaunchMode::DirectLink,
                start_param: None,
                custom_url: None,
            });
            s.miniapps.miniapp_tos_modal_open = true;
            s.miniapps.bot_profile_box = Some(bp_ent);
            s.active_panel = ActiveWorkspacePanel::Settings;
            s.nav_rail.update(cx, |r, rcx| {
                r.set_active_destination(NavDestination::Settings);
                rcx.notify();
            });
            s.settings_router.update(cx, |r, rcx| {
                r.navigate_to(SettingsSectionType::PrivacySecurity, true, rcx);
            });
            s.composer.update(cx, |c, _| c.is_compose_isolated = true);

            // Invalid Apple deep-link MUST preserve ALL 7 modals, Settings, and isolation.
            assert!(!s.handle_apple_deep_link("tg://apple/books/<invalid>", cx));
            assert!(s.modal_container.read(cx).is_modal_active());
            assert!(s.sidebar.read(cx).active_creation_modal.is_some());
            assert!(s.sidebar.read(cx).is_contacts_modal_open);
            assert!(s.miniapps.create_bot_box.is_some());
            assert!(s.miniapps.pending_miniapp_confirm.is_some());
            assert!(s.miniapps.miniapp_tos_modal_open);
            assert!(s.miniapps.bot_profile_box.is_some());
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
            assert!(s.composer.read(cx).is_compose_isolated);

            // Valid Apple deep-link MUST dismiss ALL 7 modals + Settings + sync nav_rail + clear is_compose_isolated!
            assert!(s.handle_apple_deep_link("tg://apple/books/cr_dismiss_all", cx));
            assert_eq!(
                s.active_panel,
                ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
            );
            assert_eq!(
                s.content_hub.read(cx).selected_creation_id.as_deref(),
                Some("cr_dismiss_all")
            );
            assert!(!s.modal_container.read(cx).is_modal_active());
            assert!(s.sidebar.read(cx).active_creation_modal.is_none());
            assert!(!s.sidebar.read(cx).is_contacts_modal_open);
            assert!(s.miniapps.create_bot_box.is_none());
            assert!(s.miniapps.pending_miniapp_confirm.is_none());
            assert!(!s.miniapps.miniapp_tos_modal_open);
            assert!(s.miniapps.bot_profile_box.is_none());
            assert_eq!(
                s.settings_router.read(cx).settings_section,
                SettingsSectionType::Main
            );
            assert!(s.settings_router.read(cx).settings_stack.is_empty());
            assert_ne!(
                s.nav_rail.read(cx).active_destination,
                NavDestination::Settings
            );
            assert!(!s.composer.read(cx).is_compose_isolated);
        });
    })
    .unwrap();

    // Verify ContentHub(Books) survives GPUI observer flush (SettingsRouter::close_silent & nav_rail trigger sync)!
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books),
            "C1-I7-3: active_panel must remain ContentHub(Books) after observer flush"
        );
        let bp_dto = BotPublicProfileDto {
            bot: BotRegistryEntry {
                bot_user_id: 777_100,
                username: "privacy_bot".to_string(),
                display_name: "Privacy Bot".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        // Real BotProfileBoxAction::OpenPrivacyPolicy("tg://apple/...") flow dismisses BotProfileBox!
        let bp2 = cx.new(|_| BotProfileBox::new(bp_dto, s.theme_state.palette, "en"));
        s.miniapps.bot_profile_box = Some(bp2.clone());
        bp2.update(cx, |b, _| {
            b.pending_actions
                .push_back(BotProfileBoxAction::OpenPrivacyPolicy(
                    "tg://apple/music/cr_bp_flow".to_string(),
                ));
        });
        s.drain_bot_profile_box_actions(&bp2, cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Music)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_bp_flow")
        );
        assert!(
            s.miniapps.bot_profile_box.is_none(),
            "C2-I7-1: BotProfileBox Privacy Policy Apple deep-link must dismiss BotProfileBox"
        );
    }).unwrap();

    // === 7. C2-I7-2: Direct Account Switch Clears `miniapps.catalog_miniapps` Before `refresh_content_hub_on_login` ===
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.miniapps.catalog_miniapps.push(MiniAppEntry {
                app_id: "app_a_priv".to_string(),
                bot_user_id: 900_001,
                bot_username: "user_a_private_bot".to_string(),
                bot_display_name: "User A Private".to_string(),
                short_name: "secret".to_string(),
                title: "User A Secret MiniApp".to_string(),
                ..Default::default()
            });
            s.switch_to_content_hub(ContentHubSection::AppStore, cx);
            assert_eq!(s.content_hub.read(cx).local_miniapps_fallback.len(), 1);

            // Direct User A -> User B switch (NO intervening logout!)
            s.apply_authenticated_session(sample_auth(1002, "bob"), cx);
            assert!(
                s.miniapps.catalog_miniapps.is_empty(),
                "C2-I7-2: User B catalog_miniapps must be empty"
            );
            assert!(
                s.content_hub.read(cx).local_miniapps_fallback.is_empty(),
                "C2-I7-2: User B local_miniapps_fallback must have ZERO User A MiniApp leak"
            );

            // Switch back to Alice (1001) for remaining sections.
            s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
            s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
            s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
            s.preferences.local_passcode_hash =
                Some(SettingsLocalPasscodeSections::hash_passcode_hex("1234"));
        });
    })
    .unwrap();

    // === 8. C2-I7-3: Fixed-Point Percent, Fullwidth, RFC 3986 Userinfo, Space/NBSP, Noncharacters & Non-UTF8 Quad ===
    assert!(is_invisible_format_char('\u{FDD0}'));
    assert!(is_invisible_format_char('\u{FDEF}'));
    assert!(is_invisible_format_char('\u{FFFE}'));
    assert!(is_invisible_format_char('\u{FFFF}'));
    assert!(is_invisible_format_char('\u{10FFFF}'));
    assert_eq!(visible_char_count("\u{FDD0}\u{FDEF}\u{FFFE}\u{FFFF}\u{10FFFF}"), 0);
    assert_eq!(normalize_non_empty_reason("\u{FDD0}\u{FFFF}"), None);

    let pct100_host = format!("tg://%25{}61pple/books", "25".repeat(99));
    for smuggled in [
        "tg://%25252561pple/books",
        "tg%2525253a//apple/books",
        "tg://%25252525252525252561pple/books",
        pct100_host.as_str(),
        "tg://\u{FF41}pple/books",
        "tg://\u{FF41}\u{FF50}\u{FF50}\u{FF4C}\u{FF45}/books",
        "\u{FF54}\u{FF47}://apple/books",
        "tg:\u{FF0F}\u{FF0F}apple/books",
        "tg://\u{FF05}25252561pple/books",
        "tg://user@apple/books",
        "tg://@apple/books",
        "tg://user%40apple/books",
        "tg ://apple/books",
        "url :tg://apple/books",
        "creation :open:book:cr_1",
        "creation: open:book:cr_1",
        "tg://a\u{00A0}pple/books",
        "tg://a\u{3000}pple/books",
        "tg://a\u{FDD0}pple/books",
        "tg://a\u{FFFF}pple/books",
        "tg://a\u{10FFFF}pple/books",
    ] {
        assert!(
            parse_apple_deep_link(smuggled).is_none(),
            "C2-I7-3: parser must reject smuggled {smuggled:?}"
        );
        assert!(
            is_apple_deep_link_candidate(smuggled),
            "C2-I7-3: candidate detector must catch {smuggled:?}"
        );
    }
    for benign in [
        "https://apple.com/books",
        "https://example.com/@apple",
        "tg://resolve?domain=apple",
        "tg://resolve?url=user@apple",
        "tg://chat?id=5001",
        "tg://applepay/checkout",
        "tg://apple_bot/app",
        "tg://apple_store/x",
        "tg://apple-music/x",
        "creation:opened:123",
    ] {
        assert!(
            !is_apple_deep_link_candidate(benign),
            "C2-I7-3: zero false positive for {benign:?}"
        );
    }

    // End-to-end zero leaks (zero xdg-open): MediaViewer (unlocked + locked!), toast, chat, MiniApp + IME, non-UTF8 quad callback.
    let mut cb_msgs_before = 0usize;
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
            s.window_runtime.controller.clear_all_toasts();
            let msgs_before = s.stage.read(cx).messages.len();
            cb_msgs_before = msgs_before;

            // Even while PasscodeLock is active, quad-encoded Apple URL MUST NOT open MediaViewer behind lockscreen!
            assert!(s.lock_app_now(cx));
            s.update_ime_marked_text("pass_quad_safe", cx);
            s.show_media_viewer("tg://%25252561pple/books", false, cx);
            assert!(s.overlays.media_viewer_overlay.is_none());
            assert_eq!(s.ime.ime_preedit.as_deref(), Some("pass_quad_safe"));
            assert!(s.try_unlock_with_passcode("1234", cx));

            // Unlocked end-to-end smuggling sweep with live MiniApp IME.
            let ov = cx.new(|_| {
                let mut w = BotAttachWebView::new();
                w.is_open = true;
                w.focused_input = Some(BotWebViewInputTarget::CopilotPrompt);
                w
            });
            s.miniapps.miniapp_overlay = Some(ov.clone());
            s.miniapps.is_miniapp_minimized = false;
            s.update_ime_marked_text("ma_live_keep", cx);

            for payload in [
                "tg://%25252561pple/books",
                "tg://\u{FF41}pple/books",
                "tg://user@apple/books",
                "tg ://apple/books",
                "tg://a\u{00A0}pple/books",
                "tg://a\u{FDD0}pple/books",
            ] {
                s.show_media_viewer(payload, false, cx);
                s.open_external_url_with_toast(payload, cx);
                s.handle_stage_bot_button(1, InlineButtonType::Url(payload.to_string()), cx);
                ov.update(cx, |w, _| {
                    w.pending_actions
                        .push_back(BotWebViewPendingAction::OpenInBrowser {
                            path: payload.to_string(),
                        });
                });
                s.drain_miniapp_overlay_actions(&ov, cx);
                assert!(s.overlays.media_viewer_overlay.is_none());
                assert!(s.window_runtime.controller.current_toast.is_none());
                assert_eq!(s.stage.read(cx).messages.len(), msgs_before);
                assert!(!s.miniapps.is_miniapp_minimized);
                assert_eq!(s.ime.ime_preedit.as_deref(), Some("ma_live_keep"));
            }
            s.miniapps.miniapp_overlay = None;
            s.clear_all_ime_preedits(cx);

            // Non-UTF8 quad-encoded BotKeyboard Callback -> swallowed with zero chat leak!
            s.composer.update(cx, |c, ccx| {
                c.pending_bot_keyboard_press = Some(BotKeyboardButton::new(
                    "Never Leak Quad Callback",
                    BotKeyboardButtonKind::Callback(b"tg://%25252561pple/books\xFF".to_vec()),
                ));
                ccx.notify();
            });
        });
    })
    .unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.stage.read(cx).messages.len(),
            cb_msgs_before,
            "C2-I7-3: non-UTF8 quad callback must NOT leak btn.text"
        );
    }).unwrap();

    // === 9. C2-I7-4: Fixed-Point XSS (3x/10x %XX, data: URIs, HTML Entities, Nonchars, Fullwidth) & Blank-Filler Payloads ===
    for xss in [
        "java%252573cript:alert(1)",
        "java%25252525252525252573cript:alert(1)",
        "%25253cscript>alert(1)</script>",
        "vb%252573cript:msgbox(1)",
        "java%252500script:alert(1)",
        "java%2525E2%252580%25258Bscript:alert(1)",
        "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
        "data:\ttext/html,<img src=x onerror=alert(1)>",
        "data:image/svg+xml,<svg onload=alert(1)>",
        "data:application/xhtml+xml,<html/>",
        "&#x6a;avascript:alert(1)",
        "&#106;avascript:alert(1)",
        "&#0000106;avascript:alert(1)",
        "java&#x09;script:alert(1)",
        "&#x3c;script>alert(1)</script>",
        "&lt;script>alert(1)</script>",
        "javascript&colon;alert(1)",
        "java&tab;script:alert(1)",
        "&amp;#x6a;avascript:alert(1)",
        "java&#0;script:alert(1)",
        "<\u{FDD0}script>alert(1)</script>",
        "<\u{FFFF}script>alert(1)</script>",
        "\u{FF1C}\u{FF53}cript>alert(1)</script>",
        "\u{FF4A}avascript:alert(1)",
    ] {
        assert!(
            contains_obscured_xss_pattern(xss),
            "C2-I7-4: contains_obscured_xss_pattern must catch {xss:?}"
        );
    }
    let valid_unicode = "书 🚀 👨‍👩‍👧‍👦 ❤️ مرحبا";
    for safe_text in [
        valid_unicode,
        "https://cdn.example.com/cover%20art.png?v=100%25",
        "data: 123",
        "a < b",
        "Tom & Jerry < 3",
        "data:image/png;base64,iVBORw0KGgo=",
    ] {
        assert!(
            !contains_obscured_xss_pattern(safe_text),
            "C2-I7-4: zero false positive for {safe_text:?}"
        );
    }

    // Blank Braille/Hangul-filler (`\u{2800}\u{3164}`) payloads rejected across Book, Music, Game, App!
    {
        let filler = "\u{2800}\u{3164}";
        let mut blank_book = clean_book.clone();
        if let CreationPayload::Book(ref mut b) = blank_book.payload {
            b.chapters[0].title = filler.to_string();
            b.chapters[0].pages[0].blocks[0] = BookRichBlock::Paragraph {
                text: "\u{2800}".repeat(60),
            };
        }
        let mut m = PublishReviewModalState::default();
        m.open_for_creation(blank_book);
        assert!(m.validation_errors.iter().any(|e| e.contains("book.chapters[0].title")));
        assert!(m.validation_errors.iter().any(|e| e.contains("book must contain at least")));

        let mut blank_music = clean_book.clone();
        blank_music.kind = CreationKind::Music;
        blank_music.payload = CreationPayload::Music(MusicPayload {
            artist: "Synth Artist".to_string(),
            album_title: "Album".to_string(),
            release_year: 2026,
            primary_genre: "Ambient".to_string(),
            mood_tags: vec!["calm".to_string()],
            bpm: 110,
            musical_key: "C".to_string(),
            time_signature: "4/4".to_string(),
            total_duration_secs: 60,
            tracks: vec![MusicTrack {
                track_id: "t1".to_string(),
                track_number: 1,
                title: filler.to_string(),
                duration_secs: 60,
                bpm: 110,
                musical_key: "C".to_string(),
                instruments: Vec::new(),
                melody_notes: Vec::new(),
                chord_progression: Vec::new(),
                lyrics: Vec::new(),
                audio_media_file_id: Some(filler.to_string()),
                waveform_5bit: Vec::new(),
            }],
        });
        let mut m_music = PublishReviewModalState::default();
        m_music.open_for_creation(blank_music.clone());
        assert!(m_music.validation_errors.iter().any(|e| e.contains("music.tracks[0].title")));
        assert!(m_music.validation_errors.iter().any(|e| e.contains("music.tracks[0] must provide")));
        if let CreationPayload::Music(ref mut mu) = blank_music.payload {
            mu.tracks[0].title = "Starlight".to_string();
            mu.tracks[0].audio_media_file_id = None;
            mu.tracks[0].melody_notes = (0..4)
                .map(|i| NoteEvent {
                    midi_note: 60 + i,
                    start_beat: i as f32,
                    duration_beats: 1.0,
                    velocity: 96,
                    channel: 0,
                })
                .collect();
        }
        m_music.open_for_creation(blank_music);
        assert!(
            m_music.validation_errors.is_empty(),
            "valid Music must pass: {:?}",
            m_music.validation_errors
        );

        let mut blank_game = clean_book.clone();
        blank_game.kind = CreationKind::Game;
        blank_game.payload = CreationPayload::Game(GamePayload {
            developer: filler.to_string(),
            category: GameCategory::Trivia,
            controls_hint: "Click".to_string(),
            rules_markdown: filler.to_string(),
            engine_kind: GameEngineKind::TriviaQuiz,
            supports_leaderboard: true,
            estimated_session_mins: 5,
            spec: GameSpec::TriviaQuiz {
                time_limit_secs: Some(30),
                questions: vec![TriviaQuestion {
                    question_id: "q1".to_string(),
                    prompt: filler.to_string(),
                    choices: vec![filler.to_string(), "Valid".to_string()],
                    correct_index: 1,
                    explanation: None,
                    points: 100,
                }],
            },
            achievements: Vec::new(),
        });
        let mut m_game = PublishReviewModalState::default();
        m_game.open_for_creation(blank_game);
        assert!(m_game.validation_errors.iter().any(|e| e.contains("game.developer")));
        assert!(m_game.validation_errors.iter().any(|e| e.contains("game.rules_markdown")));
        assert!(m_game.validation_errors.iter().any(|e| e.contains("questions[0].prompt")));
        assert!(m_game.validation_errors.iter().any(|e| e.contains("choices[0]")));

        let mut blank_cards = clean_book.clone();
        blank_cards.kind = CreationKind::Game;
        blank_cards.payload = CreationPayload::Game(GamePayload {
            developer: "Studio".to_string(),
            category: GameCategory::CardMatch,
            controls_hint: "Tap".to_string(),
            rules_markdown: "Match pairs".to_string(),
            engine_kind: GameEngineKind::CardMatch,
            supports_leaderboard: false,
            estimated_session_mins: 3,
            spec: GameSpec::CardMatch {
                pairs: vec![
                    CardPair {
                        pair_id: "p1".to_string(),
                        label: filler.to_string(),
                        symbol: "★".to_string(),
                        accent_hex: None,
                    },
                    CardPair {
                        pair_id: "p2".to_string(),
                        label: "Moon".to_string(),
                        symbol: "☾".to_string(),
                        accent_hex: None,
                    },
                ],
                time_limit_secs: None,
            },
            achievements: Vec::new(),
        });
        let mut m_cards = PublishReviewModalState::default();
        m_cards.open_for_creation(blank_cards);
        assert!(m_cards.validation_errors.iter().any(|e| e.contains("pairs[0].label")));

        let mut blank_app = clean_book.clone();
        blank_app.kind = CreationKind::App;
        blank_app.payload = CreationPayload::App(AppStoreListingPayload {
            developer_name: filler.to_string(),
            version_string: filler.to_string(),
            release_notes: String::new(),
            category: "Tools".to_string(),
            long_description: filler.to_string(),
            screenshots: Vec::new(),
            size_label: "4 MB".to_string(),
            price_label: "Free".to_string(),
            permissions: Vec::new(),
            linked_miniapp: Some(LinkedMiniAppRef {
                bot_username: filler.to_string(),
                short_name: filler.to_string(),
                web_url: None,
                launch_param: None,
            }),
            ui_schema_json: None,
        });
        let mut m_app = PublishReviewModalState::default();
        m_app.open_for_creation(blank_app);
        assert!(m_app.validation_errors.iter().any(|e| e.contains("app.developer_name")));
        assert!(m_app.validation_errors.iter().any(|e| e.contains("app.version_string")));
        assert!(m_app.validation_errors.iter().any(|e| e.contains("app.long_description")));
        assert!(m_app.validation_errors.iter().any(|e| e.contains("app.linked_miniapp")));

        let mut good_book = clean_book.clone();
        good_book.title = valid_unicode.to_string();
        good_book.summary = "Normal prose with data: 123 and a < b & ❤️ 👨‍👩‍👧‍👦".to_string();
        let mut m_good = PublishReviewModalState::default();
        m_good.open_for_creation(good_book);
        assert!(
            m_good.validation_errors.is_empty(),
            "C2-I7-4: valid ZWJ/CJK/RTL & normal prose must produce 0 errors: {:?}",
            m_good.validation_errors
        );
    }

    // === 10. C2-I7-5 / C1-I7-5: Occluded Z #14 PublishReviewModal Close Preserves Z #15 ComputerTerminal IME ===
    shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
        s.show_remote_computer(cx);
        if let Some(cw) = s.overlays.computer_overlay.clone() {
            cw.update(cx, |w, wcx| {
                w.active_tab = WorkbenchTab::RemoteTerminal;
                w.terminal_focused = true;
                wcx.notify();
            });
        }
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("cw_z15_term_live", cx);
        s.close_publish_review_modal(cx);
        assert!(!s.is_publish_review_modal_active(cx));
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("cw_z15_term_live"),
            "C2-I7-5/C1-I7-5: closing occluded Z #14 PublishReviewModal must preserve Z #15 ComputerTerminal IME"
        );
        let cw_pre = s
            .overlays
            .computer_overlay
            .as_ref()
            .and_then(|cw| cw.read(cx).terminal_text_and_preedit(cx).1);
        assert_eq!(cw_pre.as_deref(), Some("cw_z15_term_live"));
        s.hide_remote_computer(cx);
    }).unwrap();

    // === 11. C1-I7-1 + C1-I7-4: All Fixed Enter Targets Consume `just_committed_ime`; Empty `unmark_text` + Composer `sync_autocomplete` ===
    // (a) PasscodeLock Enter consumes just_committed_ime without premature submit/wipe
    shell.update(&mut app, |s, cx| {
        assert!(s.lock_app_now(cx));
        s.insert_committed_text("12", cx);
        s.ime.mark_ime_committed();
        assert!(s.handle_non_char_key("enter", false, cx));
        assert_eq!(
            s.overlays.passcode_unlock_input,
            "12",
            "C1-I7-1: PasscodeLock Enter must not wipe on IME commit"
        );
        assert!(s.overlays.passcode_unlock_error.is_none());
        assert!(
            !s.ime.just_committed_ime,
            "C1-I7-1: PasscodeLock must consume just_committed_ime"
        );
        assert!(s.try_unlock_with_passcode("1234", cx));

        // (b) CreationModal Enter consumes just_committed_ime without premature submit/close
        s.sidebar.update(cx, |sb, scx| {
            sb.open_create_group_modal();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
            s.insert_committed_text("测试群", cx);
            s.ime.mark_ime_committed();
            assert!(s.handle_non_char_key("enter", false, cx));
            assert!(
                s.sidebar.read(cx).active_creation_modal.is_some(),
                "C1-I7-1: CreationModal must stay open"
            );
            assert!(s.sidebar.read(cx).pending_create_group_request.is_none());
            assert!(!s.ime.just_committed_ime);
            s.sidebar.update(cx, |sb, scx| {
                sb.close_creation_modal();
                scx.notify();
            });

            // (b2) ContactsModal Enter drains just_committed_ime cleanly
            s.sidebar.update(cx, |sb, scx| {
                sb.open_contacts_modal();
                scx.notify();
            });
            s.ime.mark_ime_committed();
            assert!(s.handle_non_char_key("enter", false, cx));
            assert!(!s.ime.just_committed_ime);
            s.sidebar.update(cx, |sb, scx| {
                sb.close_contacts_modal();
                scx.notify();
            });

            // (c) CreateBotBox Enter consumes just_committed_ime without premature submit
            s.open_nl_creator_modal(cx);
            s.insert_committed_text("帮我做一个机器人", cx);
            s.ime.mark_ime_committed();
            assert!(s.handle_non_char_key("enter", false, cx));
            let b = s.miniapps.create_bot_box.as_ref().unwrap().read(cx);
            assert!(
                !b.is_submitting && b.pending_submit_nl.is_none(),
                "C1-I7-1: CreateBotBox must not prematurely submit"
            );
            assert!(!s.ime.just_committed_ime);
            s.miniapps.create_bot_box = None;

            // (d) MiniAppInput Enter consumes just_committed_ime without clearing copilot_prompt
            let ov = cx.new(|_| {
                let mut w = BotAttachWebView::new();
                w.is_open = true;
                w.focused_input = Some(BotWebViewInputTarget::CopilotPrompt);
                w
            });
            s.miniapps.miniapp_overlay = Some(ov.clone());
            s.miniapps.is_miniapp_minimized = false;
            s.insert_committed_text("优化界面", cx);
            s.ime.mark_ime_committed();
            assert!(s.handle_non_char_key("enter", false, cx));
            assert_eq!(
                ov.read(cx).copilot_prompt,
                "优化界面",
                "C1-I7-1: MiniAppInput must not prematurely submit"
            );
            assert!(!s.ime.just_committed_ime);
            s.miniapps.miniapp_overlay = None;

            // (e) SidebarSearch Enter consumes just_committed_ime without blurring search
            s.sidebar.update(cx, |sb, scx| {
                sb.search_focused = true;
                scx.notify();
            });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
            s.insert_committed_text("测试", cx);
            s.ime.mark_ime_committed();
            assert!(s.handle_non_char_key("enter", false, cx));
            assert!(
                s.sidebar.read(cx).search_focused,
                "C1-I7-1: SidebarSearch must stay focused on IME commit Enter"
            );
            assert!(!s.ime.just_committed_ime);
            s.sidebar.update(cx, |sb, scx| {
                sb.search_focused = false;
                scx.notify();
            });

            // (f) StageSearch Enter consumes just_committed_ime without stepping search match
            s.append_outgoing_text("消息命中一".to_string(), cx);
            s.append_outgoing_text("消息命中二".to_string(), cx);
            s.stage.update(cx, |st, stcx| {
                st.focus_search();
                stcx.notify();
            });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
            s.insert_committed_text("消息命中", cx);
            s.ime.mark_ime_committed();
            let cur_before = s.stage.read(cx).search_match_cursor;
            assert!(s.stage.read(cx).search_match_ids.len() >= 2);
            assert!(s.handle_non_char_key("enter", false, cx));
            assert_eq!(
                s.stage.read(cx).search_match_cursor,
                cur_before,
                "C1-I7-1: StageSearch must not step match on IME commit Enter"
            );
            assert!(!s.ime.just_committed_ime);
            s.stage.update(cx, |st, stcx| {
                st.search_open = false;
                stcx.notify();
            });

            // (g) C1-I7-4: Empty `commit_ime_unmark_text()` leaves `just_committed_ime == false`
            let msgs_before = s.stage.read(cx).messages.len();
            s.composer.update(cx, |c, ccx| {
                c.set_text("SEND_IMMEDIATELY".to_string());
                c.is_input_focused = true;
                ccx.notify();
            });
            s.commit_ime_unmark_text(cx);
            assert!(
                !s.ime.just_committed_ime,
                "C1-I7-4: empty unmark_text must NOT set just_committed_ime"
            );
            assert!(s.handle_non_char_key("enter", false, cx));
            assert_eq!(
                s.stage.read(cx).messages.len(),
                msgs_before + 1,
                "C1-I7-4: Composer Enter must send immediately after empty unmark_text"
            );

            s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
            s.content_hub.update(cx, |h, _| {
                h.publish_modal.focused_field = Some(PublishModalField::Summary);
                h.publish_modal.summary.set_text("Line1");
            });
            s.commit_ime_unmark_text(cx);
            assert!(!s.ime.just_committed_ime);
            assert!(s.handle_non_char_key("enter", false, cx));
            assert_eq!(
                s.content_hub.read(cx).publish_modal.summary.text,
                "Line1\n",
                "C1-I7-4: PublishReviewModal Enter must insert newline after empty unmark_text"
            );
            s.close_publish_review_modal(cx);

            // (h) C1-I7-4 + C1-I7-1: Composer `commit_ime_unmark_text()` commits text, calls `sync_autocomplete()`,
            // AND trailing 1st Enter is absorbed by `just_committed_ime` BEFORE slash AC executes!
            s.composer.update(cx, |c, ccx| {
                c.hermes_slash_enabled = true;
                c.bot_slash_commands.clear();
                c.set_text(String::new());
                c.is_input_focused = true;
                c.hermes_ac_visible = false;
                c.ime_machine.on_ime_update("/help", 5);
                ccx.notify();
            });
            s.commit_ime_unmark_text(cx);
            assert_eq!(s.composer.read(cx).text(), "/help");
            assert!(
                s.composer.read(cx).hermes_ac_visible,
                "C1-I7-4: Composer unmark_text must call sync_autocomplete()"
            );
            assert!(s.ime.just_committed_ime);
            assert!(s.handle_non_char_key("enter", false, cx));
            assert!(!s.ime.just_committed_ime);
            assert!(
                s.composer.read(cx).hermes_ac_visible,
                "C1-I7-1: trailing IME Enter must be absorbed before slash AC consumes it"
            );
            s.composer.update(cx, |c, ccx| {
                c.clear();
                ccx.notify();
            });
    }).unwrap();

    // === 12. C1-I7-2: `logout()` and Direct `is_user_switch` Zeroize Secrets & Drop All Lazy Settings Sections ===
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            let (sp_alice, ss_alice, ap_alice, sc_alice) = s.settings_router.update(cx, |r, rcx| {
                let sp = r.ensure_privacy(rcx);
                sp.update(rcx, |sec, _| {
                    sec.two_factor_editing = true;
                    sec.two_factor_password_draft = "ALICE_2FA_SECRET_999".to_string();
                });
                let ss = r.ensure_search(rcx);
                ss.update(rcx, |search, _| search.query = "ALICE_SEARCH_SECRET".to_string());
                let ap = r.ensure_ai_providers(rcx);
                ap.update(rcx, |sec, _| sec.draft_api_key = "sk-alice-secret".to_string());
                let sc = r.ensure_computers(rcx);
                sc.update(rcx, |sec, _| sec.form_credential = "ssh-alice-secret".to_string());
                (sp, ss, ap, sc)
            });
            s.modal_container.update(cx, |mc, _| {
                mc.dialog.primary_input = "ALICE_MODAL_PIN".to_string();
            });
            s.logout(cx);
            assert!(sp_alice.read(cx).two_factor_password_draft.is_empty());
            assert!(!sp_alice.read(cx).two_factor_editing);
            assert!(ss_alice.read(cx).query.is_empty());
            assert!(ap_alice.read(cx).draft_api_key.is_empty());
            assert!(sc_alice.read(cx).form_credential.is_empty());
            assert!(s.modal_container.read(cx).dialog.primary_input.is_empty());
            s.apply_authenticated_session(sample_auth(1002, "bob"), cx);
            let r = s.settings_router.read(cx);
            assert!(r.settings_privacy.is_none());
            assert!(r.settings_search.is_none());
            assert!(r.settings_ai_providers.is_none());
            assert!(r.settings_computers.is_none());

            // Now test direct `is_user_switch` (Bob -> Carol without logout!)
            let (sp_bob, ss_bob) = s.settings_router.update(cx, |r, rcx| {
                let sp = r.ensure_privacy(rcx);
                sp.update(rcx, |sec, _| {
                    sec.two_factor_editing = true;
                    sec.two_factor_password_draft = "BOB_2FA_SECRET_777".to_string();
                });
                let ss = r.ensure_search(rcx);
                ss.update(rcx, |search, _| search.query = "BOB_SEARCH_SECRET".to_string());
                (sp, ss)
            });
            s.apply_authenticated_session(sample_auth(1003, "carol"), cx);
            assert!(
                sp_bob.read(cx).two_factor_password_draft.is_empty()
                    && !sp_bob.read(cx).two_factor_editing,
                "C1-I7-2: direct account switch must wipe two_factor_password_draft"
            );
            assert!(
                ss_bob.read(cx).query.is_empty(),
                "C1-I7-2: direct account switch must wipe SettingsSearch query"
            );
            assert!(s.settings_router.read(cx).settings_privacy.is_none());
            assert!(s.settings_router.read(cx).settings_search.is_none());

            s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
            s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
        });
    })
    .unwrap();

    // === 13. C1-I7-6 + C1-I7-7: Classic Theme Isolation, CreationModal Field Switch IME, Unfocused/Non-Input Phantom IME & Hidden Inspector ===
    // (a) C1-I7-6: Classic theme Cmd+N, Cmd+Shift+N & OpenAddBotToGroup never trap `is_compose_isolated == true && !is_compose_or_creation_modal_active`
    shell.update(&mut app, |s, cx| {
        s.apply_theme_selection(TdesktopThemeMode::Classic, None, None, cx);
        assert!(!s.theme_state.palette.is_apple_style());
        s.composer.update(cx, |c, _| c.set_text("CLASSIC_DRAFT".to_string()));
        assert!(s.handle_shortcut_key("n", false, cx));
        assert!(
            !s.composer.read(cx).is_compose_isolated || s.is_compose_or_creation_modal_active(cx),
            "C1-I7-6: Classic Cmd+N must NEVER trap is_compose_isolated=true without active modal"
        );
        assert!(s.handle_shortcut_key("n", true, cx));
        assert!(s.modal_container.read(cx).is_modal_active());
        s.modal_container.update(cx, |mc, mcx| {
            mc.hide_all();
            mcx.notify();
        });
        s.sidebar.update(cx, |sb, scx| {
            sb.close_creation_modal();
            sb.close_contacts_modal();
            scx.notify();
        });

        s.handle_inspector_action(InspectorAction::OpenAddBotToGroup(777_020), cx);
        assert!(
            !s.composer.read(cx).is_compose_isolated || s.is_compose_or_creation_modal_active(cx),
            "C1-I7-6: Classic OpenAddBotToGroup must NEVER trap is_compose_isolated=true without active modal"
        );
        s.modal_container.update(cx, |mc, mcx| {
            mc.hide_all();
            mcx.notify();
        });
        s.sidebar.update(cx, |sb, scx| {
            sb.close_creation_modal();
            sb.close_contacts_modal();
            scx.notify();
        });
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);

        // (b) C1-I7-7a: CreationModal field switch (PrimaryName -> SecondaryHandle) clears IME preedit
        s.sidebar.update(cx, |sb, scx| {
            sb.open_create_group_modal();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("name_field_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("name_field_pre"));
        s.sidebar.update(cx, |sb, scx| {
            sb.active_creation_field = CreationInputField::SecondaryHandle;
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I7-7a: CreationModal field switch must clear IME preedit"
        );
        s.sidebar.update(cx, |sb, scx| {
            sb.close_creation_modal();
            scx.notify();
        });

        // (c) C1-I7-7b: Unfocused PublishReviewModal & non-input ModalContainer (AboutTelegram) store ZERO phantom IME and close on 1st Escape!
        s.open_publish_review_modal_for_creation(clean_book.clone(), cx);
        s.content_hub.update(cx, |h, _| h.publish_modal.focused_field = None);
        s.update_ime_marked_text("ghost_pub_none", cx);
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I7-7b: unfocused PublishReviewModal must store zero phantom IME"
        );
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.is_publish_review_modal_active(cx),
            "C1-I7-7b: unfocused PublishReviewModal must close on 1st Escape"
        );

        s.open_modal_dialog("About", ModalDialogKind::AboutTelegram, 380.0, 260.0, cx);
        s.update_ime_marked_text("about_ghost", cx);
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I7-7b: non-input ModalContainer must store zero phantom IME"
        );
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !s.modal_container.read(cx).is_modal_active(),
            "C1-I7-7b: non-input ModalContainer must close on 1st Escape"
        );

        // (d) C1-I7-7c: Escape on ContentHub with no open hub modal/drawer/input MUST NOT mutate hidden Chats ContextInspector!
        s.set_inspector_open(true, cx);
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub.update(cx, |h, _| h.blur_input());
        assert!(
            !s.handle_non_char_key("escape", false, cx),
            "C1-I7-7c: Escape on ContentHub must return false"
        );
        assert!(
            s.inspector.read(cx).is_open,
            "C1-I7-7c: hidden Chats ContextInspector must remain untouched"
        );
    }).unwrap();
}

#[test]
fn test_m5_gpui_iter9_reg_completeness_closure() {
    run_in_headless_subprocess("test_m5_gpui_iter9_reg_completeness_closure", true);
}

async fn body_iter9_reg_completeness_closure(mut app: AsyncApp) {
    let e2e_pending_book = sample_book_creation(
        "cr_iter9_reject_e2e",
        1001,
        "Iter9 Reject E2E Book",
        "A completely clean book summary exceeding ten visible characters.",
        "Chapter one body has plenty of readable text exceeding fifty visible characters comfortably.",
        CreationReviewStatus::PendingReview,
        1,
    );
    let mock_state = Arc::new(MockM5State::new(
        true,
        vec![e2e_pending_book.clone()],
        vec![e2e_pending_book.clone()],
    ));
    let temp_client = TelegramClient::new(ClientConfig::default());
    let base_url = spawn_mock_m5_http_server(temp_client.runtime().handle(), mock_state.clone());
    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url.clone();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .expect("create shell");

    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
            s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
            s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
        });
    })
    .unwrap();

    // =========================================================================
    // §1 — C1-I8-1: All 7+ Multi-Field Mouse-Click Cross-Field IME Isolation & Z-Occlusion
    // =========================================================================

    // (1a) BotAssistCard: Username -> Password clears shell.ime & bot_assist_ime_preedit
    shell.update(&mut app, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.active_bot_card_field = Some(("card_i9".to_string(), BotAssistLoginField::Username));
            stcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("alice_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("alice_pre"));
        assert_eq!(s.stage.read(cx).bot_assist_ime_preedit.as_deref(), Some("alice_pre"));
        s.stage.update(cx, |st, stcx| {
            st.active_bot_card_field = Some(("card_i9".to_string(), BotAssistLoginField::Password));
            st.bot_assist_ime_preedit = None;
            stcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1a): BotAssistCard Username -> Password switch must clear shell.ime"
        );
        assert!(s.stage.read(cx).bot_assist_ime_preedit.is_none());
        s.commit_ime_unmark_text(cx);
        assert!(
            s.stage.read(cx).active_bot_card_text().is_empty(),
            "C1-I8-1 (1a): zero preedit may leak into Password field on commit"
        );
        s.stage.update(cx, |st, stcx| {
            st.blur_bot_card_input();
            stcx.notify();
        });
    }).unwrap();

    // (1b) ContextInspector: Persona -> Model clears shell.ime & model_input preedit
    shell.update(&mut app, |s, cx| {
        s.set_inspector_open(true, cx);
        s.inspector.update(cx, |ins, icx| {
            ins.active_tab = InspectorTab::Profile;
            ins.profile.is_bot = true;
            ins.profile.bot_id = 777_001;
            ins.subpage = InspectorSubpage::BotConfigEditor;
            ins.bot_config_focus = BotConfigFocusField::Persona;
            ins.bot_draft_model.clear();
            icx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("persona_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("persona_pre"));
        s.inspector.update(cx, |ins, icx| {
            ins.bot_config_focus = BotConfigFocusField::Model;
            icx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1b): ContextInspector Persona -> Model must clear shell.ime"
        );
        s.commit_ime_unmark_text(cx);
        assert!(
            s.inspector.read(cx).bot_draft_model.is_empty(),
            "C1-I8-1 (1b): zero preedit may leak into bot_draft_model"
        );
        // Also test switching to add_member_search_focused
        s.update_ime_marked_text("model_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("model_pre"));
        s.inspector.update(cx, |ins, icx| {
            ins.active_tab = InspectorTab::Members;
            ins.add_member_picker_open = true;
            ins.add_member_search_focused = true;
            ins.add_member_query.clear();
            icx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1b): ContextInspector Model -> AddMemberSearch must clear shell.ime"
        );
        s.inspector.update(cx, |ins, icx| {
            ins.blur_bot_input();
            ins.add_member_picker_open = false;
            icx.notify();
        });
        s.set_inspector_open(false, cx);
    }).unwrap();

    // (1c) SettingsInformation: FirstName -> Bio clears shell.ime
    shell.update(&mut app, |s, cx| {
        s.navigate_settings_to(SettingsSectionType::Information, cx);
        let si = s.settings_router.update(cx, |r, rcx| r.ensure_information(rcx));
        si.update(cx, |sec, scx| {
            sec.active_field = ProfileEditField::FirstName;
            sec.bio.clear();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("fname_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("fname_pre"));
        let si = s.settings_router.update(cx, |r, rcx| r.ensure_information(rcx));
        si.update(cx, |sec, scx| {
            sec.active_field = ProfileEditField::Bio;
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1c): SettingsInformation FirstName -> Bio must clear shell.ime"
        );
        s.commit_ime_unmark_text(cx);
        let si = s.settings_router.read(cx).settings_information.as_ref().unwrap().read(cx);
        assert!(si.bio.is_empty(), "C1-I8-1 (1c): zero preedit may leak into bio");
    }).unwrap();

    // (1d) SettingsPrivacySecurity: TwoFactorPassword -> RecoveryKey clears shell.ime
    shell.update(&mut app, |s, cx| {
        s.navigate_settings_to(SettingsSectionType::PrivacySecurity, cx);
        let sp = s.settings_router.update(cx, |r, rcx| r.ensure_privacy(rcx));
        sp.update(cx, |sec, scx| {
            sec.two_factor_editing = true;
            sec.recovery_key_editing = false;
            sec.recovery_key_draft = zeroize::Zeroizing::new(String::new());
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("2fa_pw_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("2fa_pw_pre"));
        let sp = s.settings_router.update(cx, |r, rcx| r.ensure_privacy(rcx));
        sp.update(cx, |sec, scx| {
            sec.two_factor_editing = false;
            sec.recovery_key_editing = true;
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1d): SettingsPrivacySecurity 2FA -> RecoveryKey must clear shell.ime"
        );
        s.commit_ime_unmark_text(cx);
        let sp = s.settings_router.read(cx).settings_privacy.as_ref().unwrap().read(cx);
        assert!(
            sp.recovery_key_draft.is_empty(),
            "C1-I8-1 (1d): zero preedit may leak into recovery_key_draft"
        );
    }).unwrap();

    // (1e) SettingsAiProviders: BaseUrl -> ApiKey clears shell.ime
    shell.update(&mut app, |s, cx| {
        s.navigate_settings_to(SettingsSectionType::AiProviders, cx);
        let ap = s.settings_router.update(cx, |r, rcx| r.ensure_ai_providers(rcx));
        ap.update(cx, |sec, scx| {
            sec.editor_open = true;
            sec.active_field = AiProviderField::BaseUrl;
            sec.draft_api_key.clear();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("url_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("url_pre"));
        let ap = s.settings_router.update(cx, |r, rcx| r.ensure_ai_providers(rcx));
        ap.update(cx, |sec, scx| {
            sec.active_field = AiProviderField::ApiKey;
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1e): SettingsAiProviders BaseUrl -> ApiKey must clear shell.ime"
        );
        s.commit_ime_unmark_text(cx);
        let ap = s.settings_router.read(cx).settings_ai_providers.as_ref().unwrap().read(cx);
        assert!(ap.draft_api_key.is_empty(), "C1-I8-1 (1e): zero preedit may leak into draft_api_key");
    }).unwrap();

    // (1f) SettingsComputers: CloudForm Host -> Credential AND AssistInputField switch + Z-occlusion
    shell.update(&mut app, |s, cx| {
        s.navigate_settings_to(SettingsSectionType::Computers, cx);
        let sc = s.settings_router.update(cx, |r, rcx| r.ensure_computers(rcx));
        sc.update(cx, |sec, scx| {
            sec.page = SettingsComputersPage::CloudForm;
            sec.form_focus = CloudFormField::Host;
            sec.form_credential.clear();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("host_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("host_pre"));
        let sc = s.settings_router.update(cx, |r, rcx| r.ensure_computers(rcx));
        sc.update(cx, |sec, scx| {
            sec.form_focus = CloudFormField::Credential;
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1f): SettingsComputers Host -> Credential must clear shell.ime"
        );
        s.commit_ime_unmark_text(cx);
        let sc = s.settings_router.read(cx).settings_computers.as_ref().unwrap().read(cx);
        assert!(
            sc.form_credential.is_empty(),
            "C1-I8-1 (1f): zero preedit may leak into form_credential"
        );
    }).unwrap();

    // Also test AssistInputField pair & Z-occlusion by ModalContainer (Z #13) over Settings (Z #9)
    shell.update(&mut app, |s, cx| {
        let sc = s.settings_router.update(cx, |r, rcx| r.ensure_computers(rcx));
        sc.update(cx, |sec, scx| {
            sec.page = SettingsComputersPage::Overview;
            sec.overview_tab = SettingsComputersTab::RemoteAssist;
            sec.assist_focus = AssistInputField::TargetAssistId;
            sec.assist_target_code_draft.clear();
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("assist_id_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("assist_id_pre"));
        let sc = s.settings_router.update(cx, |r, rcx| r.ensure_computers(rcx));
        sc.update(cx, |sec, scx| {
            sec.assist_focus = AssistInputField::TargetAccessCode;
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1f): SettingsComputers AssistInputField switch must clear shell.ime"
        );
        // Z-occlusion: open Z #13 ModalContainer above Settings, set IME, mutate background Settings field -> MUST preserve ModalContainer IME!
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("mc_keep", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("mc_keep"));
        let sc = s.settings_router.update(cx, |r, rcx| r.ensure_computers(rcx));
        sc.update(cx, |sec, scx| {
            sec.assist_focus = AssistInputField::CustomCode;
            scx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("mc_keep"),
            "C1-I8-1 (1f Z-occlusion): background Settings field switch MUST preserve Z #13 ModalContainer IME"
        );
        s.modal_container.update(cx, |mc, mcx| {
            mc.dismiss_top();
            mcx.notify();
        });
        s.close_settings(cx);
    }).unwrap();

    // (1g) GlobalTasksWorkspace (StudioInput): NewTask -> KanbanBoardName + set_tab + Z-occlusion
    shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::TaskCenter, cx);
        s.tasks_workspace.update(cx, |tw, twcx| {
            tw.active_input = StudioInputTarget::NewTask;
            tw.kanban_board_draft.clear();
            twcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("task_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("task_pre"));
        s.tasks_workspace.update(cx, |tw, twcx| {
            tw.active_input = StudioInputTarget::KanbanBoardName;
            twcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1g): GlobalTasksWorkspace field switch must clear shell.ime"
        );
        s.commit_ime_unmark_text(cx);
        assert!(
            s.tasks_workspace.read(cx).kanban_board_draft.is_empty(),
            "C1-I8-1 (1g): zero preedit may leak into kanban_board_draft"
        );
        // set_tab clears active_input and clears IME
        s.update_ime_marked_text("tab_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("tab_pre"));
        s.tasks_workspace.update(cx, |tw, twcx| {
            tw.set_tab(TasksWorkspaceTab::Skills);
            twcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(s.tasks_workspace.read(cx).active_input, StudioInputTarget::None);
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-1 (1g): GlobalTasksWorkspace set_tab must clear active_input and shell.ime"
        );
        // Z-occlusion: ModalContainer over TaskCenter preserves ModalContainer IME
        s.tasks_workspace.update(cx, |tw, twcx| {
            tw.active_input = StudioInputTarget::SkillSearch;
            twcx.notify();
        });
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("mc_over_studio", cx);
        s.tasks_workspace.update(cx, |tw, twcx| {
            tw.active_input = StudioInputTarget::SkillEditorName;
            twcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("mc_over_studio"),
            "C1-I8-1 (1g Z-occlusion): background Studio field switch must preserve ModalContainer IME"
        );
        s.modal_container.update(cx, |mc, mcx| {
            mc.dismiss_top();
            mcx.notify();
        });
        s.activate_nav_destination(NavDestination::Chats, cx);
    }).unwrap();

    // =========================================================================
    // §2 — C1-I8-2: Composer Slash/Hermes Autocomplete vs Active IME Composition
    // =========================================================================
    shell.update(&mut app, |s, cx| {
        s.composer.update(cx, |c, ccx| {
            c.clear();
            c.is_input_focused = true;
            c.set_text("/".to_string());
            c.bot_ac_visible = true;
            c.bot_ac_matches = vec![
                BotSlashCommandItem {
                    command: "help".to_string(),
                    description: "Show help".to_string(),
                    bot_username: "CreatorStudioBot".to_string(),
                    avatar_initials: "CS".to_string(),
                    avatar_color: 0x007AFF,
                },
                BotSlashCommandItem {
                    command: "newbook".to_string(),
                    description: "Create book".to_string(),
                    bot_username: "CreatorStudioBot".to_string(),
                    avatar_initials: "CS".to_string(),
                    avatar_color: 0x34C759,
                },
            ];
            c.bot_ac_selected = 0;
            ccx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("he", cx);
        assert!(s.composer.read(cx).ime_machine.is_composing());
        // Up / Down during active composition must NOT hijack candidate nav or mutate bot_ac_selected
        assert!(!s.handle_non_char_key("down", false, cx));
        assert!(!s.handle_non_char_key("up", false, cx));
        assert_eq!(s.composer.read(cx).bot_ac_selected, 0);
        assert_eq!(s.composer.read(cx).text(), "/");
        assert!(s.composer.read(cx).ime_machine.is_composing());

        // Enter during active composition commits preedit ("/he"), exits composing, does NOT overwrite to "/help "!
        assert!(s.handle_non_char_key("enter", false, cx));
        assert!(
            !s.composer.read(cx).ime_machine.is_composing(),
            "C1-I8-2: Enter must exit composing with no dangling preedit"
        );
        assert_ne!(
            s.composer.read(cx).text(),
            "/help ",
            "C1-I8-2: Enter during active IME composition must NOT hijack into bot autocomplete accept"
        );
        assert_eq!(s.composer.read(cx).text(), "/he");

        // Tab during active composition cancels IME cleanly without corrupting draft into "/help "
        s.composer.update(cx, |c, ccx| {
            c.is_input_focused = true;
            c.set_text("/".to_string());
            c.bot_ac_visible = true;
            c.bot_ac_matches = vec![BotSlashCommandItem {
                command: "help".to_string(),
                description: "Show help".to_string(),
                bot_username: "CreatorStudioBot".to_string(),
                avatar_initials: "CS".to_string(),
                avatar_color: 0x007AFF,
            }];
            c.bot_ac_selected = 0;
            ccx.notify();
        });
        s.update_ime_marked_text("he", cx);
        assert!(s.composer.read(cx).ime_machine.is_composing());
        assert!(s.handle_non_char_key("tab", false, cx));
        assert!(!s.composer.read(cx).ime_machine.is_composing());
        assert_eq!(s.composer.read(cx).text(), "/");
        s.sidebar.update(cx, |sb, scx| {
            sb.search_focused = false;
            scx.notify();
        });
        s.composer.update(cx, |c, ccx| {
            c.clear();
            c.bot_ac_visible = false;
            c.bot_ac_matches.clear();
            ccx.notify();
        });
    }).unwrap();

    // =========================================================================
    // §3 — C1-I8-3: NavRail Cross-Account Metadata Reset on logout(), Direct Switch, & ServerUrlApplied
    // =========================================================================
    let seed_nav_rail_dirty = |s: &mut AppShell, cx: &mut gpui::Context<AppShell>| {
        s.nav_rail.update(cx, |r, rcx| {
            if let Some(first) = r.items.first_mut() {
                first.unread_badge = 42;
            }
            r.archive_unread_count = 7;
            r.show_archive_in_rail = true;
            r.is_connected = true;
            r.set_folders(vec![NavFolderItem {
                folder_id: 99,
                title: "VIP".to_string(),
                icon_hint: "work".to_string(),
                unread_badge: 5,
                is_muted: false,
            }]);
            r.active_folder_id = Some(99);
            r.set_active_destination(NavDestination::Settings);
            rcx.notify();
        });
    };
    let assert_nav_rail_clean = |s: &AppShell, cx: &App, label: &str| {
        let r = s.nav_rail.read(cx);
        assert!(
            r.items.iter().all(|it| it.unread_badge == 0),
            "C1-I8-3 ({label}): all NavRail item unread_badges must be 0"
        );
        assert_eq!(r.archive_unread_count, 0, "C1-I8-3 ({label}): archive_unread_count must be 0");
        assert!(!r.show_archive_in_rail, "C1-I8-3 ({label}): show_archive_in_rail must be false");
        assert!(r.custom_folders.is_empty(), "C1-I8-3 ({label}): custom_folders must be empty");
        assert!(r.active_folder_id.is_none(), "C1-I8-3 ({label}): active_folder_id must be None");
        assert_eq!(r.active_destination, NavDestination::Chats, "C1-I8-3 ({label}): dest must be Chats");
        assert!(!r.is_connected, "C1-I8-3 ({label}): is_connected must be false");
    };

    shell.update(&mut app, |s, cx| {
        // (3a) logout()
        seed_nav_rail_dirty(s, cx);
        s.logout(cx);
        assert_nav_rail_clean(s, cx, "logout");

        // (3b) direct account switch Alice -> Bob WITHOUT logout()
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        seed_nav_rail_dirty(s, cx);
        s.apply_authenticated_session(sample_auth(1002, "bob"), cx);
        assert_nav_rail_clean(s, cx, "direct_user_switch");

        // (3c) AuthWizardEvent::ServerUrlApplied
        seed_nav_rail_dirty(s, cx);
        s.handle_auth_wizard_event(&AuthWizardEvent::ServerUrlApplied(base_url.clone()), cx);
        assert_nav_rail_clean(s, cx, "ServerUrlApplied");

        // Restore Alice
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
        s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
    }).unwrap();

    // =========================================================================
    // §4 — C1-I8-4: TextInputController & AuthWizardView Timestamped Commit Fence
    // =========================================================================
    {
        let mut ctrl = TextInputController::new();
        assert_eq!(ctrl.handle_replace_and_mark("nihao"), None);
        assert!(ctrl.has_preedit());
        assert!(!ctrl.just_committed_ime && ctrl.last_ime_commit_at.is_none());
        assert_eq!(ctrl.handle_replace_and_mark(""), Some("nihao".to_string()));
        assert!(
            ctrl.just_committed_ime && ctrl.last_ime_commit_at.is_some(),
            "C1-I8-4: handle_replace_and_mark commit MUST stamp last_ime_commit_at"
        );
        assert!(ctrl.take_just_committed(), "C1-I8-4: immediate trailing Enter absorbed");
        // Verify expiry > 80ms does NOT permanently swallow Enter
        assert_eq!(ctrl.handle_replace_and_mark("ok"), None);
        assert_eq!(ctrl.handle_replace_and_mark(""), Some("ok".to_string()));
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            !ctrl.take_just_committed(),
            "C1-I8-4: expired commit fence (>80ms) must NOT permanently swallow Enter"
        );
    }

    shell.update(&mut app, |s, cx| {
        s.logout(cx);
        s.auth_wizard.update(cx, |w, wcx| {
            w.username_input = "alice".to_string();
            w.password_input = "sec".to_string();
            w.active_login_field = LoginInputField::Password;
            wcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("pw", cx);
        assert!(s.auth_wizard.read(cx).ime.has_preedit());
        s.commit_ime_unmark_text(cx);
        {
            let w = s.auth_wizard.read(cx);
            assert!(w.ime.just_committed_ime && w.ime.last_ime_commit_at.is_some());
            assert_eq!(w.password_input, "secpw");
        }
        // Immediate trailing Enter (<80ms) is absorbed -> !is_authenticating
        assert!(s.handle_non_char_key("enter", false, cx));
        assert!(
            !s.auth_wizard.read(cx).is_authenticating,
            "C1-I8-4: immediate trailing Enter within 80ms must be absorbed"
        );

        // Stamp commit fence, wait 120ms (> 80ms), then press Enter -> login submit fires!
        s.auth_wizard.update(cx, |w, _| {
            w.ime.mark_ime_committed();
        });
    }).unwrap();
    app.background_executor().timer(Duration::from_millis(120)).await;
    shell.update(&mut app, |s, cx| {
        assert!(s.handle_non_char_key("enter", false, cx));
        assert!(
            s.auth_wizard.read(cx).is_authenticating,
            "C1-I8-4: intentional Enter after >80ms must fire submit_login (no infinite swallow)"
        );

        // Tab clears both just_committed_ime and last_ime_commit_at
        s.auth_wizard.update(cx, |w, _| {
            w.is_authenticating = false;
            w.ime.ime_preedit = Some("tab_pre".to_string());
            w.ime.mark_ime_committed();
        });
        assert!(s.handle_non_char_key("tab", false, cx));
        {
            let w = s.auth_wizard.read(cx);
            assert!(w.ime.ime_preedit.is_none());
            assert!(!w.ime.just_committed_ime);
            assert!(w.ime.last_ime_commit_at.is_none());
        }

        // Re-authenticate Alice for §§5–7
        s.apply_authenticated_session(sample_auth(1001, "alice"), cx);
        s.apply_theme_selection(TdesktopThemeMode::Day, None, None, cx);
        s.open_or_select_chat(5001, Some("General".to_string()), false, cx);
    }).unwrap();

    // =========================================================================
    // §5 — C1-I8-5 + Exp3-I9-2: Settings Cleanup & Mic Test Stop on All Exit Paths
    // =========================================================================
    let enter_settings_calls_with_mic = |s: &mut AppShell, cx: &mut gpui::Context<AppShell>| {
        s.navigate_settings_to(SettingsSectionType::Calls, cx);
        let calls = s.settings_router.update(cx, |r, rcx| r.ensure_calls(rcx));
        calls.update(cx, |sec, scx| {
            sec.mic_test_active = true;
            scx.notify();
        });
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert_eq!(s.current_settings_section(cx), SettingsSectionType::Calls);
        assert!(calls.read(cx).mic_test_active);
        calls
    };

    shell.update(&mut app, |s, cx| {
        // (5a) Cmd+N
        let calls = enter_settings_calls_with_mic(s, cx);
        assert!(s.handle_shortcut_key("n", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.current_settings_section(cx), SettingsSectionType::Main);
        assert!(s.settings_router.read(cx).settings_stack.is_empty());
        assert!(!calls.read(cx).mic_test_active, "C1-I8-5 (Cmd+N): mic_test_active must stop");
        s.sidebar.update(cx, |sb, scx| { sb.exit_compose_mode(); scx.notify(); });

        // (5b) open_modal_dialog(NewGroup) in Apple mode
        let calls = enter_settings_calls_with_mic(s, cx);
        s.open_modal_dialog("New Group", ModalDialogKind::NewGroup, 420.0, 380.0, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.current_settings_section(cx), SettingsSectionType::Main);
        assert!(!calls.read(cx).mic_test_active, "C1-I8-5 (Apple NewGroup modal): mic_test_active must stop");
        s.sidebar.update(cx, |sb, scx| { sb.exit_compose_mode(); scx.notify(); });

        // (5c) MainMenuCommand::NewGroup in Apple mode
        let calls = enter_settings_calls_with_mic(s, cx);
        s.handle_main_menu_command(MainMenuCommand::NewGroup, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.current_settings_section(cx), SettingsSectionType::Main);
        assert!(!calls.read(cx).mic_test_active, "C1-I8-5 (MainMenu NewGroup): mic_test_active must stop");
        s.sidebar.update(cx, |sb, scx| { sb.exit_compose_mode(); scx.notify(); });

        // (5d) Cmd+1 (Chats) & Cmd+3 (Calls) via activate_nav_destination (Exp3-I9-2)
        let calls = enter_settings_calls_with_mic(s, cx);
        assert!(s.handle_shortcut_key("1", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.current_settings_section(cx), SettingsSectionType::Main);
        assert!(!calls.read(cx).mic_test_active, "Exp3-I9-2 (Cmd+1): mic_test_active must stop");

        let calls = enter_settings_calls_with_mic(s, cx);
        assert!(s.handle_shortcut_key("3", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Calls);
        assert_eq!(s.current_settings_section(cx), SettingsSectionType::Main);
        assert!(!calls.read(cx).mic_test_active, "Exp3-I9-2 (Cmd+3): mic_test_active must stop");

        s.activate_nav_destination(NavDestination::Chats, cx);
    }).unwrap();

    // =========================================================================
    // §6 — C1-I8-6a + Exp3-I9-1 + C1-I8-6b: Non-Chats Shortcuts & ClosePublishReviewModal Matrix
    // =========================================================================
    shell.update(&mut app, |s, cx| {
        s.set_inspector_open(false, cx);
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(false);
            scx.notify();
        });
        s.preferences.appearance.sidebar_collapsed = false;

        // (6a) Cmd+I and Cmd+B / Cmd+\ on ContentHub(Books) and Settings must NOT mutate hidden inspector/sidebar
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        assert!(s.handle_shortcut_key("i", false, cx));
        assert!(!s.inspector.read(cx).is_open, "C1-I8-6a: Cmd+I on ContentHub must not toggle inspector");
        assert!(s.handle_shortcut_key("b", false, cx));
        assert!(s.handle_shortcut_key("\\", false, cx));
        assert!(!s.sidebar.read(cx).is_collapsed, "Exp3-I9-1: Cmd+B/\\ on ContentHub must not collapse sidebar");

        s.navigate_settings_to(SettingsSectionType::Main, cx);
        assert!(s.handle_shortcut_key("i", false, cx));
        assert!(!s.inspector.read(cx).is_open, "C1-I8-6a: Cmd+I on Settings must not toggle inspector");
        assert!(s.handle_shortcut_key("b", false, cx));
        assert!(!s.sidebar.read(cx).is_collapsed, "Exp3-I9-1: Cmd+B on Settings must not collapse sidebar");
        s.close_settings(cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(!s.preferences.appearance.sidebar_collapsed, "Exp3-I9-1: hidden Cmd+B must not persist preference");
        // Back on Chats: Cmd+I and Cmd+B work normally!
        assert!(s.handle_shortcut_key("i", false, cx));
        assert!(s.inspector.read(cx).is_open);
        assert!(s.handle_shortcut_key("i", false, cx));
        assert!(!s.inspector.read(cx).is_open);
        assert!(s.handle_shortcut_key("b", false, cx));
        assert!(s.sidebar.read(cx).is_collapsed);
        assert!(s.handle_shortcut_key("b", false, cx));
        assert!(!s.sidebar.read(cx).is_collapsed);
    }).unwrap();

    // (6b-i) Stale ClosePublishReviewModal when already closed preserves active Z #13 ModalContainer IME
    shell.update(&mut app, |s, cx| {
        assert!(!s.is_publish_review_modal_active(cx));
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("mc_live_ime", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("mc_live_ime"));
        s.content_hub.update(cx, |_, hcx| {
            hcx.emit(ContentHubEvent::ClosePublishReviewModal);
            hcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("mc_live_ime"),
            "C1-I8-6b (6b-i): stale ClosePublishReviewModal must preserve Z #13 ModalContainer IME"
        );
        s.modal_container.update(cx, |mc, mcx| {
            mc.dismiss_top();
            mcx.notify();
        });
    }).unwrap();

    // (6b-ii) Legitimate X-button close() + emit ClosePublishReviewModal DOES clear shell.ime
    shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(e2e_pending_book.clone(), cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("pub_x_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("pub_x_pre"));
        s.content_hub.update(cx, |h, hcx| {
            h.publish_modal.close();
            hcx.emit(ContentHubEvent::ClosePublishReviewModal);
            hcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert!(!s.is_publish_review_modal_active(cx));
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I8-6b (6b-ii): legitimate X-button close must clear shell.ime"
        );
    }).unwrap();

    // (6b-iii) Occluded Z #15 X-button close preserves ComputerWorkbench IME & clears pending_close_ime_clear
    shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(e2e_pending_book.clone(), cx);
        s.show_remote_computer(cx);
        if let Some(cw) = s.overlays.computer_overlay.clone() {
            cw.update(cx, |w, wcx| {
                w.active_tab = WorkbenchTab::RemoteTerminal;
                w.terminal_focused = true;
                wcx.notify();
            });
        }
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("cw_z15_x", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("cw_z15_x"));
        s.content_hub.update(cx, |h, hcx| {
            h.publish_modal.close();
            hcx.emit(ContentHubEvent::ClosePublishReviewModal);
            hcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("cw_z15_x"),
            "C1-I8-6b (6b-iii): occluded X-button close must preserve Z #15 ComputerWorkbench IME"
        );
        assert!(
            !s.content_hub.read(cx).publish_modal.pending_close_ime_clear,
            "C1-I8-6b (6b-iii): occluded close must consume pending_close_ime_clear"
        );
        s.hide_remote_computer(cx);
    }).unwrap();

    // (6b-iv) Adversarial raw close() (no emit) -> ModalContainer IME -> flush + stale ClosePublishReviewModal
    shell.update(&mut app, |s, cx| {
        s.open_publish_review_modal_for_creation(e2e_pending_book.clone(), cx);
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.publish_modal.close();
            hcx.notify();
        });
        s.open_modal_dialog("Create Poll", ModalDialogKind::CreatePoll, 420.0, 420.0, cx);
        s.update_ime_marked_text("mc_after_raw", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("mc_after_raw"));
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("mc_after_raw"),
            "C1-I8-6b (6b-iv): observer flush after raw close + new lower-Z IME must preserve ModalContainer IME"
        );
        s.content_hub.update(cx, |_, hcx| {
            hcx.emit(ContentHubEvent::ClosePublishReviewModal);
            hcx.notify();
        });
    }).unwrap();
    shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("mc_after_raw"),
            "C1-I8-6b (6b-iv): stale ClosePublishReviewModal after raw close must preserve ModalContainer IME"
        );
        s.modal_container.update(cx, |mc, mcx| {
            mc.dismiss_top();
            mcx.notify();
        });
    }).unwrap();

    // =========================================================================
    // §7 — C2-I8-1: PublishReviewModalState::preview_dto() Unit + Live Mock-HTTP E2E Resubmit
    // =========================================================================
    // (7a) Unit/State verification
    let xss_feedback = "Please remove <script>, javascript:alert(1), and data:text/html from your summary";
    {
        let mut rejected_book = e2e_pending_book.clone();
        rejected_book.review_status = CreationReviewStatus::Rejected;
        rejected_book.rejection_reason = Some(xss_feedback.to_string());

        let mut modal = PublishReviewModalState::default();
        modal.open_for_creation(rejected_book);
        assert_eq!(
            modal
                .base_creation
                .as_ref()
                .and_then(|c| c.rejection_reason.as_deref()),
            Some(xss_feedback),
            "C2-I8-1 (7a): base_creation.rejection_reason must stay intact for banner display"
        );
        assert!(
            modal.preview_dto().and_then(|d| d.rejection_reason).is_none(),
            "C2-I8-1 (7a): preview_dto() must clear stale rejection_reason"
        );
        assert!(
            modal.validation_errors.is_empty(),
            "C2-I8-1 (7a): clean resubmission must NOT be blocked by stale rejection_reason: {:?}",
            modal.validation_errors
        );

        // Genuine XSS in editable summary MUST still be caught!
        modal.summary.set_text("Summary with <script>alert(1)</script> and data:text/html");
        modal.recompute_validation();
        assert!(
            !modal.validation_errors.is_empty(),
            "C2-I8-1 (7a): genuine XSS in editable summary must still be rejected"
        );
    }

    // (7b) Live Mock-HTTP Reviewer-Reject -> Creator-Open -> Clean 1-Click Resubmit E2E
    shell.update(&mut app, |s, cx| {
        s.refresh_my_creations(cx);
        s.probe_reviewer_queue(cx, true);
    }).unwrap();
    for _ in 0..40 {
        let ready = shell
            .read_with(&app, |s, cx| {
                let h = s.content_hub.read(cx);
                h.find_my_creation("cr_iter9_reject_e2e").is_some()
                    && !h.publish_modal.review_queue.is_empty()
            })
            .unwrap();
        if ready {
            break;
        }
        app.background_executor().timer(Duration::from_millis(25)).await;
    }

    shell.update(&mut app, |s, cx| {
        s.submit_review_decision_fenced(
            "cr_iter9_reject_e2e".to_string(),
            ReviewDecisionRequest {
                decision: ReviewDecision::Reject,
                visibility: None,
                reason: Some(xss_feedback.to_string()),
                notes: None,
            },
            cx,
        );
    }).unwrap();

    for _ in 0..40 {
        let rejected = shell
            .read_with(&app, |s, cx| {
                s.content_hub
                    .read(cx)
                    .find_my_creation("cr_iter9_reject_e2e")
                    .is_some_and(|c| c.review_status == CreationReviewStatus::Rejected)
            })
            .unwrap();
        if rejected {
            break;
        }
        app.background_executor().timer(Duration::from_millis(25)).await;
    }

    let (update_req, submit_req) = shell
        .update(&mut app, |s, cx| {
            let item = s
                .content_hub
                .read(cx)
                .find_my_creation("cr_iter9_reject_e2e")
                .cloned()
                .expect("rejected creation present");
            assert_eq!(item.review_status, CreationReviewStatus::Rejected);
            assert_eq!(item.rejection_reason.as_deref(), Some(xss_feedback));

            s.open_publish_review_modal_for_creation(item, cx);
            let h = s.content_hub.read(cx);
            assert_eq!(
                h.publish_modal
                    .base_creation
                    .as_ref()
                    .and_then(|c| c.rejection_reason.as_deref()),
                Some(xss_feedback),
                "C2-I8-1 (7b): modal base_creation must preserve rejection_reason banner before submit"
            );
            assert!(
                h.publish_modal.validation_errors.is_empty(),
                "C2-I8-1 (7b): rejected creation with XSS-quoting reviewer reason must have 0 validation_errors: {:?}",
                h.publish_modal.validation_errors
            );
            let upd = if h.publish_modal.has_metadata_changes() {
                h.publish_modal.build_update_request()
            } else {
                None
            };
            let sub = h.publish_modal.build_submit_request();
            (upd, sub)
        })
        .unwrap();

    shell.update(&mut app, |s, cx| {
        s.submit_creation_for_review_fenced(
            "cr_iter9_reject_e2e".to_string(),
            update_req,
            submit_req,
            cx,
        );
    }).unwrap();

    for _ in 0..40 {
        let resubmitted = shell
            .read_with(&app, |s, cx| {
                s.content_hub
                    .read(cx)
                    .find_my_creation("cr_iter9_reject_e2e")
                    .is_some_and(|c| c.review_status == CreationReviewStatus::PendingReview)
            })
            .unwrap();
        if resubmitted {
            break;
        }
        app.background_executor().timer(Duration::from_millis(25)).await;
    }

    shell.update(&mut app, |s, cx| {
        let h = s.content_hub.read(cx);
        let item = h
            .find_my_creation("cr_iter9_reject_e2e")
            .expect("resubmitted creation present");
        assert_eq!(item.review_status, CreationReviewStatus::PendingReview);
        assert!(
            item.rejection_reason.is_none(),
            "C2-I8-1 (7b): my_creations rejection_reason must be cleared after resubmission"
        );
        assert!(
            h.publish_modal
                .base_creation
                .as_ref()
                .and_then(|c| c.rejection_reason.as_ref())
                .is_none(),
            "C2-I8-1 (7b): publish_modal base_creation rejection_reason must be cleared after resubmission"
        );
    }).unwrap();
}

