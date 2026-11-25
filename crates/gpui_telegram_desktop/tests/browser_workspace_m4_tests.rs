//! M4B Headless GPUI Integration Tests (`BrowserWorkspace`, `AppShell`, `RecorderPanel`, `StorePanel`).
//!
//! Covers all 7 F7/F8 acceptance criteria using `Application::headless().with_assets(TelegramAssetSource::new())`
//! with macOS `__DATA,__mod_init_func` subprocess re-execution, `GPUI_OFFLINE_TEST=1`, and `tempfile::TempDir`.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io::Cursor;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bot_browser::types::BotBrowserError;
use bot_browser::{
    classify_probe_io_error, ActionOutcome, AutomationScriptApp, BrowserBackendKind,
    BrowserControlMode, BrowserCookie, BrowserPerformanceMetrics, BrowserProfileStore,
    BrowserSessionBackend, BrowserSnapshot, ChromeImportProbe, KeyInputEvent, NavigationResult,
    PointerButton, PointerEventKind, PointerInputEvent, ScriptStep, SemanticLocator,
};
use futures::FutureExt;
use gpui::{
    bounds, point, px, size, App, AppContext, Application, AsyncApp, Entity, Focusable, Keystroke,
    WindowBounds, WindowOptions,
};
use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
use gpui_telegram_client::net::http_client::{HttpClient, RegisterRequest};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_desktop::components::bots::bot_attach_web_view::BotWebViewPendingAction;
#[cfg(target_os = "macos")]
use gpui_telegram_desktop::components::bots::macos_wkwebview::parse_recorded_dom_event;
use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};
use gpui_telegram_desktop::views::boxes::peers::bot_profile_box::BotProfileBoxAction;
use gpui_telegram_desktop::views::browser::{
    ActiveBrowserInput, BrowserInspectorTab, BrowserLogLevel, ChromeProbeUiState,
};
use gpui_telegram_desktop::views::chat_sidebar::{DialogDeliveryStatus, DialogItemModel};
use gpui_telegram_desktop::views::drawer::inspector_tabs::InspectorAction;
use gpui_telegram_desktop::views::nav_rail::NavDestination;
use gpui_telegram_desktop::views::shell::{
    nav_destination_for_shortcut_digit, ActiveWorkspacePanel, AppShell,
};
use gpui_telegram_desktop::views::MainMenuCommand;
use gpui_telegram_proto::miniapp_contracts::{
    BotPublicProfileDto, BotRegistryEntry, MiniAppEntry, MiniAppLaunchContext,
    MiniAppLaunchMode, MiniAppWebMcpCommand,
};
use gpui_telegram_proto::sync::UpdateEvent;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CHILD_ENV: &str = "GPUI_M4B_HEADLESS_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_M4B_HEADLESS_TARGET";
const CHILD_AFTER_CTORS_ENV: &str = "GPUI_M4B_HEADLESS_AFTER_CTORS";

fn exit_headless_child(code: i32, use_underscore_exit: bool) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    if use_underscore_exit {
        #[cfg(unix)]
        unsafe {
            unsafe extern "C" {
                fn _exit(status: i32) -> !;
            }
            _exit(code);
        }
    }
    std::process::exit(code);
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

fn dispatch_headless_target(target: &str, use_underscore_exit: bool) -> ! {
    type AsyncBody = fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()> + 'static>>;
    let async_fn: AsyncBody = match target {
        "test_m4b_appshell_nav_browser_mount_address_typing_and_close" => {
            |app| Box::pin(body_appshell_nav_browser_mount_address_typing_and_close(app))
        }
        "test_m4b_cdp_surface_render_and_manual_input_forwarding" => {
            |app| Box::pin(body_cdp_surface_render_and_manual_input_forwarding(app))
        }
        "test_m4b_ai_user_takeover_resume_and_cancel_transitions" => {
            |app| Box::pin(body_ai_user_takeover_resume_and_cancel_transitions(app))
        }
        "test_m4b_non_blocking_ui_ticks_during_slow_navigation_and_replay" => {
            |app| Box::pin(body_non_blocking_ui_ticks_during_slow_navigation_and_replay(app))
        }
        "test_m4b_cookie_persistence_import_and_tcc_eperm_guidance" => {
            |app| Box::pin(body_cookie_persistence_import_and_tcc_eperm_guidance(app))
        }
        "test_m4b_distill_and_zero_llm_replay_x_social_and_general_web" => {
            |app| Box::pin(body_distill_and_zero_llm_replay_x_social_and_general_web(app))
        }
        "test_m4b_two_client_store_422_redaction_publish_install_and_replay_e2e" => {
            |app| Box::pin(body_two_client_store_422_redaction_publish_install_and_replay_e2e(app))
        }
        "test_m4b_remediation_slow_redirect_error_and_single_flight_nav" => {
            |app| Box::pin(body_remediation_slow_redirect_error_and_single_flight_nav(app))
        }
        "test_m4b_remediation_ime_multiline_viewport_dom_coalesce_host_and_cmd_comma" => {
            |app| {
                Box::pin(
                    body_remediation_ime_multiline_viewport_dom_coalesce_host_and_cmd_comma(app),
                )
            }
        }
        "test_m4b_regression_single_flight_repeat_url_and_ai_collision" => {
            |app| Box::pin(body_regression_single_flight_repeat_url_and_ai_collision(app))
        }
        "test_m4b_regression_ime_preedit_tab_and_cdp_viewport_key_bubbling" => {
            |app| Box::pin(body_regression_ime_preedit_tab_and_cdp_viewport_key_bubbling(app))
        }
        "test_m4b_regression_refresh_frame_nav_race_and_replay_url_sync" => {
            |app| Box::pin(body_regression_refresh_frame_nav_race_and_replay_url_sync(app))
        }
        "test_m4b_regression_post_settlement_refresh_and_url_convergence" => {
            |app| {
                Box::pin(body_regression_post_settlement_refresh_and_url_convergence(app))
            }
        }
        "test_m4b_regression_stale_active_input_focus_and_store_error_clear" => {
            |app| {
                Box::pin(body_regression_stale_active_input_focus_and_store_error_clear(app))
            }
        }
        "test_m4b_regression_direct_cdp_focus_ime_leak_and_cmd_n_hide" => {
            |app| {
                Box::pin(body_regression_direct_cdp_focus_ime_leak_and_cmd_n_hide(app))
            }
        }
        "test_m4b_regression_shell_blockers_overlay_ime_and_hostile_invariants" => {
            |app| {
                Box::pin(body_regression_shell_blockers_overlay_ime_and_hostile_invariants(app))
            }
        }
        "test_m4b_regression_predraw_overlay_occlusion_for_creators_miniapp_iv_computer_and_lock" => {
            |app| {
                Box::pin(
                    body_regression_predraw_overlay_occlusion_for_creators_miniapp_iv_computer_and_lock(app),
                )
            }
        }
        "test_m4b_regression_escape_miniapp_modals_cancel_ai_occluded_and_async_overlays" => {
            |app| {
                Box::pin(
                    body_regression_escape_miniapp_modals_cancel_ai_occluded_and_async_overlays(app),
                )
            }
        }
        other => {
            eprintln!("unknown m4b headless gpui test target: {other}");
            exit_headless_child(101, use_underscore_exit);
        }
    };

    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let t = target_owned.clone();
                cx.spawn(async move |app: &mut AsyncApp| {
                    let app_clone = app.clone();
                    let res = std::panic::AssertUnwindSafe(async_fn(app_clone))
                        .catch_unwind()
                        .await;
                    match res {
                        Ok(()) => exit_headless_child(0, use_underscore_exit),
                        Err(payload) => {
                            let msg = panic_payload_to_string(payload);
                            eprintln!("[headless async child panic] {t}: {msg}");
                            exit_headless_child(101, use_underscore_exit);
                        }
                    }
                })
                .detach();
            });
    }));
    if let Err(payload) = outer {
        let msg = panic_payload_to_string(payload);
        eprintln!("[headless outer panic] {target}: {msg}");
        exit_headless_child(101, use_underscore_exit);
    }
    exit_headless_child(0, use_underscore_exit);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = macos_mod_init_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_atexit_headless_entry() {
    if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
        dispatch_headless_target(&target, true);
    }
}

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_headless_entry() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        if std::env::var(CHILD_AFTER_CTORS_ENV).as_deref() == Ok("1") {
            unsafe extern "C" {
                fn atexit(func: extern "C" fn()) -> i32;
            }
            unsafe {
                atexit(macos_atexit_headless_entry);
            }
            return;
        }
        if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
            dispatch_headless_target(&target, false);
        }
    }
}

fn run_in_headless_subprocess(test_fn_name: &str) {
    run_in_headless_subprocess_impl(test_fn_name, false);
}

fn run_in_headless_subprocess_with_window(test_fn_name: &str) {
    run_in_headless_subprocess_impl(test_fn_name, true);
}

fn run_in_headless_subprocess_impl(test_fn_name: &str, after_ctors_window: bool) {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(test_fn_name, false);
    }

    let cfg_dir = TempDir::new().expect("create TempDir for GPUI_TELEGRAM_CONFIG_DIR");
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", cfg_dir.path())
        .env("GPUI_OFFLINE_TEST", "1");
    if after_ctors_window {
        cmd.env(CHILD_AFTER_CTORS_ENV, "1")
            .arg("--exact")
            .arg("__m4b_headless_child_noop__");
    } else {
        cmd.arg("--exact").arg(test_fn_name);
    }
    let output = cmd
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

// ============================================================================
// Stateful Loopback HTTP Mock Server (X/Social + General Web)
// ============================================================================

#[derive(Default)]
struct MockWebState {
    posts: Vec<String>,
    last_search_query: Option<String>,
    page_hits: Vec<String>,
}

struct MockWebServerGuard {
    base_url: String,
    state: Arc<Mutex<MockWebState>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for MockWebServerGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn parse_query_param(target: &str, key: &str) -> Option<String> {
    let (_, q) = target.split_once('?')?;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v.replace("%20", " ").replace('+', " "));
            }
        }
    }
    None
}

async fn start_mock_web_server(handle: &tokio::runtime::Handle) -> MockWebServerGuard {
    let state = Arc::new(Mutex::new(MockWebState::default()));
    let state_srv = Arc::clone(&state);
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<String>();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    handle.spawn(async move {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback mock web server");
        let addr = listener.local_addr().unwrap();
        let _ = addr_tx.send(format!("http://{addr}"));

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else { break; };
                    let st = Arc::clone(&state_srv);
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        let req_str = String::from_utf8_lossy(&buf[..n]);
                        let first_line = req_str.lines().next().unwrap_or("");
                        let mut parts = first_line.split_whitespace();
                        let method = parts.next().unwrap_or("GET");
                        let target = parts.next().unwrap_or("/");
                        let body = req_str.split("\r\n\r\n").nth(1).unwrap_or("").trim().to_string();

                        let (status, title, body_html) = {
                            let mut g = st.lock().unwrap();
                            g.page_hits.push(format!("{method} {target}"));
                            if target.starts_with("/home") {
                                ("200 OK", "Loopback Home", "<h1 data-testid=\"home-heading\">Welcome Home</h1>")
                            } else if target.starts_with("/x/compose") && method == "POST" {
                                let post_text = if body.is_empty() {
                                    "Hello X".to_string()
                                } else {
                                    body.clone()
                                };
                                g.posts.push(post_text.clone());
                                (
                                    "200 OK",
                                    "X Timeline",
                                    "<div data-testid=\"tweet-timeline\">Posted</div>",
                                )
                            } else if target.starts_with("/x/compose") {
                                (
                                    "200 OK",
                                    "X Compose",
                                    "<textarea data-testid=\"tweetTextarea_0\"></textarea><button data-testid=\"tweetButton\">Post</button>",
                                )
                            } else if target.starts_with("/x/timeline") {
                                (
                                    "200 OK",
                                    "X Timeline",
                                    "<div data-testid=\"tweet-timeline\">Timeline Loaded</div>",
                                )
                            } else if target.starts_with("/search") {
                                if let Some(q) = parse_query_param(target, "q") {
                                    g.last_search_query = Some(q);
                                }
                                (
                                    "200 OK",
                                    "Web Search Portal",
                                    "<input data-testid=\"search-input\"/><button data-testid=\"search-submit\">Search</button><div data-testid=\"search-result\">Invoice Total: $420.00</div>",
                                )
                            } else {
                                ("200 OK", "Mock Page", "<div data-testid=\"page-ok\">OK</div>")
                            }
                        };

                        let html = format!("<!doctype html><html><head><title>{title}</title></head><body>{body_html}</body></html>");
                        let resp = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                            html.len()
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                    });
                }
            }
        }
    });

    let base_url = addr_rx.await.expect("receive mock web server address");
    MockWebServerGuard {
        base_url,
        state,
        shutdown_tx: Some(shutdown_tx),
    }
}

// ============================================================================
// HTTP-Backed Interactive Browser Backend
// ============================================================================

fn render_png_frame(color: [u8; 4]) -> Vec<u8> {
    let mut img = RgbaImage::from_pixel(64, 48, Rgba([24, 32, 48, 255]));
    img.put_pixel(0, 0, Rgba(color));
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encode png");
    bytes
}

struct HttpBackedMockBrowserBackend {
    http: reqwest::Client,
    url: Mutex<String>,
    title: Mutex<String>,
    inputs: Mutex<HashMap<String, String>>,
    cookies: Mutex<Vec<BrowserCookie>>,
    pointer_events: Mutex<Vec<PointerInputEvent>>,
    key_events: Mutex<Vec<KeyInputEvent>>,
    scroll_events: Mutex<Vec<(f64, f64)>>,
    history: Mutex<Vec<String>>,
    frame_seq: AtomicU64,
    op_delay_ms: AtomicU64,
    closed: AtomicBool,
    posted_tweet_text: Mutex<Option<String>>,
    search_submitted: Mutex<Option<String>>,
}

impl HttpBackedMockBrowserBackend {
    fn is_loopback_http_url(url: &str) -> bool {
        url.starts_with("http://127.0.0.1:")
            || url.starts_with("http://localhost:")
            || url.starts_with("http://[::1]:")
    }

    fn new(initial_url: &str) -> Arc<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .connect_timeout(Duration::from_millis(250))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Arc::new(Self {
            http,
            url: Mutex::new(initial_url.to_string()),
            title: Mutex::new("Blank".to_string()),
            inputs: Mutex::new(HashMap::new()),
            cookies: Mutex::new(Vec::new()),
            pointer_events: Mutex::new(Vec::new()),
            key_events: Mutex::new(Vec::new()),
            scroll_events: Mutex::new(Vec::new()),
            history: Mutex::new(vec![initial_url.to_string()]),
            frame_seq: AtomicU64::new(1),
            op_delay_ms: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            posted_tweet_text: Mutex::new(None),
            search_submitted: Mutex::new(None),
        })
    }

    fn locator_key(loc: &SemanticLocator) -> String {
        loc.test_id
            .clone()
            .or_else(|| loc.aria_label.clone())
            .or_else(|| loc.css_selector.clone())
            .or_else(|| loc.name_attr.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn extract_title(html: &str) -> String {
        if let Some(start) = html.find("<title>") {
            let rest = &html[start + 7..];
            if let Some(end) = rest.find("</title>") {
                return rest[..end].to_string();
            }
        }
        "Mock Page".to_string()
    }
}

type MockFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl BrowserSessionBackend for HttpBackedMockBrowserBackend {
    fn backend_kind(&self) -> BrowserBackendKind {
        BrowserBackendKind::MockCdp
    }

    fn navigate<'life0, 'life1, 'async_trait>(
        &'life0 self,
        url: &'life1 str,
    ) -> MockFuture<'async_trait, Result<NavigationResult, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let delay = self.op_delay_ms.load(Ordering::Relaxed);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let mut title = format!("Page: {url}");
            if Self::is_loopback_http_url(url) {
                if let Ok(resp) = self.http.get(url).send().await {
                    if let Ok(body) = resp.text().await {
                        title = Self::extract_title(&body);
                    }
                }
            }
            *self.url.lock().unwrap() = url.to_string();
            *self.title.lock().unwrap() = title.clone();
            self.history.lock().unwrap().push(url.to_string());
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(NavigationResult {
                url: url.to_string(),
                origin: url.to_string(),
                title,
                duration_ms: delay.max(1),
                backend: BrowserBackendKind::MockCdp,
            })
        })
    }

    fn go_back<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let mut hist = self.history.lock().unwrap();
            if hist.len() > 1 {
                hist.pop();
                if let Some(prev) = hist.last().cloned() {
                    *self.url.lock().unwrap() = prev;
                }
            }
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn go_forward<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn reload<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn current_url<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(self.url.lock().unwrap().clone()) })
    }

    fn page_title<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(self.title.lock().unwrap().clone()) })
    }

    fn snapshot_dom<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<BrowserSnapshot, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let cur = self.url.lock().unwrap().clone();
            Ok(BrowserSnapshot {
                url: cur.clone(),
                origin: cur,
                title: self.title.lock().unwrap().clone(),
                text_excerpt: "mock dom".to_string(),
                interactive_elements: Vec::new(),
                redacted_field_count: 0,
            })
        })
    }

    fn eval_js<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _script: &'life1 str,
    ) -> MockFuture<'async_trait, Result<serde_json::Value, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(serde_json::json!({ "ok": true })) })
    }

    fn click_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let delay = self.op_delay_ms.load(Ordering::Relaxed);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let key = Self::locator_key(locator);
            let cur_url = self.url.lock().unwrap().clone();
            if key == "tweetButton" {
                let text = self
                    .inputs
                    .lock()
                    .unwrap()
                    .get("tweetTextarea_0")
                    .cloned()
                    .unwrap_or_default();
                *self.posted_tweet_text.lock().unwrap() = Some(text.clone());
                if let Some(base) = cur_url.split("/x/").next() {
                    let post_url = format!("{base}/x/compose");
                    if Self::is_loopback_http_url(&post_url) {
                        let _ = self.http.post(&post_url).body(text).send().await;
                    }
                    let timeline_url = format!("{base}/x/timeline");
                    *self.url.lock().unwrap() = timeline_url;
                    *self.title.lock().unwrap() = "X Timeline".to_string();
                }
            } else if key == "search-submit" {
                let q = self
                    .inputs
                    .lock()
                    .unwrap()
                    .get("search-input")
                    .cloned()
                    .unwrap_or_default();
                *self.search_submitted.lock().unwrap() = Some(q.clone());
                if let Some(base) = cur_url.split("/search").next() {
                    let query_url = format!("{base}/search?q={}", q.replace(' ', "+"));
                    if Self::is_loopback_http_url(&query_url) {
                        let _ = self.http.get(&query_url).send().await;
                    }
                    *self.url.lock().unwrap() = query_url;
                }
            }
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(ActionOutcome {
                ok: true,
                operation: format!("click_locator:{key}"),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn fill_locator<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
        value: &'life2 str,
        _is_secret: bool,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let key = Self::locator_key(locator);
            self.inputs
                .lock()
                .unwrap()
                .insert(key.clone(), value.to_string());
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(ActionOutcome {
                ok: true,
                operation: format!("fill_locator:{key}"),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn press_key<'life0, 'life1, 'async_trait>(
        &'life0 self,
        key: &'life1 str,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(ActionOutcome {
                ok: true,
                operation: format!("press_key:{key}"),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn scroll<'life0, 'async_trait>(
        &'life0 self,
        delta_x: f64,
        delta_y: f64,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.scroll_events.lock().unwrap().push((delta_x, delta_y));
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(ActionOutcome {
                ok: true,
                operation: "scroll".to_string(),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn wait_for_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
        _timeout_ms: u64,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let key = Self::locator_key(locator);
            Ok(ActionOutcome {
                ok: true,
                operation: format!("wait_for_locator:{key}"),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn extract_text_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
    ) -> MockFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let key = Self::locator_key(locator);
            if key == "search-result" {
                let q = self
                    .search_submitted
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_default();
                return Ok(format!("Invoice Total for {q}: $420.00"));
            }
            if key == "tweet-timeline" {
                return Ok(self
                    .posted_tweet_text
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "Timeline Loaded".to_string()));
            }
            Ok("Mock Text".to_string())
        })
    }

    fn capture_screenshot<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<Vec<u8>, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let delay = self.op_delay_ms.load(Ordering::Relaxed);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let seq = self.frame_seq.load(Ordering::Relaxed);
            let r = ((seq * 47) % 255) as u8;
            let g = ((seq * 83) % 255) as u8;
            Ok(render_png_frame([r, g, 220, 255]))
        })
    }

    fn export_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        domain_filter: Option<&'life1 str>,
    ) -> MockFuture<'async_trait, Result<Vec<BrowserCookie>, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let guard = self.cookies.lock().unwrap();
            match domain_filter {
                Some(dom) => Ok(guard
                    .iter()
                    .filter(|c| c.domain.contains(dom))
                    .cloned()
                    .collect()),
                None => Ok(guard.clone()),
            }
        })
    }

    fn import_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        cookies: &'life1 [BrowserCookie],
    ) -> MockFuture<'async_trait, Result<usize, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let mut guard = self.cookies.lock().unwrap();
            for c in cookies {
                if let Some(pos) = guard
                    .iter()
                    .position(|ex| ex.name == c.name && ex.domain == c.domain)
                {
                    guard[pos] = c.clone();
                } else {
                    guard.push(c.clone());
                }
            }
            Ok(cookies.len())
        })
    }

    fn dispatch_pointer_event<'life0, 'async_trait>(
        &'life0 self,
        event: PointerInputEvent,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.pointer_events.lock().unwrap().push(event);
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn dispatch_key_event<'life0, 'async_trait>(
        &'life0 self,
        event: KeyInputEvent,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.key_events.lock().unwrap().push(event);
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn performance_metrics(&self) -> BrowserPerformanceMetrics {
        BrowserPerformanceMetrics {
            backend: Some(BrowserBackendKind::MockCdp),
            ..Default::default()
        }
    }

    fn close<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

// ============================================================================
// Real gpui_telegram_server Guard (TempDir SQLite DB)
// ============================================================================

fn find_server_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let target_debug = workspace_root
        .join("target")
        .join("debug")
        .join("gpui_telegram_server");
    if target_debug.exists() {
        return target_debug;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            let cand = p.join("gpui_telegram_server");
            if cand.exists() {
                return cand;
            }
            if let Some(gp) = p.parent() {
                let cand = gp.join("gpui_telegram_server");
                if cand.exists() {
                    return cand;
                }
            }
        }
    }
    let _ = std::process::Command::new("cargo")
        .args(["build", "-p", "gpui_telegram_server"])
        .current_dir(workspace_root)
        .status();
    target_debug
}

struct StoreServerGuard {
    _db_dir: TempDir,
    child: std::process::Child,
    base_url: String,
}

impl StoreServerGuard {
    async fn start() -> Self {
        let db_dir = TempDir::new().expect("temp dir for server sqlite");
        let db_path = db_dir.path().join("server.db");
        let media_dir = db_dir.path().join("cas");
        std::fs::create_dir_all(&media_dir).unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let bin = find_server_binary();
        assert!(
            bin.exists(),
            "gpui_telegram_server binary missing at {}",
            bin.display()
        );

        let child = std::process::Command::new(&bin)
            .env("HOST", "127.0.0.1")
            .env("PORT", port.to_string())
            .env("DATABASE_URL", format!("sqlite://{}", db_path.display()))
            .env("MEDIA_STORAGE_PATH", media_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn gpui_telegram_server");

        let base_url = format!("http://127.0.0.1:{port}");
        let http = HttpClient::new(base_url.clone());
        let start = Instant::now();
        let mut ready = false;
        while start.elapsed() < Duration::from_secs(8) {
            if let Ok(true) = http.check_health().await {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(ready, "gpui_telegram_server failed health check");
        Self {
            _db_dir: db_dir,
            child,
            base_url,
        }
    }
}

impl Drop for StoreServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn new_isolated_client(server_url: &str) -> (TempDir, Arc<TelegramClient>) {
    let dir = TempDir::new().expect("temp dir for client");
    let cfg = ClientConfig {
        server_url: server_url.to_string(),
        ..Default::default()
    };
    let client = Arc::new(TelegramClient::open_with_dir(cfg, dir.path()).unwrap());
    (dir, client)
}

fn build_authenticated_shell(
    app: &mut App,
    client: Arc<TelegramClient>,
    user_id: u64,
    username: &str,
) -> Entity<AppShell> {
    let _rt_guard = client.runtime().enter();
    app.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.is_authenticated = true;
        s.session_sync.user_id = Some(user_id);
        s.session_sync.username = Some(username.to_string());
        s.session_sync.session_token = Some(format!("tok_{username}"));
        s
    })
}

async fn pump_until(
    app: &mut AsyncApp,
    timeout: Duration,
    mut predicate: impl FnMut(&mut App) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let done = app.update(|cx| predicate(cx)).unwrap_or(false);
        if done {
            return true;
        }
        app.background_executor().timer(Duration::from_millis(15)).await;
    }
    app.update(|cx| predicate(cx)).unwrap_or(false)
}

// ============================================================================
// Test 1: Mount AppShell + BrowserWorkspace + Address Input + Navigation + Close
// ============================================================================

#[test]
fn test_m4b_appshell_nav_browser_mount_address_typing_and_close() {
    run_in_headless_subprocess("test_m4b_appshell_nav_browser_mount_address_typing_and_close");
}

async fn body_appshell_nav_browser_mount_address_typing_and_close(mut app: AsyncApp) {
    assert_eq!(IconName::ALL.len(), 64);
    assert_eq!(
        nav_destination_for_shortcut_digit("9"),
        Some(NavDestination::Browser)
    );

    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let rt_handle = client.runtime().handle().clone();
    let web = rt_handle.block_on(start_mock_web_server(&rt_handle));
    let home_url = format!("{}/home", web.base_url);

    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 101, "alice"))
        .unwrap();

    // 1. Open BrowserWorkspace via Cmd+9 shortcut
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            assert!(s.handle_shortcut_key("9", false, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            assert_eq!(
                s.nav_rail.read(scx).active_destination,
                NavDestination::Browser
            );
            assert!(s.should_hide_composer(scx));
        });
    })
    .unwrap();

    // 2. Attach loopback-HTTP backend and type URL via AppShell IME/input routing
    let backend = HttpBackedMockBrowserBackend::new("about:blank");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(
                    gpui_telegram_desktop::views::browser::ActiveBrowserInput::AddressBar,
                    None,
                    wcx,
                );
            });
            assert!(s.is_browser_workspace_input_active(scx));
            s.clear_active_field(scx);
            s.insert_committed_text(&home_url, scx);
            assert!(s.handle_non_char_key("enter", false, scx));
        });
    })
    .unwrap();

    let navigated = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let s = shell.read(cx);
        let Some(bw) = s.browser_workspace() else {
            return false;
        };
        let w = bw.read(cx);
        w.current_url == home_url && w.page_title == "Loopback Home"
    })
    .await;
    assert!(navigated, "Address bar Enter must navigate to loopback /home");
    assert!(web
        .state
        .lock()
        .unwrap()
        .page_hits
        .iter()
        .any(|h| h == "GET /home"));

    // 3. First Escape blurs AddressBar; second Escape returns to Chats and hides/occludes BrowserWorkspace
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            assert!(s.handle_non_char_key("escape", false, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            assert!(!s.is_browser_workspace_input_active(scx));
            assert!(s.handle_non_char_key("escape", false, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            assert_eq!(
                s.nav_rail.read(scx).active_destination,
                NavDestination::Chats
            );
            let bw = s.browser_workspace().unwrap().read(scx);
            assert!(!bw.is_workspace_visible());
            assert!(bw.is_externally_occluded());
        });
    })
    .unwrap();

    // 4. Re-opening via NavRail::select(NavDestination::Browser) works via observer
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.nav_rail.update(scx, |r, rcx| {
                r.select(NavDestination::Browser);
                rcx.notify();
            });
        });
    })
    .unwrap();

    let reopened = pump_until(&mut app, Duration::from_secs(2), |cx| {
        shell.read(cx).active_panel == ActiveWorkspacePanel::Browser
    })
    .await;
    assert!(reopened, "NavRail::select(Browser) must re-open BrowserWorkspace");

    // 5. Cmd+Shift+B toggles Browser off/on, and MainMenuCommand::Browser opens BrowserWorkspace
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            assert!(s.handle_shortcut_key("b", true, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            assert!(s.handle_shortcut_key("b", true, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            s.activate_nav_destination(NavDestination::Chats, scx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            s.handle_main_menu_command(MainMenuCommand::Browser, scx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
        });
    })
    .unwrap();

    app.update(|_cx| drop(shell)).unwrap();
    drop(web);
    drop(client);
}

// ============================================================================
// Test 2: CdpViewportSurface Frame Render & Manual Click / Key / Scroll
// ============================================================================

#[test]
fn test_m4b_cdp_surface_render_and_manual_input_forwarding() {
    run_in_headless_subprocess("test_m4b_cdp_surface_render_and_manual_input_forwarding");
}

async fn body_cdp_surface_render_and_manual_input_forwarding(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 102, "bob"))
        .unwrap();

    let backend = HttpBackedMockBrowserBackend::new("https://example.local");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    let frame_ok = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let Some(bw) = shell.read(cx).browser_workspace() else {
            return false;
        };
        let Some(surface) = bw.read(cx).cdp_surface() else {
            return false;
        };
        let sr = surface.read(cx);
        sr.has_rendered_frame() && sr.last_frame_revision() >= 1
    })
    .await;
    assert!(frame_ok, "CdpViewportSurface must decode and hold RenderImage");

    let initial_rev = app
        .update(|cx| {
            shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .cdp_surface()
                .unwrap()
                .read(cx)
                .last_frame_revision()
        })
        .unwrap();

    // Forward manual click, scroll, key down, and text input through CdpViewportSurface
    app.update(|cx| {
        let surface = shell
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surface.update(cx, |sr, scx| {
            sr.handle_click(120.0, 240.0, PointerButton::Left, 1, 0, scx);
            sr.handle_scroll(120.0, 240.0, 0.0, -180.0, 0, scx);
            sr.handle_key_down("Enter", Some("Enter".into()), None, Some(13), 0, scx);
            sr.send_text_input("hello", scx);
        });
    })
    .unwrap();

    let inputs_forwarded = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let rev = shell
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .read(cx)
            .last_frame_revision();
        let ptrs = backend.pointer_events.lock().unwrap();
        let keys = backend.key_events.lock().unwrap();
        let has_click = ptrs.iter().any(|p| p.kind == PointerEventKind::Down && (p.x - 120.0).abs() < 0.1);
        let has_wheel = ptrs.iter().any(|p| p.kind == PointerEventKind::Wheel && p.delta_y < -100.0);
        let has_text = keys.iter().any(|k| k.text.as_deref() == Some("hello"));
        rev > initial_rev && has_click && has_wheel && has_text
    })
    .await;
    assert!(
        inputs_forwarded,
        "Click, wheel scroll, and text input must forward to backend and refresh frame"
    );

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}



// ============================================================================
// Test 3: AI -> User Takeover -> Resume AI & Cancel AI Transitions
// ============================================================================

#[test]
fn test_m4b_ai_user_takeover_resume_and_cancel_transitions() {
    run_in_headless_subprocess("test_m4b_ai_user_takeover_resume_and_cancel_transitions");
}

async fn body_ai_user_takeover_resume_and_cancel_transitions(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let rt_handle = client.runtime().handle().clone();
    let web = rt_handle.block_on(start_mock_web_server(&rt_handle));
    let login_url = format!("{}/x/timeline", web.base_url);
    let step1_url = format!("{}/x/compose", web.base_url);

    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 103, "carol"))
        .unwrap();

    let backend = HttpBackedMockBrowserBackend::new(&login_url);
    backend.op_delay_ms.store(90, Ordering::Relaxed);

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    // Script with 4 steps: Navigate -> Click -> Fill -> Click
    let app_script = AutomationScriptApp {
        schema_version: 1,
        app_id: "takeover-test-app".to_string(),
        slug: "takeover-test-app".to_string(),
        name: "Takeover Test".to_string(),
        version: "1.0.0".to_string(),
        description: "Verify takeover and resume".to_string(),
        category: "social".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: vec![],
        steps: vec![
            ScriptStep::Navigate {
                url_template: step1_url,
            },
            ScriptStep::Click {
                locator: SemanticLocator {
                    test_id: Some("step2-btn".to_string()),
                    ..Default::default()
                },
            },
            ScriptStep::Fill {
                locator: SemanticLocator {
                    test_id: Some("tweetTextarea_0".to_string()),
                    ..Default::default()
                },
                value_template: "resumed post".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator {
                    test_id: Some("tweetButton".to_string()),
                    ..Default::default()
                },
            },
        ],
        required_capabilities: vec![],
        author_id: None,
        source_trace_summary: None,
    };

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_run_script_app(app_script.clone(), BTreeMap::new(), scx);
        });
    })
    .unwrap();

    // Wait until AIControlling is active
    let entered_ai = pump_until(&mut app, Duration::from_secs(2), |cx| {
        matches!(
            shell.read(cx).browser_control_mode(cx),
            BrowserControlMode::AiControlling { .. }
        )
    })
    .await;
    assert!(entered_ai, "Expected BrowserControlMode::AiControlling");

    // User triggers Take Over
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_user_takeover(scx);
            assert!(matches!(
                s.browser_control_mode(scx),
                BrowserControlMode::UserTakeover { .. }
            ));
        });
    })
    .unwrap();

    // Verify AI remains paused for 180ms while user manually navigates/updates state
    app.background_executor()
        .timer(Duration::from_millis(180))
        .await;
    app.update(|cx| {
        let s = shell.read(cx);
        assert!(matches!(
            s.browser_control_mode(cx),
            BrowserControlMode::UserTakeover { .. }
        ));
        let bw = s.browser_workspace().unwrap().read(cx);
        assert!(bw.recorder.is_replaying, "Replay should still be suspended awaiting resume");
    })
    .unwrap();

    // Speed up backend and resume AI
    backend.op_delay_ms.store(5, Ordering::Relaxed);
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_resume_ai(scx);
        });
    })
    .unwrap();

    let completed = pump_until(&mut app, Duration::from_secs(4), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.recorder.is_replaying && bw.recorder.last_replay_report.is_some()
    })
    .await;
    assert!(completed, "Resumed AI/script must complete cleanly");

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let rep = bw.recorder.last_replay_report.as_ref().unwrap();
        assert!(rep.success);
        assert!(!rep.used_llm);
        assert_eq!(rep.steps_executed, 4);
        assert_eq!(
            backend.posted_tweet_text.lock().unwrap().as_deref(),
            Some("resumed post")
        );
    })
    .unwrap();

    // Verify Cancel AI transitions cleanly back to Idle
    backend.op_delay_ms.store(120, Ordering::Relaxed);
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_run_script_app(app_script, BTreeMap::new(), scx);
        });
    })
    .unwrap();
    let entered_ai_2 = pump_until(&mut app, Duration::from_secs(2), |cx| {
        matches!(
            shell.read(cx).browser_control_mode(cx),
            BrowserControlMode::AiControlling { .. }
        )
    })
    .await;
    assert!(entered_ai_2);

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_cancel_ai(scx);
        });
    })
    .unwrap();

    let cancelled = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.recorder.is_replaying
            && matches!(
                shell.read(cx).browser_control_mode(cx),
                BrowserControlMode::Idle
            )
    })
    .await;
    assert!(cancelled, "Cancel AI must return to Idle and stop replay");

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}



// ============================================================================
// Test 4: Non-Blocking UI Ticks During Simulated Slow Backend Navigation
// ============================================================================

#[test]
fn test_m4b_non_blocking_ui_ticks_during_slow_navigation_and_replay() {
    run_in_headless_subprocess("test_m4b_non_blocking_ui_ticks_during_slow_navigation_and_replay");
}

async fn body_non_blocking_ui_ticks_during_slow_navigation_and_replay(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let rt_handle = client.runtime().handle().clone();
    let web = rt_handle.block_on(start_mock_web_server(&rt_handle));
    let slow_url = format!("{}/home", web.base_url);

    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 104, "dave"))
        .unwrap();

    let backend = HttpBackedMockBrowserBackend::new("about:blank");
    backend.op_delay_ms.store(220, Ordering::Relaxed);

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    // Wait for initial frame then trigger slow navigate
    let _ = pump_until(&mut app, Duration::from_secs(2), |cx| {
        shell
            .read(cx)
            .browser_workspace()
            .and_then(|bw| bw.read(cx).cdp_surface().cloned())
            .is_some_and(|s| !s.read(cx).is_refreshing())
    })
    .await;

    let ui_ticks = Arc::new(AtomicU64::new(0));
    let ticks_before_complete = Arc::new(AtomicU64::new(0));
    let mut finished = false;

    let slow_url_nav = slow_url.clone();
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_navigate(&slow_url_nav, scx);
        });
    })
    .unwrap();

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        let done = app
            .update(|cx| {
                let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
                let sr = bw.cdp_surface().unwrap().read(cx);
                sr.current_url() == slow_url
                    && !sr.is_refreshing()
                    && bw.current_url == slow_url
                    && !bw.is_loading
            })
            .unwrap_or(false);
        if done {
            ticks_before_complete.store(ui_ticks.load(Ordering::Relaxed), Ordering::Relaxed);
            finished = true;
            break;
        }
        ui_ticks.fetch_add(1, Ordering::Relaxed);
        app.background_executor().timer(Duration::from_millis(15)).await;
    }

    assert!(finished, "Slow loopback navigation must complete within 3s");
    assert!(
        ticks_before_complete.load(Ordering::Relaxed) >= 5,
        "Expected >= 5 non-blocking UI ticks while slow backend navigated, got {}",
        ticks_before_complete.load(Ordering::Relaxed)
    );
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(bw.current_url, slow_url);
        assert!(!bw.is_loading);
        assert_eq!(bw.page_title, "Loopback Home");
    })
    .unwrap();

    // Now verify >= 5 non-blocking UI ticks during slow browser_run_script_app replay
    backend.op_delay_ms.store(120, Ordering::Relaxed);
    let replay_app = AutomationScriptApp {
        schema_version: 1,
        app_id: "m4b-slow-replay-ticks".to_string(),
        slug: "m4b-slow-replay-ticks".to_string(),
        name: "Slow Replay".to_string(),
        version: "1.0.0".to_string(),
        description: "Exercises non-blocking UI ticks during slow replay".to_string(),
        category: "general".to_string(),
        target_domains: vec!["127.0.0.1".to_string()],
        parameters: Vec::new(),
        steps: vec![
            ScriptStep::Navigate {
                url_template: format!("{}/search", web.base_url),
            },
            ScriptStep::Fill {
                locator: SemanticLocator {
                    test_id: Some("search-input".to_string()),
                    ..Default::default()
                },
                value_template: "slow-tick-query".to_string(),
                secret_vault_key: None,
            },
            ScriptStep::Click {
                locator: SemanticLocator {
                    test_id: Some("search-submit".to_string()),
                    ..Default::default()
                },
            },
        ],
        required_capabilities: vec!["navigate".to_string(), "fill".to_string(), "click".to_string()],
        author_id: None,
        source_trace_summary: None,
    };

    let replay_ticks = Arc::new(AtomicU64::new(0));
    let ticks_during_replay = Arc::new(AtomicU64::new(0));
    let mut replay_finished = false;

    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_run_script_app(replay_app, BTreeMap::new(), scx);
        });
    })
    .unwrap();

    let replay_start = Instant::now();
    while replay_start.elapsed() < Duration::from_secs(4) {
        let done = app
            .update(|cx| {
                let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
                !bw.recorder.is_replaying && bw.recorder.last_replay_report.is_some()
            })
            .unwrap_or(false);
        if done {
            ticks_during_replay.store(replay_ticks.load(Ordering::Relaxed), Ordering::Relaxed);
            replay_finished = true;
            break;
        }
        replay_ticks.fetch_add(1, Ordering::Relaxed);
        app.background_executor().timer(Duration::from_millis(15)).await;
    }

    assert!(replay_finished, "Slow script replay must complete within 4s");
    assert!(
        ticks_during_replay.load(Ordering::Relaxed) >= 5,
        "Expected >= 5 non-blocking UI ticks during slow browser_run_script_app replay, got {}",
        ticks_during_replay.load(Ordering::Relaxed)
    );
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let rep = bw.recorder.last_replay_report.as_ref().unwrap();
        assert!(rep.success);
        assert!(!rep.used_llm);
    })
    .unwrap();

    app.update(|_cx| drop(shell)).unwrap();
    drop(web);
    drop(client);
}

// ============================================================================
// Test 5: Cookie Persistence, JSON/Netscape Import & Chrome TCC EPERM Guidance
// ============================================================================

#[test]
fn test_m4b_cookie_persistence_import_and_tcc_eperm_guidance() {
    run_in_headless_subprocess("test_m4b_cookie_persistence_import_and_tcc_eperm_guidance");
}

async fn body_cookie_persistence_import_and_tcc_eperm_guidance(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 105, "eve"))
        .unwrap();

    let backend1 = HttpBackedMockBrowserBackend::new("https://x.com");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend1.clone(), scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.switch_profile("x-social".to_string(), wcx);
                // Import Netscape cookies
                let netscape = ".x.com\tTRUE\t/\tTRUE\t1893456000\tauth_token\tdemo_tok_1\n\
                                .x.com\tTRUE\t/\tTRUE\t1893456000\tct0\tdemo_ct0_1\n";
                w.import_cookies_from_text_async(netscape.to_string(), wcx);
            });
        });
    })
    .unwrap();

    let imported = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        bw.recorder.last_imported_cookie_count >= 2 && bw.recorder.last_persisted_cookie_count >= 2
    })
    .await;
    assert!(imported, "Netscape cookies must import into backend and BrowserProfileStore");
    assert_eq!(backend1.cookies.lock().unwrap().len(), 2);

    // Also test JSON cookie import merging
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            let json = r#"[{"name":"twid","value":"u=999","domain":".x.com","path":"/","secure":true,"httpOnly":false}]"#;
            w.import_cookies_from_text_async(json.to_string(), wcx);
        });
    })
    .unwrap();
    let merged = pump_until(&mut app, Duration::from_secs(3), |cx| {
        shell
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .recorder
            .last_persisted_cookie_count
            >= 3
    })
    .await;
    assert!(merged, "JSON cookie import must merge cleanly with persisted profile");

    // Simulate reload into a fresh backend via BrowserProfileStore
    let backend2 = HttpBackedMockBrowserBackend::new("https://x.com");
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.set_cdp_backend(backend2.clone(), wcx);
            w.restore_active_profile_cookies(wcx);
        });
    })
    .unwrap();

    let restored = pump_until(&mut app, Duration::from_secs(3), |_| {
        backend2.cookies.lock().unwrap().len() >= 3
    })
    .await;
    assert!(restored, "Fresh backend must receive all 3 persisted cookies from profile");

    // Verify Chrome TCC EPERM guidance surfacing in RecorderPanel
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            let fake_path = PathBuf::from("/Users/test/Library/Application Support/Google/Chrome/Default/Cookies");
            let eperm = std::io::Error::from_raw_os_error(1);
            let probe = classify_probe_io_error(&fake_path, &eperm);
            assert!(matches!(probe, ChromeImportProbe::PermissionDeniedTcc { .. }));
            w.apply_chrome_probe_result(probe, wcx);
            match &w.recorder.chrome_probe {
                ChromeProbeUiState::PermissionDeniedTcc { path, guidance } => {
                    assert!(path.contains("Chrome/Default/Cookies"));
                    assert!(!guidance.trim().is_empty());
                }
                other => panic!("Expected PermissionDeniedTcc, got {other:?}"),
            }
        });
    })
    .unwrap();

    // Verify BrowserProfileStore persisted files exist on disk
    let data_dir = app
        .update(|cx| {
            shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .data_dir
                .clone()
        })
        .unwrap();
    let store = BrowserProfileStore::new(&data_dir);
    let stored = store.load_cookies("x-social", None).unwrap();
    assert!(stored.len() >= 3);

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}



// ============================================================================
// Test 6: Trace Distillation -> Zero-LLM Replay (X Social & General Web)
// ============================================================================

#[test]
fn test_m4b_distill_and_zero_llm_replay_x_social_and_general_web() {
    run_in_headless_subprocess("test_m4b_distill_and_zero_llm_replay_x_social_and_general_web");
}

async fn body_distill_and_zero_llm_replay_x_social_and_general_web(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let rt_handle = client.runtime().handle().clone();
    let web = rt_handle.block_on(start_mock_web_server(&rt_handle));

    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 106, "frank"))
        .unwrap();

    let backend = HttpBackedMockBrowserBackend::new("about:blank");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    // --- Flow A: Mock X / Social Media ---
    let x_compose_url = format!("{}/x/compose", web.base_url);
    let x_app = app
        .update(|cx| {
            shell.update(cx, |s, scx| {
                let bw = s.browser_workspace().unwrap().read(scx);
                let session = bw.automation_session().unwrap().clone();
                session.record_navigate(&x_compose_url, 12);
                session.record_fill(
                    Some(x_compose_url.clone()),
                    SemanticLocator {
                        test_id: Some("tweetTextarea_0".to_string()),
                        ..Default::default()
                    },
                    "Automated X post from M4B",
                    false,
                    None,
                    Some("post_body".to_string()),
                    8,
                );
                session.record_click(
                    Some(x_compose_url.clone()),
                    SemanticLocator {
                        test_id: Some("tweetButton".to_string()),
                        ..Default::default()
                    },
                    10,
                );
                session.record_extract_text(
                    Some(format!("{}/x/timeline", web.base_url)),
                    SemanticLocator {
                        test_id: Some("tweet-timeline".to_string()),
                        ..Default::default()
                    },
                    "timeline_post".to_string(),
                    5,
                );
                s.browser_distill_last_trace(
                    "X Auto Poster",
                    "Posts an update on X without LLM",
                    "social",
                    scx,
                )
                .expect("distill X trace")
            })
        })
        .unwrap();

    assert_eq!(x_app.category, "social");
    assert!(x_app.steps.len() >= 4);

    // Replay X Script App with parameter override
    let mut x_params = BTreeMap::new();
    x_params.insert("post_body".to_string(), "Hello Replayed X!".to_string());
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_run_script_app(x_app.clone(), x_params, scx);
        });
    })
    .unwrap();

    let x_replayed = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.recorder.is_replaying
            && bw
                .recorder
                .last_replay_report
                .as_ref()
                .is_some_and(|r| r.app_id == x_app.app_id)
    })
    .await;
    assert!(x_replayed, "X ScriptApp replay must finish");
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let rep = bw.recorder.last_replay_report.as_ref().unwrap();
        assert!(rep.success);
        assert!(!rep.used_llm);
        assert!(!rep.step_timings.is_empty());
        assert_eq!(
            rep.extracted_outputs.get("timeline_post").map(String::as_str),
            Some("Hello Replayed X!")
        );
    })
    .unwrap();
    assert!(web
        .state
        .lock()
        .unwrap()
        .posts
        .iter()
        .any(|p| p == "Hello Replayed X!"));

    // --- Flow B: Non-Social General Web Search / Extract ---
    let search_url = format!("{}/search", web.base_url);
    let web_app = app
        .update(|cx| {
            shell.update(cx, |s, scx| {
                let bw = s.browser_workspace().unwrap().read(scx);
                let session = bw.automation_session().unwrap().clone();
                session.record_navigate(&search_url, 10);
                session.record_fill(
                    Some(search_url.clone()),
                    SemanticLocator {
                        test_id: Some("search-input".to_string()),
                        ..Default::default()
                    },
                    "Q4 Invoice",
                    false,
                    None,
                    Some("query".to_string()),
                    7,
                );
                session.record_click(
                    Some(search_url.clone()),
                    SemanticLocator {
                        test_id: Some("search-submit".to_string()),
                        ..Default::default()
                    },
                    9,
                );
                session.record_extract_text(
                    Some(search_url.clone()),
                    SemanticLocator {
                        test_id: Some("search-result".to_string()),
                        ..Default::default()
                    },
                    "invoice_summary".to_string(),
                    6,
                );
                s.browser_distill_last_trace(
                    "Invoice Lookup",
                    "Searches portal and extracts invoice total",
                    "general",
                    scx,
                )
                .expect("distill general web trace")
            })
        })
        .unwrap();

    assert_eq!(web_app.category, "general");
    let mut web_params = BTreeMap::new();
    web_params.insert("query".to_string(), "ACME-2026".to_string());
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_run_script_app(web_app.clone(), web_params, scx);
        });
    })
    .unwrap();

    let web_replayed = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.recorder.is_replaying
            && bw
                .recorder
                .last_replay_report
                .as_ref()
                .is_some_and(|r| r.app_id == web_app.app_id)
    })
    .await;
    assert!(web_replayed, "General web ScriptApp replay must finish");
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let rep = bw.recorder.last_replay_report.as_ref().unwrap();
        assert!(rep.success);
        assert!(!rep.used_llm);
        assert_eq!(
            rep.extracted_outputs.get("invoice_summary").map(String::as_str),
            Some("Invoice Total for ACME-2026: $420.00")
        );
        assert!(bw.recorder.local_scripts.len() >= 2);
    })
    .unwrap();
    assert_eq!(
        web.state.lock().unwrap().last_search_query.as_deref(),
        Some("ACME-2026")
    );

    app.update(|_cx| drop(shell)).unwrap();
    drop(web);
    drop(client);
}

// ============================================================================
// Test 7: Full 2-Client Store E2E (422 Secret Redaction -> Publish -> Install -> Replay)
// ============================================================================

#[test]
fn test_m4b_two_client_store_422_redaction_publish_install_and_replay_e2e() {
    run_in_headless_subprocess(
        "test_m4b_two_client_store_422_redaction_publish_install_and_replay_e2e",
    );
}

async fn body_two_client_store_422_redaction_publish_install_and_replay_e2e(mut app: AsyncApp) {
    let (_rt_dir, rt_client) = new_isolated_client("http://127.0.0.1:9");
    let rt_handle = rt_client.runtime().handle().clone();

    let server = rt_handle.block_on(StoreServerGuard::start());
    let web = rt_handle.block_on(start_mock_web_server(&rt_handle));

    let (_alice_dir, client_a) = new_isolated_client(&server.base_url);
    let (bob_dir, client_b) = new_isolated_client(&server.base_url);

    let alice_auth = rt_handle
        .block_on(client_a.register(&RegisterRequest {
            username: "m4b_alice".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Alice".to_string(),
            device_name: "mac-a".to_string(),
        }))
        .expect("register Alice");
    client_a.set_auth_session(alice_auth.clone());

    let bob_auth = rt_handle
        .block_on(client_b.register(&RegisterRequest {
            username: "m4b_bob".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Bob".to_string(),
            device_name: "mac-b".to_string(),
        }))
        .expect("register Bob");
    client_b.set_auth_session(bob_auth.clone());

    // --- Client A AppShell ---
    let shell_a = app
        .update(|cx| build_authenticated_shell(cx, client_a.clone(), alice_auth.user_id, "m4b_alice"))
        .unwrap();
    let backend_a = HttpBackedMockBrowserBackend::new("about:blank");
    let x_url = format!("{}/x/compose", web.base_url);

    let clean_app = app
        .update(|cx| {
            shell_a.update(cx, |s, scx| {
                s.set_browser_backend_for_test(backend_a.clone(), scx);
                let bw = s.browser_workspace().unwrap().read(scx);
                let sess = bw.automation_session().unwrap().clone();
                sess.record_navigate(&x_url, 10);
                sess.record_fill(
                    Some(x_url.clone()),
                    SemanticLocator {
                        test_id: Some("tweetTextarea_0".to_string()),
                        ..Default::default()
                    },
                    "Cross-client store post",
                    false,
                    None,
                    Some("message".to_string()),
                    8,
                );
                sess.record_click(
                    Some(x_url.clone()),
                    SemanticLocator {
                        test_id: Some("tweetButton".to_string()),
                        ..Default::default()
                    },
                    10,
                );
                s.browser_distill_last_trace(
                    "Store X Broadcaster",
                    "Clean distilled social poster",
                    "social",
                    scx,
                )
                .expect("distill clean app")
            })
        })
        .unwrap();

    // 1. Client A tries to publish a secret-containing draft -> server returns 422 -> StorePanel surfaces it
    let mut leaky_app = clean_app.clone();
    leaky_app.description = "Authorization: Bearer super_secret_token_999".to_string();
    app.update(|cx| {
        shell_a.update(cx, |s, scx| {
            s.browser_publish_script_app(leaky_app, scx);
        });
    })
    .unwrap();

    let rejected_422 = pump_until(&mut app, Duration::from_secs(4), |cx| {
        let bw = shell_a.read(cx).browser_workspace().unwrap().read(cx);
        !bw.store.is_publishing && bw.store.publish_secret_rejected
    })
    .await;
    assert!(rejected_422, "StorePanel must surface 422 secret_leak_detected");
    app.update(|cx| {
        let bw = shell_a.read(cx).browser_workspace().unwrap().read(cx);
        let err = bw.store.publish_error.as_deref().unwrap_or("");
        assert!(err.contains("422"), "Expected 422 in error: {err}");
    })
    .unwrap();

    // 2. Client A publishes clean distilled app
    let mut clean_for_store = clean_app.clone();
    clean_for_store.app_id = "m4b-store-x-broadcaster".to_string();
    clean_for_store.slug = "m4b-store-x-broadcaster".to_string();
    app.update(|cx| {
        shell_a.update(cx, |s, scx| {
            s.browser_publish_script_app_with_changelog(
                clean_for_store.clone(),
                Some("Initial M4B store release".to_string()),
                scx,
            );
        });
    })
    .unwrap();

    let published = pump_until(&mut app, Duration::from_secs(4), |cx| {
        let bw = shell_a.read(cx).browser_workspace().unwrap().read(cx);
        !bw.store.is_publishing && bw.store.publish_success.is_some()
    })
    .await;
    assert!(published, "Clean app must publish to real gpui_telegram_server");

    // --- Client B AppShell ---
    let shell_b = app
        .update(|cx| build_authenticated_shell(cx, client_b.clone(), bob_auth.user_id, "m4b_bob"))
        .unwrap();
    let backend_b = HttpBackedMockBrowserBackend::new("about:blank");

    app.update(|cx| {
        shell_b.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend_b.clone(), scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.inspector_tab = BrowserInspectorTab::Store;
                w.store.search_query_draft = "Broadcaster".to_string();
                w.search_store_apps(wcx);
            });
        });
    })
    .unwrap();

    let found_in_catalog = pump_until(&mut app, Duration::from_secs(4), |cx| {
        let bw = shell_b.read(cx).browser_workspace().unwrap().read(cx);
        bw.store
            .catalog_apps
            .iter()
            .any(|a| a.app_id == "m4b-store-x-broadcaster")
    })
    .await;
    assert!(found_in_catalog, "Client B must discover Alice's published app");

    // Client B installs the app
    app.update(|cx| {
        shell_b.update(cx, |s, scx| {
            s.browser_install_store_app("m4b-store-x-broadcaster", scx);
        });
    })
    .unwrap();

    let installed_ok = pump_until(&mut app, Duration::from_secs(4), |cx| {
        let bw = shell_b.read(cx).browser_workspace().unwrap().read(cx);
        bw.store
            .installed_apps
            .iter()
            .any(|a| a.app_id == "m4b-store-x-broadcaster")
    })
    .await;
    assert!(installed_ok, "Client B must install app into StorePanel & LocalStore");

    // Verify offline LocalStore SQLite persistence for Client B
    let bob_reopened = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            ..Default::default()
        },
        bob_dir.path(),
    )
    .unwrap();
    let cached = bob_reopened.list_installed_automation_apps_cached();
    assert!(
        cached.iter().any(|a| a.app_id == "m4b-store-x-broadcaster"),
        "Installed app must persist in Bob's LocalStore across reload"
    );
    drop(bob_reopened);

    // Client B runs the installed Store app zero-LLM on its BrowserWorkspace
    let installed_entry = app
        .update(|cx| {
            shell_b
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .store
                .installed_apps
                .iter()
                .find(|a| a.app_id == "m4b-store-x-broadcaster")
                .cloned()
                .unwrap()
        })
        .unwrap();

    app.update(|cx| {
        let bw = shell_b.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.recorder
                .param_inputs
                .insert("message".to_string(), "Bob replayed from Store!".to_string());
            w.run_installed_store_app(installed_entry, wcx);
        });
    })
    .unwrap();

    let replayed = pump_until(&mut app, Duration::from_secs(4), |cx| {
        let bw = shell_b.read(cx).browser_workspace().unwrap().read(cx);
        !bw.recorder.is_replaying
            && bw
                .recorder
                .last_replay_report
                .as_ref()
                .is_some_and(|r| r.app_id == "m4b-store-x-broadcaster")
    })
    .await;
    assert!(replayed, "Client B installed Store app replay must succeed");

    app.update(|cx| {
        let bw = shell_b.read(cx).browser_workspace().unwrap().read(cx);
        let rep = bw.recorder.last_replay_report.as_ref().unwrap();
        assert!(rep.success);
        assert!(!rep.used_llm);
        assert_eq!(
            backend_b.posted_tweet_text.lock().unwrap().as_deref(),
            Some("Bob replayed from Store!")
        );
    })
    .unwrap();

    // Verify server recorded run_count >= 1
    let detail_ok = pump_until(&mut app, Duration::from_secs(3), |_| {
        rt_handle
            .block_on(client_b.get_automation_app("m4b-store-x-broadcaster"))
            .is_ok_and(|d| d.run_count >= 1 && d.install_count >= 1)
    })
    .await;
    assert!(detail_ok, "Server install_count and run_count must increment");

    app.update(|_cx| drop((shell_a, shell_b))).unwrap();
    drop(web);
    drop(server);
    drop((client_a, client_b, rt_client));
}

// ============================================================================
// Test 8: Regression — >20ms Redirect/Error/History Sync & Single-Flight Nav
// ============================================================================

struct AdversarialNavBackend {
    url: Mutex<String>,
    title: Mutex<String>,
    history: Mutex<Vec<String>>,
    nav_log: Mutex<Vec<String>>,
    frame_seq: AtomicU64,
}

impl AdversarialNavBackend {
    fn new(initial_url: &str) -> Arc<Self> {
        Arc::new(Self {
            url: Mutex::new(initial_url.to_string()),
            title: Mutex::new("Initial".to_string()),
            history: Mutex::new(vec![initial_url.to_string()]),
            nav_log: Mutex::new(Vec::new()),
            frame_seq: AtomicU64::new(1),
        })
    }

    fn current_url_sync(&self) -> String {
        self.url.lock().unwrap().clone()
    }
}

impl BrowserSessionBackend for AdversarialNavBackend {
    fn backend_kind(&self) -> BrowserBackendKind {
        BrowserBackendKind::MockCdp
    }

    fn navigate<'life0, 'life1, 'async_trait>(
        &'life0 self,
        url: &'life1 str,
    ) -> MockFuture<'async_trait, Result<NavigationResult, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let delay_ms = match url {
                "https://redirect.example" => 100,
                "https://fail-slow.example" => 80,
                "https://fail-fast.example" => 10,
                "https://slow0.example" => 140,
                "https://slow1.example" => 140,
                "https://fast2.example" => 10,
                _ => 30,
            };
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if url == "https://fail-slow.example" || url == "https://fail-fast.example" {
                return Err(BotBrowserError::Cdp(
                    format!("simulated nav failure for {url}"),
                ));
            }
            let (final_url, title) = if url == "https://redirect.example" {
                (
                    "https://redirected.example/final".to_string(),
                    "Redirected Final".to_string(),
                )
            } else {
                (url.to_string(), format!("Title({url})"))
            };
            *self.url.lock().unwrap() = final_url.clone();
            *self.title.lock().unwrap() = title.clone();
            self.history.lock().unwrap().push(final_url.clone());
            self.nav_log.lock().unwrap().push(final_url.clone());
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(NavigationResult {
                url: final_url.clone(),
                origin: final_url,
                title,
                duration_ms: delay_ms,
                backend: BrowserBackendKind::MockCdp,
            })
        })
    }

    fn go_back<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let mut hist = self.history.lock().unwrap();
            if hist.len() > 1 {
                hist.pop();
                if let Some(prev) = hist.last().cloned() {
                    *self.url.lock().unwrap() = prev.clone();
                    *self.title.lock().unwrap() = format!("Title({prev})");
                }
            }
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn go_forward<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn reload<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(35)).await;
            *self.title.lock().unwrap() = "Reloaded Title".to_string();
            self.frame_seq.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn current_url<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(self.url.lock().unwrap().clone()) })
    }

    fn page_title<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(self.title.lock().unwrap().clone()) })
    }

    fn snapshot_dom<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<BrowserSnapshot, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Ok(BrowserSnapshot {
                url: self.url.lock().unwrap().clone(),
                origin: self.url.lock().unwrap().clone(),
                title: self.title.lock().unwrap().clone(),
                text_excerpt: String::new(),
                interactive_elements: Vec::new(),
                redacted_field_count: 0,
            })
        })
    }

    fn eval_js<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _script: &'life1 str,
    ) -> MockFuture<'async_trait, Result<serde_json::Value, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(serde_json::Value::Null) })
    }

    fn click_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _locator: &'life1 SemanticLocator,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Ok(ActionOutcome {
                ok: true,
                operation: "click".to_string(),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn fill_locator<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        _locator: &'life1 SemanticLocator,
        _value: &'life2 str,
        _is_secret: bool,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Ok(ActionOutcome {
                ok: true,
                operation: "fill".to_string(),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn press_key<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _key: &'life1 str,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Ok(ActionOutcome {
                ok: true,
                operation: "press_key".to_string(),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn scroll<'life0, 'async_trait>(
        &'life0 self,
        _delta_x: f64,
        _delta_y: f64,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Ok(ActionOutcome {
                ok: true,
                operation: "scroll".to_string(),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
        })
    }

    fn wait_for_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
        _timeout_ms: u64,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        let test_id = locator.test_id.clone();
        Box::pin(async move {
            match test_id.as_deref() {
                Some("slow-wait-fail") => {
                    tokio::time::sleep(Duration::from_millis(160)).await;
                    Err(BotBrowserError::Cdp("slow-wait-fail timed out".to_string()))
                }
                Some("slow-wait-ok") => {
                    tokio::time::sleep(Duration::from_millis(160)).await;
                    Ok(ActionOutcome {
                        ok: true,
                        operation: "wait_for".to_string(),
                        duration_ms: 160,
                        matched_strategy: Some("test_id".to_string()),
                        matched_selector: Some("slow-wait-ok".to_string()),
                        x: None,
                        y: None,
                        extracted_text: None,
                    })
                }
                _ => Ok(ActionOutcome {
                    ok: true,
                    operation: "wait_for".to_string(),
                    duration_ms: 1,
                    matched_strategy: None,
                    matched_selector: None,
                    x: None,
                    y: None,
                    extracted_text: None,
                }),
            }
        })
    }

    fn capture_screenshot<'life0, 'async_trait>(
        &'life0 self,
    ) -> MockFuture<'async_trait, Result<Vec<u8>, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let seq = self.frame_seq.load(Ordering::Relaxed);
            Ok(render_png_frame([((seq * 31) % 255) as u8, 120, 200, 255]))
        })
    }

    fn export_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        _domain_filter: Option<&'life1 str>,
    ) -> MockFuture<'async_trait, Result<Vec<BrowserCookie>, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn import_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        cookies: &'life1 [BrowserCookie],
    ) -> MockFuture<'async_trait, Result<usize, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(cookies.len()) })
    }

    fn dispatch_pointer_event<'life0, 'async_trait>(
        &'life0 self,
        _event: PointerInputEvent,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(()) })
    }

    fn dispatch_key_event<'life0, 'async_trait>(
        &'life0 self,
        _event: KeyInputEvent,
    ) -> MockFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(()) })
    }
}

#[test]
fn test_m4b_remediation_slow_redirect_error_and_single_flight_nav() {
    run_in_headless_subprocess("test_m4b_remediation_slow_redirect_error_and_single_flight_nav");
}

async fn body_remediation_slow_redirect_error_and_single_flight_nav(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 108, "grace"))
        .unwrap();

    let backend = AdversarialNavBackend::new("https://start.example");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    // Wait for initial frame
    let ready = pump_until(&mut app, Duration::from_secs(2), |cx| {
        shell
            .read(cx)
            .browser_workspace()
            .and_then(|bw| bw.read(cx).cdp_surface().cloned())
            .is_some_and(|s| !s.read(cx).is_refreshing())
    })
    .await;
    assert!(ready);

    // 1. >20ms redirecting navigation (100ms) syncs final URL, url_input, title, and is_loading == false
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_navigate("https://redirect.example", scx);
        });
    })
    .unwrap();

    let redirected = pump_until(&mut app, Duration::from_secs(2), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.is_loading && bw.current_url == "https://redirected.example/final"
    })
    .await;
    assert!(redirected, "BrowserWorkspace must sync redirected URL after >20ms nav");
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(bw.url_input, "https://redirected.example/final");
        assert_eq!(bw.page_title, "Redirected Final");
        assert!(bw.last_error.is_none());
        assert!(!bw.is_loading);
    })
    .unwrap();

    // 1b. History actions (go_back & reload) >20ms also sync BrowserWorkspace URL/title/is_loading
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.go_back(wcx);
        });
    })
    .unwrap();
    let backed = pump_until(&mut app, Duration::from_secs(2), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.is_loading && bw.current_url == "https://start.example"
    })
    .await;
    assert!(backed, "go_back must sync BrowserWorkspace.current_url");

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.reload(wcx);
        });
    })
    .unwrap();
    let reloaded = pump_until(&mut app, Duration::from_secs(2), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.is_loading && bw.page_title == "Reloaded Title"
    })
    .await;
    assert!(reloaded, "reload must sync BrowserWorkspace.page_title");

    // 1c. >20ms failing navigation (80ms) clears is_loading and surfaces last_error
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_navigate("https://fail-slow.example", scx);
        });
    })
    .unwrap();
    let failed_synced = pump_until(&mut app, Duration::from_secs(2), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        !bw.is_loading && bw.last_error.is_some()
    })
    .await;
    assert!(failed_synced, "Slow nav failure must clear is_loading and set last_error");
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure"),
            "Unexpected error: {:?}",
            bw.last_error
        );
    })
    .unwrap();

    // 2. Back-to-back navigate("slow1") then navigate("fast2") in the same turn
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow1.example", wcx);
            w.navigate("https://fast2.example", wcx);
        });
    })
    .unwrap();

    let settled_same_turn = pump_until(&mut app, Duration::from_secs(2), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading
            && !sr.is_refreshing()
            && bw.current_url == "https://fast2.example"
            && sr.current_url() == "https://fast2.example"
    })
    .await;
    assert!(settled_same_turn);
    // Sleep past slow1 duration to prove slow1 never overwrites backend later
    app.background_executor()
        .timer(Duration::from_millis(200))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(backend.current_url_sync(), "https://fast2.example");
        assert_eq!(bw.current_url, "https://fast2.example");
        assert_eq!(sr.current_url(), "https://fast2.example");
    })
    .unwrap();

    // 2b. Staggered in-flight back-to-back: slow1 starts executing, 20ms later fast2 is requested
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow1.example", wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://fast2.example", wcx);
        });
    })
    .unwrap();

    let settled_staggered = pump_until(&mut app, Duration::from_secs(2), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading
            && !sr.is_refreshing()
            && bw.current_url == "https://fast2.example"
            && sr.current_url() == "https://fast2.example"
    })
    .await;
    assert!(settled_staggered);
    app.background_executor()
        .timer(Duration::from_millis(180))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(backend.current_url_sync(), "https://fast2.example");
        assert_eq!(bw.current_url, "https://fast2.example");
        assert_eq!(sr.current_url(), "https://fast2.example");
    })
    .unwrap();

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}



// ============================================================================
// Test 9: Regression — IME Precedence, Multiline Paste, Viewport Blur,
//         contenteditable Coalescing, Host Replace & Cmd+, Immediate Hide
// ============================================================================

#[test]
fn test_m4b_remediation_ime_multiline_viewport_dom_coalesce_host_and_cmd_comma() {
    run_in_headless_subprocess(
        "test_m4b_remediation_ime_multiline_viewport_dom_coalesce_host_and_cmd_comma",
    );
}

async fn body_remediation_ime_multiline_viewport_dom_coalesce_host_and_cmd_comma(
    mut app: AsyncApp,
) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 109, "heidi"))
        .unwrap();

    let backend = HttpBackedMockBrowserBackend::new("https://x.com");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            // Open BrowserWorkspace BEFORE attaching host1
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    // 1. IME split-brain consistency (`BrowserWorkspace` vs `ChatSidebar` search)
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.sidebar.update(scx, |sb, _| sb.focus_search());
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
                w.clear_active_input(wcx);
            });
            assert_eq!(s.active_text_and_preedit(scx).0, "");
            s.insert_committed_text("https://x.com", scx);
            assert_eq!(bw.read(scx).url_input, "https://x.com");
            assert_eq!(s.active_text_and_preedit(scx).0, "https://x.com");
            assert_eq!(
                s.sidebar.read(scx).search_query,
                "",
                "Sidebar search must not steal text when BrowserWorkspace input is active"
            );
            s.clear_active_field(scx);
            assert_eq!(bw.read(scx).url_input, "");
        });
    })
    .unwrap();

    // 2. Multiline paste preserving `\n` in CopilotPrompt and CookieImport
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::CopilotPrompt, None, wcx);
            });
            s.insert_committed_text("line1\nline2", scx);
            assert_eq!(bw.read(scx).recorder.copilot_prompt_draft, "line1\nline2");

            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::CookieImport, None, wcx);
            });
            let netscape = ".x.com\tTRUE\t/\tTRUE\t1893456000\tauth_token\tv1\n.x.com\tTRUE\t/\tTRUE\t1893456000\tct0\tv2";
            s.insert_committed_text(netscape, scx);
            assert_eq!(bw.read(scx).recorder.cookie_import_draft, netscape);

            // Clearing/blurring active_input prevents subsequent typing from modifying URL/form inputs
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
                w.clear_active_input(wcx);
                w.set_active_input(ActiveBrowserInput::None, None, wcx);
            });
            assert!(!s.is_browser_workspace_input_active(scx));
            s.insert_committed_text("should-not-hit-address-bar", scx);
            assert_eq!(bw.read(scx).url_input, "");
        });
    })
    .unwrap();

    // 3. contenteditable RecordedDomEvent coalescing with mutating text_anchor (macOS)
    #[cfg(target_os = "macos")]
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.recorder.is_dom_recording = true;
                for partial in ["H", "He", "Hello"] {
                    let payload = serde_json::json!({
                        "event_kind": "input",
                        "locator": {
                            "test_id": "tweetTextarea_0",
                            "aria_label": "Post text",
                            "role": "textbox",
                            "text_anchor": partial,
                            "css_selector": "div[data-testid=\"tweetTextarea_0\"]",
                            "name_attr": null
                        },
                        "value": partial,
                        "is_sensitive": false,
                        "page_url": "https://x.com/compose/post",
                        "timestamp_ms": 1700000000000i64
                    });
                    let ev = parse_recorded_dom_event(&payload).expect("parse RecordedDomEvent");
                    w.process_recorded_dom_event(ev);
                }
                // Toggling DOM recording OFF flushes pending_dom_fill cleanly
                w.toggle_dom_recording(wcx);
                assert!(!w.recorder.is_dom_recording);
                let app_distilled = w
                    .distill_last_trace("ContentEditable Test", "Coalesce test", "social", wcx)
                    .expect("distill coalesced contenteditable fill");
                let fill_steps: Vec<_> = app_distilled
                    .steps
                    .iter()
                    .filter(|st| matches!(st, ScriptStep::Fill { .. }))
                    .collect();
                assert_eq!(
                    fill_steps.len(),
                    1,
                    "Expected 3 mutating-text_anchor contenteditable inputs to coalesce into 1 Fill step, got {:?}",
                    app_distilled.steps
                );
            });
        });
    })
    .unwrap();

    // 4. attach_local_fabushi_host(host1) then host2 updating already-open BrowserWorkspace
    let (host1, host2) = {
        let _rt = client.runtime().enter();
        (
            Arc::new(
                FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("create host1"),
            ),
            Arc::new(
                FabushiBotHost::new_unified_ephemeral("hermes-local-v1").expect("create host2"),
            ),
        )
    };
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.attach_local_fabushi_host(Arc::clone(&host1));
            let bw = s.browser_workspace().unwrap().clone();
            assert!(
                bw.read(scx)
                    .effective_local_fabushi()
                    .is_some_and(|h| Arc::ptr_eq(&h, &host1)),
                "BrowserWorkspace must immediately resolve host1 after attach_local_fabushi_host"
            );
            s.attach_local_fabushi_host(Arc::clone(&host2));
            assert!(
                bw.read(scx)
                    .effective_local_fabushi()
                    .is_some_and(|h| Arc::ptr_eq(&h, &host2)),
                "BrowserWorkspace must immediately resolve host2 after host replacement"
            );
            s.attach_local_fabushi_host_with_cx(Arc::clone(&host2), scx);
            assert!(
                bw.read(scx)
                    .local_fabushi
                    .as_ref()
                    .is_some_and(|h| Arc::ptr_eq(h, &host2)),
                "attach_local_fabushi_host_with_cx must update bw.local_fabushi directly"
            );
        });
    })
    .unwrap();

    // 5. Cmd+, (`handle_shortcut_key(",", false, cx)`) immediately hides/occludes BrowserWorkspace
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            // Skip unrelated startup magpie discovery spawn in reactor-free headless test
            s.magpie_local.bootstrapped_auth_generation = Some(s.session_sync.auth_generation);
            let bw = s.open_browser_workspace(scx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            assert!(bw.read(scx).is_workspace_visible());
            assert!(!bw.read(scx).is_externally_occluded());

            assert!(s.handle_shortcut_key(",", false, scx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
            assert!(
                !bw.read(scx).is_workspace_visible(),
                "Cmd+, must immediately set workspace_visible = false"
            );
            assert!(
                bw.read(scx).is_externally_occluded(),
                "Cmd+, must immediately set external_occluded = true"
            );
        });
    })
    .unwrap();

    // 6. RecorderPanelState::push_log monotonic seq past 250 + cookie_status_message clear on error
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                for i in 0..260 {
                    w.recorder
                        .push_log(BrowserLogLevel::Info, format!("Log {i}"), "", None);
                }
                assert_eq!(w.recorder.event_logs.len(), 250);
                assert!(w.recorder.event_logs.last().unwrap().seq >= 260);

                w.import_cookies_from_text_async(
                    r#"[{"name":"c1","value":"v1","domain":".x.com","path":"/"}]"#.to_string(),
                    wcx,
                );
            });
        });
    })
    .unwrap();
    let ok_imported = pump_until(&mut app, Duration::from_secs(2), |cx| {
        shell
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .recorder
            .cookie_status_message
            .is_some()
    })
    .await;
    assert!(ok_imported);

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.import_cookies_from_text_async("{not valid json".to_string(), wcx);
        });
    })
    .unwrap();
    let err_cleared = pump_until(&mut app, Duration::from_secs(2), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        bw.recorder.cookie_error_message.is_some()
    })
    .await;
    assert!(err_cleared);
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert!(
            bw.recorder.cookie_status_message.is_none(),
            "Failed cookie import must clear stale cookie_status_message"
        );
    })
    .unwrap();

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}


// ============================================================================
// Test 10: Regression — Repeated-URL Single-Flight Nav & AI Collision
// ============================================================================

#[test]
fn test_m4b_regression_single_flight_repeat_url_and_ai_collision() {
    run_in_headless_subprocess("test_m4b_regression_single_flight_repeat_url_and_ai_collision");
}

async fn body_regression_single_flight_repeat_url_and_ai_collision(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 110, "iris"))
        .unwrap();

    let backend = AdversarialNavBackend::new("https://start.example");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    let ready = pump_until(&mut app, Duration::from_secs(2), |cx| {
        shell
            .read(cx)
            .browser_workspace()
            .and_then(|bw| bw.read(cx).cdp_surface().cloned())
            .is_some_and(|s| !s.read(cx).is_refreshing())
    })
    .await;
    assert!(ready);

    // 1. Repeated-URL sequence: slow0 -> a -> b -> a must settle on "https://a.example" everywhere
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow0.example", wcx);
            w.navigate("https://a.example", wcx);
            w.navigate("https://b.example", wcx);
            w.navigate("https://a.example", wcx);
        });
    })
    .unwrap();

    let settled_repeat = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading
            && !sr.is_refreshing()
            && bw.current_url == "https://a.example"
            && sr.current_url() == "https://a.example"
    })
    .await;
    assert!(settled_repeat, "slow0 -> a -> b -> a must settle on a.example");
    app.background_executor()
        .timer(Duration::from_millis(200))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(backend.current_url_sync(), "https://a.example");
        assert_eq!(bw.current_url, "https://a.example");
        assert_eq!(bw.url_input, "https://a.example");
        assert_eq!(sr.current_url(), "https://a.example");
        assert!(bw.last_error.is_none());
    })
    .unwrap();

    // 2. slow1 -> go_back returns cleanly to previous history URL ("https://a.example")
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow1.example", wcx);
            w.go_back(wcx);
        });
    })
    .unwrap();
    let settled_back = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading
            && !sr.is_refreshing()
            && bw.current_url == "https://a.example"
            && sr.current_url() == "https://a.example"
    })
    .await;
    assert!(settled_back, "slow1 -> go_back must return to a.example");
    assert_eq!(backend.current_url_sync(), "https://a.example");

    // 3. slow1 (ok) -> fail-fast keeps slow1 URL & url_input and surfaces failure
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow1.example", wcx);
            w.navigate("https://fail-fast.example", wcx);
        });
    })
    .unwrap();
    let settled_fail = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading && !sr.is_refreshing() && bw.last_error.is_some()
    })
    .await;
    assert!(settled_fail, "slow1 -> fail-fast must surface error after settling");
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(bw.current_url, "https://slow1.example");
        assert_eq!(bw.url_input, "https://slow1.example");
        assert_eq!(backend.current_url_sync(), "https://slow1.example");
        assert!(bw
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("simulated nav failure for https://fail-fast.example"));
    })
    .unwrap();

    // 4. AI/Script direct backend.navigate("https://a.example") while UI has ["https://a.example", "https://b.example"] queued
    //    must NEVER be skipped by SingleFlightNavBackend!
    backend.nav_log.lock().unwrap().clear();
    let wrapped_backend = app
        .update(|cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().clone();
            bw.update(cx, |w, wcx| {
                w.navigate("https://a.example", wcx);
                w.navigate("https://b.example", wcx);
                w.active_backend().cloned().expect("active_backend present")
            })
        })
        .unwrap();

    let rt_handle = client.runtime().handle().clone();
    let ai_nav_res = rt_handle
        .block_on(async move { wrapped_backend.navigate("https://a.example").await })
        .expect("AI navigate must succeed");
    assert_eq!(
        ai_nav_res.url, "https://a.example",
        "AI navigate(\"https://a.example\") must NOT be skipped when colliding with UI queue"
    );

    let settled_ai = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading
            && !sr.is_refreshing()
            && bw.current_url == "https://b.example"
            && sr.current_url() == "https://b.example"
    })
    .await;
    assert!(settled_ai, "UI must settle on b.example after AI collision");
    assert_eq!(backend.current_url_sync(), "https://b.example");
    let log = backend.nav_log.lock().unwrap().clone();
    assert!(
        log.iter().filter(|u| u.as_str() == "https://a.example").count() >= 2,
        "Expected both UI and AI navigations to a.example to execute, log={log:?}"
    );

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}


// ============================================================================
// Test 11: Regression — IME Preedit Backspace/Enter, Tab Leakage &
//          CdpViewportSurface Window Key-Bubbling
// ============================================================================

#[test]
fn test_m4b_regression_ime_preedit_tab_and_cdp_viewport_key_bubbling() {
    run_in_headless_subprocess_with_window(
        "test_m4b_regression_ime_preedit_tab_and_cdp_viewport_key_bubbling",
    );
}

async fn body_regression_ime_preedit_tab_and_cdp_viewport_key_bubbling(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");

    // Part A: IME preedit Backspace/Enter + Tab in BrowserWorkspace (without Window)
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 111, "ivan"))
        .unwrap();
    let backend = HttpBackedMockBrowserBackend::new("https://x.com");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.magpie_local.bootstrapped_auth_generation = Some(s.session_sync.auth_generation);
            s.set_browser_backend_for_test(backend.clone(), scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
                w.clear_active_input(wcx);
            });
            s.insert_committed_text("https://x.com", scx);
            assert_eq!(bw.read(scx).url_input, "https://x.com");

            // IME preedit "ni" + Backspace clears preedit without deleting committed url_input
            s.update_ime_marked_text("ni", scx);
            assert_eq!(
                s.active_text_and_preedit(scx),
                ("https://x.com".to_string(), Some("ni".to_string()))
            );
            assert!(s.handle_non_char_key("backspace", false, scx));
            assert_eq!(bw.read(scx).url_input, "https://x.com");
            assert_eq!(s.active_text_and_preedit(scx).1, None);

            // IME preedit "hao" + Enter commits preedit without triggering navigate()
            s.update_ime_marked_text("hao", scx);
            assert!(s.handle_non_char_key("enter", false, scx));
            assert_eq!(bw.read(scx).url_input, "https://x.comhao");
            assert_eq!(s.active_text_and_preedit(scx).1, None);
            assert!(!bw.read(scx).is_loading, "Enter with IME preedit must commit, not navigate");

            // Tab while BrowserWorkspace input is active must NOT leak to sidebar search or composer
            assert!(!s.sidebar.read(scx).search_focused);
            assert!(!s.composer.read(scx).is_input_focused);
            assert!(s.handle_non_char_key("tab", false, scx));
            assert!(!s.sidebar.read(scx).search_focused);
            assert!(!s.composer.read(scx).is_input_focused);

            // Tab cycles Distill form inputs internally without leaking
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::DistillName, None, wcx);
            });
            assert!(s.handle_non_char_key("tab", false, scx));
            assert_eq!(bw.read(scx).active_input, ActiveBrowserInput::DistillVersion);
            assert!(!s.sidebar.read(scx).search_focused);
        });
    })
    .unwrap();
    app.update(|_cx| drop(shell)).unwrap();

    // Part B: Focused CdpViewportSurface in a real headless Window must NOT bubble Tab/Escape
    let backend2 = HttpBackedMockBrowserBackend::new("https://x.com");
    let win_handle = app
        .update(|cx| {
            let _rt_guard = client.runtime().enter();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let mut s = AppShell::new_with_client(client.clone(), cx);
                        s.is_authenticated = true;
                        s.session_sync.user_id = Some(112);
                        s.session_sync.username = Some("judy".to_string());
                        s.session_sync.session_token = Some("tok_judy".to_string());
                        s.magpie_local.bootstrapped_auth_generation =
                            Some(s.session_sync.auth_generation);
                        s.set_browser_backend_for_test(backend2.clone(), cx);
                        let surf = s
                            .browser_workspace()
                            .unwrap()
                            .read(cx)
                            .cdp_surface()
                            .unwrap()
                            .clone();
                        surf.update(cx, |sr, scx| {
                            sr.focus_handle(scx).focus(window);
                        });
                        s
                    })
                },
            )
            .expect("open headless window")
        })
        .unwrap();

    let shell_ent = win_handle.entity(&app).unwrap();

    // Draw window so element tree + focus dispatch tree are populated
    app.update_window(win_handle.into(), |_, window, cx| {
        let _ = window.draw(cx);
    })
    .unwrap();

    app.update_window(win_handle.into(), |_, window, cx| {
        let s = shell_ent.read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
        assert!(
            s.is_browser_cdp_viewport_focused(window, cx),
            "CdpViewportSurface must retain GPUI focus across render"
        );
        window.dispatch_keystroke(Keystroke::parse("a").unwrap(), cx);
        window.dispatch_keystroke(Keystroke::parse("tab").unwrap(), cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
    })
    .unwrap();

    app.update(|cx| {
        let s = shell_ent.read(cx);
        assert!(
            !s.sidebar.read(cx).search_focused,
            "Tab inside focused CdpViewportSurface must NOT focus sidebar search"
        );
        assert!(
            !s.composer.read(cx).is_input_focused,
            "Tab inside focused CdpViewportSurface must NOT focus composer"
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::Browser,
            "Escape inside focused CdpViewportSurface must NOT exit BrowserWorkspace"
        );
    })
    .unwrap();

    let forwarded = pump_until(&mut app, Duration::from_secs(2), |_| {
        let keys: Vec<String> = backend2
            .key_events
            .lock()
            .unwrap()
            .iter()
            .map(|k| k.key.clone())
            .collect();
        keys.iter().any(|k| k == "a")
            && keys.iter().any(|k| k == "tab")
            && keys.iter().any(|k| k == "escape")
    })
    .await;
    assert!(
        forwarded,
        "a, tab, and escape must still forward to backend, got {:?}",
        backend2.key_events.lock().unwrap()
    );

    // Cmd+K and Cmd+1 platform shortcuts work while CdpViewportSurface is focused
    // and must NOT leak "k" or "1" (or ",") into backend.key_events
    app.update_window(win_handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("cmd-9").unwrap(), cx);
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.focus_handle(scx).focus(window);
        });
        let _ = window.draw(cx);
        window.dispatch_keystroke(Keystroke::parse("cmd-k").unwrap(), cx);
    })
    .unwrap();
    app.update(|cx| {
        let s = shell_ent.read(cx);
        assert!(
            s.sidebar.read(cx).search_focused,
            "Cmd+K shortcut must focus sidebar search when CdpViewportSurface is focused"
        );
    })
    .unwrap();

    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.sidebar.update(scx, |sb, _| sb.search_focused = false);
        });
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.focus_handle(scx).focus(window);
        });
        let _ = window.draw(cx);
        window.dispatch_keystroke(Keystroke::parse("cmd-1").unwrap(), cx);
    })
    .unwrap();
    app.update(|cx| {
        assert_eq!(
            shell_ent.read(cx).active_panel,
            ActiveWorkspacePanel::Chats,
            "Cmd+1 shortcut must switch to Chats when CdpViewportSurface is focused"
        );
    })
    .unwrap();

    // Cmd+, platform shortcut still works even while CdpViewportSurface is focused
    app.update_window(win_handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("cmd-9").unwrap(), cx);
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.focus_handle(scx).focus(window);
        });
        let _ = window.draw(cx);
        window.dispatch_keystroke(Keystroke::parse("cmd-,").unwrap(), cx);
    })
    .unwrap();
    app.update(|cx| {
        assert_eq!(
            shell_ent.read(cx).active_panel,
            ActiveWorkspacePanel::Settings,
            "Cmd+, shortcut must still exit Browser to Settings when CdpViewportSurface is focused"
        );
    })
    .unwrap();

    app.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    {
        let leaked: Vec<String> = backend2
            .key_events
            .lock()
            .unwrap()
            .iter()
            .map(|k| k.key.clone())
            .collect();
        assert!(
            !leaked.iter().any(|k| k == "k" || k == "1" || k == "," || k == "9"),
            "Platform shortcuts (Cmd+K, Cmd+1, Cmd+9, Cmd+,) must NOT leak to backend.key_events, got {leaked:?}"
        );
    }

    drop(client);
}

// ============================================================================
// Test 12: Regression — refresh_frame vs navigate_and_refresh Race &
//          Script/AI Replay URL Sync (Challenger 5 Findings 1 & 2)
// ============================================================================

#[test]
fn test_m4b_regression_refresh_frame_nav_race_and_replay_url_sync() {
    run_in_headless_subprocess("test_m4b_regression_refresh_frame_nav_race_and_replay_url_sync");
}

async fn body_regression_refresh_frame_nav_race_and_replay_url_sync(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 113, "kate"))
        .unwrap();

    let backend = AdversarialNavBackend::new("https://start.example");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    let ready = pump_until(&mut app, Duration::from_secs(2), |cx| {
        shell
            .read(cx)
            .browser_workspace()
            .and_then(|bw| bw.read(cx).cdp_surface().cloned())
            .is_some_and(|s| !s.read(cx).is_refreshing())
    })
    .await;
    assert!(ready);

    // 1. Challenge 1A: slow1 navigate + mid-nav user_take_over() -> must settle on slow1.example
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow1.example", wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.user_take_over(wcx);
        });
    })
    .unwrap();

    let settled_takeover = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading && !sr.is_refreshing() && bw.current_url == "https://slow1.example"
    })
    .await;
    assert!(
        settled_takeover,
        "slow1 + user_take_over mid-nav must settle on slow1.example"
    );
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(bw.current_url, "https://slow1.example");
        assert_eq!(bw.url_input, "https://slow1.example");
        assert_eq!(sr.current_url(), "https://slow1.example");
        assert_eq!(backend.current_url_sync(), "https://slow1.example");
    })
    .unwrap();

    // 2. Challenge 1B: settle on a.example, then slow1 navigate + mid-nav handle_click() -> must settle on slow1.example
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://a.example", wcx);
        });
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading && !sr.is_refreshing() && bw.current_url == "https://a.example"
        })
        .await
    );

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow1.example", wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    app.update(|cx| {
        let surf = shell
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |s, scx| {
            s.handle_click(42.0, 64.0, bot_browser::PointerButton::Left, 1, 0, scx);
        });
    })
    .unwrap();

    let settled_click = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading && !sr.is_refreshing() && bw.current_url == "https://slow1.example"
    })
    .await;
    assert!(
        settled_click,
        "slow1 + handle_click mid-nav must settle on slow1.example"
    );
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(bw.current_url, "https://slow1.example");
        assert_eq!(bw.url_input, "https://slow1.example");
        assert_eq!(sr.current_url(), "https://slow1.example");
        assert_eq!(backend.current_url_sync(), "https://slow1.example");
    })
    .unwrap();

    // 3. Challenge 2A: browser_run_script_app after prior UI nav ("https://b.example")
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://b.example", wcx);
        });
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading && !sr.is_refreshing() && bw.current_url == "https://b.example"
        })
        .await
    );

    let ai_app = bot_browser::AutomationScriptApp {
        schema_version: 1,
        app_id: "ai-replay-sync".to_string(),
        slug: "ai-replay-sync".to_string(),
        name: "AI Replay Sync".to_string(),
        version: "1.0.0".to_string(),
        description: "Navigate to ai-target".to_string(),
        category: "general".to_string(),
        target_domains: vec!["ai-target.example".to_string()],
        parameters: Vec::new(),
        steps: vec![bot_browser::ScriptStep::Navigate {
            url_template: "https://ai-target.example".to_string(),
        }],
        required_capabilities: Vec::new(),
        author_id: None,
        source_trace_summary: None,
    };
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.browser_run_script_app(ai_app.clone(), std::collections::BTreeMap::new(), scx);
        });
    })
    .unwrap();

    let settled_replay = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.recorder.is_replaying
            && !sr.is_refreshing()
            && bw.current_url == "https://ai-target.example"
            && sr.current_url() == "https://ai-target.example"
    })
    .await;
    assert!(
        settled_replay,
        "browser_run_script_app must sync bw, url_input, and surf to ai-target.example"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(bw.current_url, "https://ai-target.example");
        assert_eq!(bw.url_input, "https://ai-target.example");
        assert_eq!(sr.current_url(), "https://ai-target.example");
        assert_eq!(backend.current_url_sync(), "https://ai-target.example");
    })
    .unwrap();

    // 4. Challenge 2B: direct registry.replay_script after prior UI nav ("https://b.example")
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://b.example", wcx);
        });
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading && !sr.is_refreshing() && bw.current_url == "https://b.example"
        })
        .await
    );

    let (registry, sid) = app
        .update(|cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            (bw.session_registry.clone(), bw.active_session_id.clone())
        })
        .unwrap();
    let ai_reg_app = bot_browser::AutomationScriptApp {
        app_id: "ai-reg-sync".to_string(),
        slug: "ai-reg-sync".to_string(),
        name: "AI Registry Sync".to_string(),
        target_domains: vec!["ai-reg.example".to_string()],
        steps: vec![bot_browser::ScriptStep::Navigate {
            url_template: "https://ai-reg.example".to_string(),
        }],
        ..ai_app.clone()
    };
    let rt_handle = client.runtime().handle().clone();
    let reg_clone = registry.clone();
    let sid_clone = sid.clone();
    let reg_report = rt_handle
        .block_on(async move {
            reg_clone
                .replay_script(
                    &sid_clone,
                    &ai_reg_app,
                    &std::collections::BTreeMap::new(),
                    None,
                )
                .await
        })
        .expect("direct registry replay_script succeeds");
    assert!(reg_report.success);

    let settled_reg = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        matches!(bw.control_mode(), BrowserControlMode::Idle)
            && !sr.is_refreshing()
            && bw.current_url == "https://ai-reg.example"
            && sr.current_url() == "https://ai-reg.example"
    })
    .await;
    assert!(
        settled_reg,
        "direct registry.replay_script must sync bw, url_input, and surf to ai-reg.example"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(bw.current_url, "https://ai-reg.example");
        assert_eq!(bw.url_input, "https://ai-reg.example");
        assert_eq!(sr.current_url(), "https://ai-reg.example");
        assert_eq!(backend.current_url_sync(), "https://ai-reg.example");
    })
    .unwrap();

    // 5. Challenge 1C: Concurrent UI queue ["a", "b"] + registry.replay_script("ai-target")
    backend.nav_log.lock().unwrap().clear();
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://a.example", wcx);
            w.navigate("https://b.example", wcx);
        });
    })
    .unwrap();

    let ai_app_concurrent2 = ai_app.clone();
    let registry2 = app
        .update(|cx| {
            shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .session_registry
                .clone()
        })
        .unwrap();
    let sid2 = app
        .update(|cx| {
            shell
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .active_session_id
                .clone()
        })
        .unwrap();

    let ai_join = rt_handle.spawn(async move {
        registry
            .replay_script(&sid, &ai_app, &std::collections::BTreeMap::new(), None)
            .await
    });
    while !ai_join.is_finished() {
        app.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    let ai_res = ai_join.await.expect("join ai replay").expect("ai replay ok");
    assert!(ai_res.success);

    let settled_concurrent = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        let expected = backend.current_url_sync();
        !bw.is_loading
            && !sr.is_refreshing()
            && matches!(bw.control_mode(), BrowserControlMode::Idle)
            && bw.current_url == expected
            && sr.current_url() == expected
    })
    .await;
    assert!(
        settled_concurrent,
        "Concurrent UI queue + registry.replay_script must converge across bw, surf, and backend"
    );
    app.background_executor()
        .timer(Duration::from_millis(150))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        let expected = backend.current_url_sync();
        assert_eq!(bw.current_url, expected);
        assert_eq!(bw.url_input, expected);
        assert_eq!(sr.current_url(), expected);
        let log = backend.nav_log.lock().unwrap().clone();
        assert!(log.iter().any(|u| u == "https://a.example"), "log={log:?}");
        assert!(log.iter().any(|u| u == "https://b.example"), "log={log:?}");
        assert!(
            log.iter().any(|u| u == "https://ai-target.example"),
            "log={log:?}"
        );
    })
    .unwrap();

    // 6. Slow-nav UI queue ["slow1", "b"] + concurrent registry.replay_script("ai-target")
    //    forces StepCompleted/ControlModeChanged refresh_frame to collide mid-flight with slow1/b.
    backend.nav_log.lock().unwrap().clear();
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://slow1.example", wcx);
            w.navigate("https://b.example", wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    let ai_join2 = rt_handle.spawn(async move {
        registry2
            .replay_script(
                &sid2,
                &ai_app_concurrent2,
                &std::collections::BTreeMap::new(),
                None,
            )
            .await
    });
    while !ai_join2.is_finished() {
        app.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    assert!(ai_join2.await.expect("join ai2").expect("ai2 ok").success);

    let settled_slow_concurrent = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        let expected = backend.current_url_sync();
        !bw.is_loading
            && !sr.is_refreshing()
            && matches!(bw.control_mode(), BrowserControlMode::Idle)
            && bw.current_url == expected
            && sr.current_url() == expected
    })
    .await;
    assert!(
        settled_slow_concurrent,
        "Slow UI queue + concurrent registry.replay_script must converge"
    );
    app.background_executor()
        .timer(Duration::from_millis(150))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        let expected = backend.current_url_sync();
        assert_eq!(bw.current_url, expected);
        assert_eq!(bw.url_input, expected);
        assert_eq!(sr.current_url(), expected);
    })
    .unwrap();

    // 7. Challenger 7 Regression A: WaitingForTakeover + failing slow nav + mid-nav user_take_over()
    //    must preserve navigation failure last_error on both BrowserWorkspace and CdpViewportSurface.
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://a.example", wcx);
        });
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && bw.current_url == "https://a.example"
        })
        .await
    );

    let session_for_err = app
        .update(|cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            bw.session_registry
                .get_session(&bw.active_session_id)
                .expect("active automation session present")
        })
        .unwrap();
    let sess_takeover = session_for_err.clone();
    rt_handle
        .block_on(async move {
            sess_takeover
                .request_takeover_and_wait("Need manual verification", false, false)
                .await
        })
        .expect("request_takeover_and_wait succeeds");
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            matches!(bw.control_mode(), BrowserControlMode::WaitingForTakeover { .. })
        })
        .await,
        "Workspace must transition to WaitingForTakeover before starting fail-slow nav"
    );

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://fail-slow.example", wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.user_take_over(wcx);
        });
    })
    .unwrap();

    let settled_takeover_err = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading
            && !sr.is_refreshing()
            && !sr.is_navigating()
            && bw.last_error.is_some()
    })
    .await;
    assert!(
        settled_takeover_err,
        "WaitingForTakeover -> fail-slow + user_take_over must preserve bw.last_error"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert!(!bw.is_loading);
        assert_eq!(bw.current_url, "https://a.example");
        assert_eq!(bw.url_input, "https://a.example");
        assert_eq!(sr.current_url(), "https://a.example");
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Expected bw.last_error to retain fail-slow error, got {:?}",
            bw.last_error
        );
        assert!(
            sr.last_error()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Expected surf.last_error() to retain fail-slow error, got {:?}",
            sr.last_error()
        );
    })
    .unwrap();

    // 8. Challenger 7 Regression B: StepCompleted emitted during in-flight failing slow nav
    //    must preserve navigation failure last_error on both BrowserWorkspace and CdpViewportSurface.
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.user_cancel_ai(wcx);
            w.navigate("https://a.example", wcx);
        });
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && bw.last_error.is_none()
                && sr.last_error().is_none()
                && bw.current_url == "https://a.example"
        })
        .await
    );

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://fail-slow.example", wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    session_for_err.emit_step_completed(
        0,
        "click",
        12,
        None,
        Some("https://a.example".to_string()),
    );

    let settled_step_err = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        !bw.is_loading
            && !sr.is_refreshing()
            && !sr.is_navigating()
            && bw.last_error.is_some()
    })
    .await;
    assert!(
        settled_step_err,
        "StepCompleted during fail-slow nav must preserve bw.last_error"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert!(!bw.is_loading);
        assert_eq!(bw.current_url, "https://a.example");
        assert_eq!(bw.url_input, "https://a.example");
        assert_eq!(sr.current_url(), "https://a.example");
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Expected bw.last_error to retain fail-slow error, got {:?}",
            bw.last_error
        );
        assert!(
            sr.last_error()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Expected surf.last_error() to retain fail-slow error, got {:?}",
            sr.last_error()
        );
        assert!(
            bw.recorder
                .event_logs
                .iter()
                .any(|l| l.title.contains("Step #1 done: click")),
            "Expected StepCompleted event to have been processed during fail-slow nav"
        );
    })
    .unwrap();

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}

// ============================================================================
// Test 13: Regression — Post-Settlement refresh_frame Preserves Failed-Nav
//          last_error & Probe 1F URL Convergence (Challenger 10 Probes 1B/1D/1E/1F/1H)
// ============================================================================

#[test]
fn test_m4b_regression_post_settlement_refresh_and_url_convergence() {
    run_in_headless_subprocess("test_m4b_regression_post_settlement_refresh_and_url_convergence");
}

async fn body_regression_post_settlement_refresh_and_url_convergence(mut app: AsyncApp) {
    use gpui_telegram_proto::automation_apps::InstalledAutomationApp;

    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let shell = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 114, "liam"))
        .unwrap();

    let backend = AdversarialNavBackend::new("https://a.example");
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.set_browser_backend_for_test(backend.clone(), scx);
        });
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !sr.is_refreshing() && sr.current_url() == "https://a.example"
        })
        .await,
        "Initial surface refresh must settle on a.example"
    );

    let make_installed = |app_id: &str, script: &AutomationScriptApp| InstalledAutomationApp {
        app_id: app_id.to_string(),
        slug: app_id.to_string(),
        name: script.name.clone(),
        description: script.description.clone(),
        category: script.category.clone(),
        target_domains: script.target_domains.clone(),
        installed_version: script.version.clone(),
        latest_version: script.version.clone(),
        manifest: serde_json::to_value(script).expect("serialize script manifest"),
        content_sha256: String::new(),
        installed_at_ms: 1700000000000,
        last_run_at_ms: None,
        local_run_count: 0,
    };

    // ------------------------------------------------------------------------
    // Probe 1B: fail-slow UI nav (80ms) + concurrently failing slow-wait-fail
    //           store app (160ms). Post-settlement ControlModeChanged(Idle) /
    //           refresh_frame must NOT wipe fail-slow's last_error.
    // ------------------------------------------------------------------------
    let failing_wait_script = AutomationScriptApp {
        schema_version: 1,
        app_id: "probe-1b-wait-fail".to_string(),
        slug: "probe-1b-wait-fail".to_string(),
        name: "Probe 1B Wait Fail".to_string(),
        version: "1.0.0".to_string(),
        description: "Wait on failing locator".to_string(),
        category: "general".to_string(),
        target_domains: vec!["a.example".to_string()],
        parameters: Vec::new(),
        steps: vec![ScriptStep::WaitFor {
            locator: SemanticLocator {
                test_id: Some("slow-wait-fail".to_string()),
                ..Default::default()
            },
            timeout_ms: 500,
        }],
        required_capabilities: Vec::new(),
        author_id: None,
        source_trace_summary: None,
    };
    let failing_wait_installed = make_installed("probe-1b-wait-fail", &failing_wait_script);

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://fail-slow.example", wcx);
            w.run_installed_store_app(failing_wait_installed.clone(), wcx);
        });
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !bw.recorder.is_replaying
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && matches!(bw.control_mode(), BrowserControlMode::Idle)
                && bw.store.store_error.is_some()
        })
        .await,
        "Probe 1B must settle both fail-slow nav and failing store app"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1B: bw.last_error must retain fail-slow error, got {:?}",
            bw.last_error
        );
        assert!(
            sr.last_error()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1B: sr.last_error() must retain fail-slow error, got {:?}",
            sr.last_error()
        );
        assert!(
            bw.recorder
                .replay_error
                .as_deref()
                .unwrap_or("")
                .contains("slow-wait-fail timed out"),
            "Probe 1B: replay_error must record store app failure, got {:?}",
            bw.recorder.replay_error
        );
        assert!(
            bw.store
                .store_error
                .as_deref()
                .unwrap_or("")
                .contains("slow-wait-fail timed out"),
            "Probe 1B: store_error must record store app failure, got {:?}",
            bw.store.store_error
        );
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Probe 1E: Post-settlement user_take_over() AFTER fail-slow has settled
    //           must NOT wipe fail-slow's last_error via refresh_frame().
    // ------------------------------------------------------------------------
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.user_take_over(wcx);
        });
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading && !sr.is_refreshing() && !sr.is_navigating()
        })
        .await
    );
    app.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1E: bw.last_error must survive post-settlement user_take_over(), got {:?}",
            bw.last_error
        );
        assert!(
            sr.last_error()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1E: sr.last_error() must survive post-settlement user_take_over(), got {:?}",
            sr.last_error()
        );
    })
    .unwrap();

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| w.user_cancel_ai(wcx));
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Probe 1H: Post-settlement StepCompleted AFTER fail-slow has settled
    //           must NOT wipe fail-slow's last_error via refresh_frame().
    // ------------------------------------------------------------------------
    let session_1h = app
        .update(|cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            bw.session_registry
                .get_session(&bw.active_session_id)
                .expect("active session present")
        })
        .unwrap();
    session_1h.emit_step_completed(
        0,
        "post_settlement_click",
        15,
        None,
        Some("https://a.example".to_string()),
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && bw.recorder
                    .event_logs
                    .iter()
                    .any(|l| l.detail.contains("post_settlement_click") || l.title.contains("post_settlement_click"))
        })
        .await
    );
    app.background_executor()
        .timer(Duration::from_millis(100))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1H: bw.last_error must survive post-settlement StepCompleted, got {:?}",
            bw.last_error
        );
        assert!(
            sr.last_error()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1H: sr.last_error() must survive post-settlement StepCompleted, got {:?}",
            sr.last_error()
        );
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Probe 1D: Slow replay_script_app (Navigate(slow1) 140ms -> WaitFor(slow-wait-ok) 160ms)
    //           + mid-run UI fail-slow nav (80ms). When WaitFor finishes AFTER fail-slow
    //           has settled, replay_script_app + ControlModeChanged(Idle) must NOT wipe
    //           fail-slow's error, and URLs must converge on slow1.example.
    // ------------------------------------------------------------------------
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| w.navigate("https://a.example", wcx));
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && bw.last_error.is_none()
                && sr.last_error().is_none()
                && bw.current_url == "https://a.example"
        })
        .await
    );

    let slow_replay_script = AutomationScriptApp {
        schema_version: 1,
        app_id: "probe-1d-slow-replay".to_string(),
        slug: "probe-1d-slow-replay".to_string(),
        name: "Probe 1D Slow Replay".to_string(),
        version: "1.0.0".to_string(),
        description: "Navigate slow1 then slow wait ok".to_string(),
        category: "general".to_string(),
        target_domains: vec!["slow1.example".to_string()],
        parameters: Vec::new(),
        steps: vec![
            ScriptStep::Navigate {
                url_template: "https://slow1.example".to_string(),
            },
            ScriptStep::WaitFor {
                locator: SemanticLocator {
                    test_id: Some("slow-wait-ok".to_string()),
                    ..Default::default()
                },
                timeout_ms: 500,
            },
        ],
        required_capabilities: Vec::new(),
        author_id: None,
        source_trace_summary: None,
    };

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.replay_script_app(slow_replay_script.clone(), BTreeMap::new(), wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://fail-slow.example", wcx);
        });
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !bw.recorder.is_replaying
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && matches!(bw.control_mode(), BrowserControlMode::Idle)
        })
        .await,
        "Probe 1D must settle both slow replay and mid-run fail-slow"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(backend.current_url_sync(), "https://slow1.example");
        assert_eq!(bw.current_url, "https://slow1.example");
        assert_eq!(bw.url_input, "https://slow1.example");
        assert_eq!(sr.current_url(), "https://slow1.example");
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1D: bw.last_error must retain mid-run fail-slow error, got {:?}",
            bw.last_error
        );
        assert!(
            sr.last_error()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1D: sr.last_error() must retain mid-run fail-slow error, got {:?}",
            sr.last_error()
        );
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Probe 1F: run_installed_store_app(Navigate(slow1)) + mid-run fail-slow.
    //           Even when fail-slow fails after slow1 completed on backend,
    //           bw.current_url, bw.url_input, and sr.current_url() MUST converge
    //           with backend ("https://slow1.example") while preserving last_error.
    // ------------------------------------------------------------------------
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| w.navigate("https://a.example", wcx));
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && bw.last_error.is_none()
                && sr.last_error().is_none()
                && bw.current_url == "https://a.example"
        })
        .await
    );

    let slow1_script = AutomationScriptApp {
        schema_version: 1,
        app_id: "probe-1f-slow1".to_string(),
        slug: "probe-1f-slow1".to_string(),
        name: "Probe 1F Slow1".to_string(),
        version: "1.0.0".to_string(),
        description: "Navigate to slow1".to_string(),
        category: "general".to_string(),
        target_domains: vec!["slow1.example".to_string()],
        parameters: Vec::new(),
        steps: vec![ScriptStep::Navigate {
            url_template: "https://slow1.example".to_string(),
        }],
        required_capabilities: Vec::new(),
        author_id: None,
        source_trace_summary: None,
    };
    let slow1_installed = make_installed("probe-1f-slow1", &slow1_script);

    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.run_installed_store_app(slow1_installed.clone(), wcx);
        });
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(20))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.navigate("https://fail-slow.example", wcx);
        });
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !bw.recorder.is_replaying
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && matches!(bw.control_mode(), BrowserControlMode::Idle)
        })
        .await,
        "Probe 1F must settle store app + fail-slow nav"
    );
    app.background_executor()
        .timer(Duration::from_millis(120))
        .await;
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(backend.current_url_sync(), "https://slow1.example");
        assert_eq!(
            bw.current_url, "https://slow1.example",
            "Probe 1F: bw.current_url must converge to backend URL"
        );
        assert_eq!(
            bw.url_input, "https://slow1.example",
            "Probe 1F: bw.url_input must converge to backend URL"
        );
        assert_eq!(
            sr.current_url(),
            "https://slow1.example",
            "Probe 1F: sr.current_url() must converge to backend URL"
        );
        assert!(
            bw.last_error
                .as_deref()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1F: bw.last_error must retain fail-slow error, got {:?}",
            bw.last_error
        );
        assert!(
            sr.last_error()
                .unwrap_or("")
                .contains("simulated nav failure for https://fail-slow.example"),
            "Probe 1F: sr.last_error() must retain fail-slow error, got {:?}",
            sr.last_error()
        );
    })
    .unwrap();

    // Subsequent valid navigation MUST clear the preserved navigation error
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| w.navigate("https://b.example", wcx));
    })
    .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.is_loading
                && !sr.is_refreshing()
                && !sr.is_navigating()
                && bw.current_url == "https://b.example"
        })
        .await
    );
    app.update(|cx| {
        let bw = shell.read(cx).browser_workspace().unwrap().read(cx);
        let sr = bw.cdp_surface().unwrap().read(cx);
        assert_eq!(bw.last_error, None);
        assert_eq!(sr.last_error(), None);
        assert_eq!(bw.url_input, "https://b.example");
        assert_eq!(sr.current_url(), "https://b.example");
    })
    .unwrap();

    app.update(|_cx| drop(shell)).unwrap();
    drop(client);
}

// ============================================================================
// Test 14: Regression — Stale active_input Across Cmd+,/Cmd+9/Tab Switches
//          & StorePanelState::store_error Clear in run_installed_store_app
//          (Challenger 12 Findings 1 & 2)
// ============================================================================

#[test]
fn test_m4b_regression_stale_active_input_focus_and_store_error_clear() {
    run_in_headless_subprocess_with_window(
        "test_m4b_regression_stale_active_input_focus_and_store_error_clear",
    );
}

async fn body_regression_stale_active_input_focus_and_store_error_clear(mut app: AsyncApp) {
    use gpui_telegram_proto::automation_apps::InstalledAutomationApp;

    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");

    // ------------------------------------------------------------------------
    // Part A: Headless Window — DistillName -> Cmd+, -> Cmd+9 -> focus
    //         CdpViewportSurface -> draw -> 'z' arrives in backend, plus
    //         inspector_tab switching & Cmd+N/Cmd+9 re-entry checks.
    // ------------------------------------------------------------------------
    let backend = HttpBackedMockBrowserBackend::new("https://x.com");
    let win_handle = app
        .update(|cx| {
            let _rt_guard = client.runtime().enter();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut s = AppShell::new_with_client(client.clone(), cx);
                        s.is_authenticated = true;
                        s.session_sync.user_id = Some(115);
                        s.session_sync.username = Some("maya".to_string());
                        s.session_sync.session_token = Some("tok_maya".to_string());
                        s.magpie_local.bootstrapped_auth_generation =
                            Some(s.session_sync.auth_generation);
                        s.set_browser_backend_for_test(backend.clone(), cx);
                        s
                    })
                },
            )
            .expect("open headless window")
        })
        .unwrap();

    let shell_ent = win_handle.entity(&app).unwrap();

    // Initial draw
    app.update_window(win_handle.into(), |_, window, cx| {
        let _ = window.draw(cx);
    })
    .unwrap();

    // Focus DistillName + transient IME preedit, then Cmd+, -> Cmd+9 -> focus CdpViewportSurface -> draw -> 'z'
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
            assert_eq!(bw.read(scx).active_input, ActiveBrowserInput::DistillName);
            assert_eq!(bw.read(scx).recorder.distill_name_draft, "Recorded Browser Flow");
        });

        window.dispatch_keystroke(Keystroke::parse("cmd-,").unwrap(), cx);
    })
    .unwrap();

    app.update(|cx| {
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert_eq!(bw.active_input, ActiveBrowserInput::None);
        assert!(!bw.is_workspace_visible());
        assert!(bw.is_externally_occluded());
        assert_eq!(s.active_text_and_preedit(cx).1, None);
    })
    .unwrap();

    app.update_window(win_handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("cmd-9").unwrap(), cx);
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.focus_handle(scx).focus(window);
        });
        let _ = window.draw(cx);
        assert!(
            shell_ent
                .read(cx)
                .is_browser_cdp_viewport_focused(window, cx),
            "CdpViewportSurface must retain GPUI focus after Cmd+, -> Cmd+9"
        );
        window.dispatch_keystroke(Keystroke::parse("z").unwrap(), cx);
    })
    .unwrap();

    let got_z = pump_until(&mut app, Duration::from_secs(2), |_| {
        backend
            .key_events
            .lock()
            .unwrap()
            .iter()
            .any(|k| k.key == "z")
    })
    .await;
    assert!(
        got_z,
        "'z' must forward to backend.key_events after Cmd+, -> Cmd+9, got {:?}",
        backend.key_events.lock().unwrap()
    );

    app.update(|cx| {
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
        assert_eq!(bw.active_input, ActiveBrowserInput::None);
        assert_eq!(
            bw.recorder.distill_name_draft, "Recorded Browser Flow",
            "distill_name_draft must remain unchanged when typing into CdpViewportSurface"
        );
    })
    .unwrap();

    // Repeat with set_active_input(..., None, ...) (headless/programmatic focus without window)
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, None, wcx);
            });
        });
        window.dispatch_keystroke(Keystroke::parse("cmd-,").unwrap(), cx);
        window.dispatch_keystroke(Keystroke::parse("cmd-9").unwrap(), cx);
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.focus_handle(scx).focus(window);
        });
        let _ = window.draw(cx);
        window.dispatch_keystroke(Keystroke::parse("q").unwrap(), cx);
    })
    .unwrap();

    let got_q = pump_until(&mut app, Duration::from_secs(2), |_| {
        backend
            .key_events
            .lock()
            .unwrap()
            .iter()
            .any(|k| k.key == "q")
    })
    .await;
    assert!(got_q, "'q' must forward to backend.key_events");

    // Inspector tab switching clears tab-owned active_input while preserving AddressBar
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, None, wcx);
                w.set_inspector_tab(BrowserInspectorTab::Store, wcx);
                assert_eq!(
                    w.active_input,
                    ActiveBrowserInput::None,
                    "Switching away from Recorder tab must clear DistillName active_input"
                );

                w.set_inspector_tab(BrowserInspectorTab::Copilot, wcx);
                w.set_active_input(ActiveBrowserInput::CopilotPrompt, None, wcx);
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                assert_eq!(
                    w.active_input,
                    ActiveBrowserInput::None,
                    "Switching away from Copilot tab must clear CopilotPrompt active_input"
                );

                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
                w.set_inspector_tab(BrowserInspectorTab::Session, wcx);
                assert_eq!(
                    w.active_input,
                    ActiveBrowserInput::AddressBar,
                    "Top-bar AddressBar must stay active when switching inspector tabs"
                );
            });
        });

        // Leaving via Cmd+N (Chats) and returning via Cmd+9 also clears stale active_input
        window.dispatch_keystroke(Keystroke::parse("cmd-n").unwrap(), cx);
        assert_eq!(shell_ent.read(cx).active_panel, ActiveWorkspacePanel::Chats);
        window.dispatch_keystroke(Keystroke::parse("cmd-9").unwrap(), cx);
        let bw = shell_ent.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(shell_ent.read(cx).active_panel, ActiveWorkspacePanel::Browser);
        assert_eq!(bw.active_input, ActiveBrowserInput::None);
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part B: run_installed_store_app clears stale StorePanelState::store_error
    //         on subsequent succeeding store app run (Challenger 12 Finding 2).
    // ------------------------------------------------------------------------
    let shell2 = app
        .update(|cx| build_authenticated_shell(cx, client.clone(), 116, "noah"))
        .unwrap();
    let adv_backend = AdversarialNavBackend::new("https://a.example");
    app.update(|cx| {
        shell2.update(cx, |s, scx| {
            s.set_browser_backend_for_test(adv_backend.clone(), scx);
        });
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let bw = shell2.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !sr.is_refreshing() && sr.current_url() == "https://a.example"
        })
        .await
    );

    let make_installed = |app_id: &str, script: &AutomationScriptApp| InstalledAutomationApp {
        app_id: app_id.to_string(),
        slug: app_id.to_string(),
        name: script.name.clone(),
        description: script.description.clone(),
        category: script.category.clone(),
        target_domains: script.target_domains.clone(),
        installed_version: script.version.clone(),
        latest_version: script.version.clone(),
        manifest: serde_json::to_value(script).expect("serialize script manifest"),
        content_sha256: String::new(),
        installed_at_ms: 1700000000000,
        last_run_at_ms: None,
        local_run_count: 0,
    };

    let failing_script = AutomationScriptApp {
        schema_version: 1,
        app_id: "store-fail-app".to_string(),
        slug: "store-fail-app".to_string(),
        name: "Store Fail App".to_string(),
        version: "1.0.0".to_string(),
        description: "Fails on slow-wait-fail".to_string(),
        category: "general".to_string(),
        target_domains: vec!["a.example".to_string()],
        parameters: Vec::new(),
        steps: vec![ScriptStep::WaitFor {
            locator: SemanticLocator {
                test_id: Some("slow-wait-fail".to_string()),
                ..Default::default()
            },
            timeout_ms: 500,
        }],
        required_capabilities: Vec::new(),
        author_id: None,
        source_trace_summary: None,
    };
    let ok_script = AutomationScriptApp {
        schema_version: 1,
        app_id: "store-ok-app".to_string(),
        slug: "store-ok-app".to_string(),
        name: "Store Ok App".to_string(),
        version: "1.0.0".to_string(),
        description: "Navigates cleanly to b.example".to_string(),
        category: "general".to_string(),
        target_domains: vec!["b.example".to_string()],
        parameters: Vec::new(),
        steps: vec![ScriptStep::Navigate {
            url_template: "https://b.example".to_string(),
        }],
        required_capabilities: Vec::new(),
        author_id: None,
        source_trace_summary: None,
    };

    let failing_installed = make_installed("store-fail-app", &failing_script);
    let ok_installed = make_installed("store-ok-app", &ok_script);

    app.update(|cx| {
        let bw = shell2.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.run_installed_store_app(failing_installed.clone(), wcx);
        });
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let bw = shell2.read(cx).browser_workspace().unwrap().read(cx);
            !bw.recorder.is_replaying && bw.store.store_error.is_some()
        })
        .await,
        "Failing run_installed_store_app must populate store.store_error"
    );
    app.update(|cx| {
        let bw = shell2.read(cx).browser_workspace().unwrap().read(cx);
        assert!(
            bw.store
                .store_error
                .as_deref()
                .unwrap_or("")
                .contains("slow-wait-fail timed out"),
            "Unexpected store_error: {:?}",
            bw.store.store_error
        );
        assert!(bw.recorder.replay_error.is_some());
    })
    .unwrap();

    // Now run a succeeding installed store app -> store_error MUST be cleared!
    app.update(|cx| {
        let bw = shell2.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.run_installed_store_app(ok_installed.clone(), wcx);
        });
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let bw = shell2.read(cx).browser_workspace().unwrap().read(cx);
            let sr = bw.cdp_surface().unwrap().read(cx);
            !bw.recorder.is_replaying
                && !sr.is_refreshing()
                && bw.current_url == "https://b.example"
                && bw
                    .recorder
                    .last_replay_report
                    .as_ref()
                    .is_some_and(|r| r.app_id == "store-ok-app" && r.success)
        })
        .await,
        "Succeeding run_installed_store_app must complete and navigate to b.example"
    );

    app.update(|cx| {
        let bw = shell2.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(
            bw.store.store_error, None,
            "Succeeding run_installed_store_app must clear stale store_error"
        );
        assert_eq!(bw.recorder.replay_error, None);
    })
    .unwrap();

    app.update(|_cx| drop(shell2)).unwrap();
    drop(client);
}

// ============================================================================
// Test 15: Regression — Direct CdpViewportSurface Focus Bounce, IME Preedit
//          Leak Across Cmd+K / set_inspector_tab, and Cmd+N Immediate Hide
//          (Challengers 13 & 14)
// ============================================================================

#[test]
fn test_m4b_regression_direct_cdp_focus_ime_leak_and_cmd_n_hide() {
    run_in_headless_subprocess_with_window(
        "test_m4b_regression_direct_cdp_focus_ime_leak_and_cmd_n_hide",
    );
}

async fn body_regression_direct_cdp_focus_ime_leak_and_cmd_n_hide(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let backend = HttpBackedMockBrowserBackend::new("https://x.com");

    // ------------------------------------------------------------------------
    // Part A: Pre-draw direct surf.focus_handle(cx).focus(window) while
    //         DistillName + IME preedit "ni" is active (before initial draw).
    // ------------------------------------------------------------------------
    let win_handle = app
        .update(|cx| {
            let _rt_guard = client.runtime().enter();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let mut s = AppShell::new_with_client(client.clone(), cx);
                        s.is_authenticated = true;
                        s.session_sync.user_id = Some(120);
                        s.session_sync.username = Some("olivia".to_string());
                        s.session_sync.session_token = Some("tok_olivia".to_string());
                        s.magpie_local.bootstrapped_auth_generation =
                            Some(s.session_sync.auth_generation);
                        s.set_browser_backend_for_test(backend.clone(), cx);
                        let bw = s.browser_workspace().unwrap().clone();
                        bw.update(cx, |w, wcx| {
                            w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                            w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
                        });
                        s.update_ime_marked_text("ni", cx);
                        let surf = bw.read(cx).cdp_surface().unwrap().clone();
                        surf.update(cx, |sr, scx| {
                            sr.focus_handle(scx).focus(window);
                        });
                        s
                    })
                },
            )
            .expect("open headless window")
        })
        .unwrap();

    let shell_ent = win_handle.entity(&app).unwrap();

    app.update_window(win_handle.into(), |_, window, cx| {
        let _ = window.draw(cx);
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert!(
            s.is_browser_cdp_viewport_focused(window, cx),
            "Pre-draw surf.focus must survive initial window.draw(cx)"
        );
        assert_eq!(bw.active_input, ActiveBrowserInput::None);
        assert_eq!(s.active_text_and_preedit(cx).1, None);
        window.dispatch_keystroke(Keystroke::parse("y").unwrap(), cx);
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(2), |_| {
            backend
                .key_events
                .lock()
                .unwrap()
                .iter()
                .any(|k| k.key == "y")
        })
        .await,
        "'y' must reach backend.key_events after pre-draw surf.focus"
    );
    app.update(|cx| {
        let bw = shell_ent.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(bw.recorder.distill_name_draft, "Recorded Browser Flow");
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part B: Post-draw direct surf.focus while DistillName (Some(window) & None)
    //         and AddressBar are active, plus AddressBar typing sanity check.
    // ------------------------------------------------------------------------
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
            let surf = bw.read(scx).cdp_surface().unwrap().clone();
            surf.update(scx, |sr, scx2| {
                sr.focus_handle(scx2).focus(window);
            });
        });
        let _ = window.draw(cx);
        assert!(shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        assert_eq!(shell_ent.read(cx).active_text_and_preedit(cx).1, None);
        window.dispatch_keystroke(Keystroke::parse("k").unwrap(), cx);
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(2), |_| {
            backend
                .key_events
                .lock()
                .unwrap()
                .iter()
                .any(|k| k.key == "k")
        })
        .await,
        "'k' must reach backend.key_events after DistillName -> direct surf.focus"
    );

    // Repeat with DistillName set using window=None (from AppShell focus -> surf.focus)
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, _| {
            s.focus_handle.focus(window);
        });
        let _ = window.draw(cx);
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::DistillName, None, wcx);
            });
            let surf = bw.read(scx).cdp_surface().unwrap().clone();
            surf.update(scx, |sr, scx2| {
                sr.focus_handle(scx2).focus(window);
            });
        });
        let _ = window.draw(cx);
        assert!(shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        window.dispatch_keystroke(Keystroke::parse("m").unwrap(), cx);
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(2), |_| {
            backend
                .key_events
                .lock()
                .unwrap()
                .iter()
                .any(|k| k.key == "m")
        })
        .await,
        "'m' must reach backend.key_events after DistillName(None) -> direct surf.focus"
    );

    // AddressBar typing still works, and direct surf.focus from AddressBar + IME works
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::AddressBar, Some(window), wcx);
            });
        });
        let _ = window.draw(cx);
        window.dispatch_keystroke(Keystroke::parse("a").unwrap(), cx);
    })
    .unwrap();

    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            assert_eq!(bw.read(scx).active_input, ActiveBrowserInput::AddressBar);
            assert_eq!(bw.read(scx).url_input, "https://x.coma");
            bw.update(scx, |w, wcx| {
                w.url_input = "https://x.com".to_string();
                wcx.notify();
            });
            s.update_ime_marked_text("ni", scx);
            let surf = bw.read(scx).cdp_surface().unwrap().clone();
            surf.update(scx, |sr, scx2| {
                sr.focus_handle(scx2).focus(window);
            });
        });
        let _ = window.draw(cx);
        assert!(shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        assert_eq!(shell_ent.read(cx).active_text_and_preedit(cx).1, None);
        window.dispatch_keystroke(Keystroke::parse("x").unwrap(), cx);
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(2), |_| {
            backend
                .key_events
                .lock()
                .unwrap()
                .iter()
                .any(|k| k.key == "x")
        })
        .await,
        "'x' must reach backend.key_events after AddressBar -> direct surf.focus"
    );
    app.update(|cx| {
        let bw = shell_ent.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(bw.url_input, "https://x.com");
        assert_eq!(bw.recorder.distill_name_draft, "Recorded Browser Flow");
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part C: Cmd+J modal -> Esc -> direct surf.focus
    // ------------------------------------------------------------------------
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
        });
        window.dispatch_keystroke(Keystroke::parse("cmd-j").unwrap(), cx);
        assert!(shell_ent.read(cx).modal_container.read(cx).is_modal_active());
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        assert!(!shell_ent.read(cx).modal_container.read(cx).is_modal_active());
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.focus_handle(scx).focus(window);
        });
        let _ = window.draw(cx);
        assert!(shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        window.dispatch_keystroke(Keystroke::parse("v").unwrap(), cx);
    })
    .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(2), |_| {
            backend
                .key_events
                .lock()
                .unwrap()
                .iter()
                .any(|k| k.key == "v")
        })
        .await,
        "'v' must reach backend.key_events after Cmd+J -> Esc -> surf.focus"
    );
    app.update(|cx| {
        let bw = shell_ent.read(cx).browser_workspace().unwrap().read(cx);
        assert_eq!(bw.recorder.distill_name_draft, "Recorded Browser Flow");
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part D: Cmd+K with DistillName + IME preedit "ni" clears preedit,
    //         and Enter leaves sidebar.search_query empty.
    // ------------------------------------------------------------------------
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
        });
        window.dispatch_keystroke(Keystroke::parse("cmd-k").unwrap(), cx);
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert!(s.sidebar.read(cx).search_focused);
        assert_eq!(bw.active_input, ActiveBrowserInput::None);
        assert_eq!(
            s.active_text_and_preedit(cx).1,
            None,
            "Cmd+K must clear stale IME preedit from BrowserWorkspace"
        );
        window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
        let s = shell_ent.read(cx);
        assert!(
            s.sidebar.read(cx).search_query.is_empty(),
            "Enter after Cmd+K must not commit stale preedit into sidebar.search_query, got {:?}",
            s.sidebar.read(cx).search_query
        );
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part E: set_inspector_tab(Store) with DistillName + IME preedit "ni"
    //         clears preedit immediately before any window.draw(cx), and
    //         switching tabs while AddressBar is active preserves AddressBar
    //         and its preedit.
    // ------------------------------------------------------------------------
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
        });
    })
    .unwrap();

    app.update(|cx| {
        let s = shell_ent.read(cx);
        assert_eq!(
            s.active_text_and_preedit(cx),
            ("Recorded Browser Flow".to_string(), Some("ni".to_string()))
        );
        let bw = s.browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.set_inspector_tab(BrowserInspectorTab::Store, wcx);
        });
    })
    .unwrap();

    app.update(|cx| {
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().clone();
        assert_eq!(bw.read(cx).active_input, ActiveBrowserInput::None);
        assert_eq!(
            s.ime.ime_preedit, None,
            "set_inspector_tab away from Recorder must clear ime_preedit before draw"
        );
        bw.update(cx, |w, wcx| {
            w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
        });
    })
    .unwrap();

    app.update(|cx| {
        let s = shell_ent.read(cx);
        assert_eq!(
            s.active_text_and_preedit(cx),
            ("https://x.com".to_string(), None),
            "Focusing AddressBar after set_inspector_tab(Store) must not inherit DistillName preedit"
        );
    })
    .unwrap();

    // Preserving AddressBar + preedit when switching inspector tabs
    app.update(|cx| {
        shell_ent.update(cx, |s, scx| {
            s.update_ime_marked_text("hao", scx);
        });
    })
    .unwrap();
    app.update(|cx| {
        let bw = shell_ent.read(cx).browser_workspace().unwrap().clone();
        bw.update(cx, |w, wcx| {
            w.set_inspector_tab(BrowserInspectorTab::Session, wcx);
        });
    })
    .unwrap();
    app.update(|cx| {
        let s = shell_ent.read(cx);
        assert_eq!(
            s.browser_workspace().unwrap().read(cx).active_input,
            ActiveBrowserInput::AddressBar
        );
        assert_eq!(
            s.active_text_and_preedit(cx),
            ("https://x.com".to_string(), Some("hao".to_string()))
        );
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part F: Cmd+N while in Browser with DistillName active immediately
    //         hides/occludes BrowserWorkspace and clears active_input + preedit
    //         BEFORE any window.draw(cx).
    // ------------------------------------------------------------------------
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
        });
        window.dispatch_keystroke(Keystroke::parse("cmd-n").unwrap(), cx);
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert!(
            !bw.is_workspace_visible(),
            "Cmd+N must immediately set bw.is_workspace_visible() == false before draw"
        );
        assert!(
            bw.is_externally_occluded(),
            "Cmd+N must immediately set bw.is_externally_occluded() == true before draw"
        );
        assert_eq!(
            bw.active_input,
            ActiveBrowserInput::None,
            "Cmd+N must immediately clear bw.active_input before draw"
        );
        assert_eq!(s.ime.ime_preedit, None);
    })
    .unwrap();

    drop(client);
}

// ============================================================================
// Test 16: Regression — Shell Blockers (Drawer/Contacts/Creation/Computer),
//          Cmd+E/J/P Overlay IME/Input Cleanup, Sidebar/Menu Panel Transitions,
//          and Programmatic HOSTILE-4H / HOSTILE-4I Invariants (Challengers 15 & 16)
// ============================================================================

#[test]
fn test_m4b_regression_shell_blockers_overlay_ime_and_hostile_invariants() {
    run_in_headless_subprocess_with_window(
        "test_m4b_regression_shell_blockers_overlay_ime_and_hostile_invariants",
    );
}

async fn body_regression_shell_blockers_overlay_ime_and_hostile_invariants(mut app: AsyncApp) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let backend = HttpBackedMockBrowserBackend::new("https://x.com");

    let win_handle = app
        .update(|cx| {
            let _rt_guard = client.runtime().enter();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |_window, cx| {
                    cx.new(|cx| {
                        let mut s = AppShell::new_with_client(client.clone(), cx);
                        s.is_authenticated = true;
                        s.session_sync.user_id = Some(130);
                        s.session_sync.username = Some("maya".to_string());
                        s.session_sync.session_token = Some("tok_maya".to_string());
                        s.magpie_local.bootstrapped_auth_generation =
                            Some(s.session_sync.auth_generation);
                        s.sidebar.update(cx, |sb, _| {
                            sb.dialogs.push(DialogItemModel {
                                id: 77,
                                peer_name: "Chat 77".to_string(),
                                avatar_initials: "C7".to_string(),
                                avatar_color: 0x5092E6,
                                last_message: "hi".to_string(),
                                timestamp: "now".to_string(),
                                unread_count: 0,
                                is_muted: false,
                                is_pinned: false,
                                is_online: true,
                                typing_indicator: None,
                                delivery_status: DialogDeliveryStatus::Read,
                                folder: "All".to_string(),
                            });
                        });
                        s.set_browser_backend_for_test(backend.clone(), cx);
                        s
                    })
                },
            )
            .expect("open headless window")
        })
        .unwrap();

    let shell_ent = win_handle.entity(&app).unwrap();

    let focus_cdp_and_draw = |win: &mut gpui::Window, cx: &mut App| {
        let surf = shell_ent
            .read(cx)
            .browser_workspace()
            .unwrap()
            .read(cx)
            .cdp_surface()
            .unwrap()
            .clone();
        surf.update(cx, |sr, scx| {
            sr.focus_handle(scx).focus(win);
        });
        let _ = win.draw(cx);
        assert!(
            shell_ent.read(cx).is_browser_cdp_viewport_focused(win, cx),
            "CdpViewportSurface must hold focus before blocker check"
        );
    };

    // ------------------------------------------------------------------------
    // Part A: CAVEAT-7A / 7B / 7C / 7I — Shell blockers while CdpViewportSurface
    //         holds focus must prevent key hijacking and let Esc close overlay.
    // ------------------------------------------------------------------------
    // 1. CAVEAT-7A: main_menu_drawer
    app.update_window(win_handle.into(), |_, window, cx| {
        focus_cdp_and_draw(window, cx);
        backend.key_events.lock().unwrap().clear();
        shell_ent.update(cx, |s, scx| {
            s.main_menu_drawer.update(scx, |d, dcx| {
                d.open();
                dcx.notify();
            });
        });
        let _ = window.draw(cx);
        assert!(!shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        window.dispatch_keystroke(Keystroke::parse("a").unwrap(), cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        assert!(
            !shell_ent.read(cx).main_menu_drawer.read(cx).is_open,
            "Single Esc must close main_menu_drawer while browser was CDP-focused"
        );
    })
    .unwrap();

    // 2. CAVEAT-7B: contacts_modal
    app.update_window(win_handle.into(), |_, window, cx| {
        focus_cdp_and_draw(window, cx);
        shell_ent.update(cx, |s, scx| {
            s.sidebar.update(scx, |sb, sbcx| {
                sb.open_contacts_modal();
                sbcx.notify();
            });
        });
        let _ = window.draw(cx);
        assert!(!shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        window.dispatch_keystroke(Keystroke::parse("q").unwrap(), cx);
        assert_eq!(
            shell_ent.read(cx).sidebar.read(cx).contacts_search_query,
            "q",
            "Typing 'q' behind contacts_modal must reach contacts_search_query"
        );
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        assert!(!shell_ent.read(cx).sidebar.read(cx).is_contacts_modal_open);
    })
    .unwrap();

    // 3. CAVEAT-7C: creation_modal
    app.update_window(win_handle.into(), |_, window, cx| {
        focus_cdp_and_draw(window, cx);
        shell_ent.update(cx, |s, scx| {
            s.sidebar.update(scx, |sb, sbcx| {
                sb.open_create_group_modal();
                sbcx.notify();
            });
        });
        let _ = window.draw(cx);
        assert!(!shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        window.dispatch_keystroke(Keystroke::parse("z").unwrap(), cx);
        assert_eq!(
            shell_ent.read(cx).sidebar.read(cx).creation_primary_input,
            "z",
            "Typing 'z' behind creation_modal must reach creation_primary_input"
        );
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        assert!(shell_ent
            .read(cx)
            .sidebar
            .read(cx)
            .active_creation_modal
            .is_none());
    })
    .unwrap();

    // 4. CAVEAT-7I: computer_overlay
    app.update_window(win_handle.into(), |_, window, cx| {
        focus_cdp_and_draw(window, cx);
        shell_ent.update(cx, |s, scx| {
            s.show_remote_computer(scx);
        });
        let _ = window.draw(cx);
        assert!(!shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));
        window.dispatch_keystroke(Keystroke::parse("a").unwrap(), cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        assert!(
            shell_ent.read(cx).overlays.computer_overlay.is_none(),
            "Single Esc must close computer_overlay while browser was CDP-focused"
        );
    })
    .unwrap();

    // Pump briefly to confirm zero leaked key events reached backend during Part A
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    assert!(
        backend.key_events.lock().unwrap().is_empty(),
        "Zero keys may leak to backend behind shell overlays, got: {:?}",
        *backend.key_events.lock().unwrap()
    );

    // ------------------------------------------------------------------------
    // Part B: HOSTILE-4K — Cmd+E Account Popover must clear active_input &
    //         IME preedit and prevent occluded typing into distill_name_draft.
    // ------------------------------------------------------------------------
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.recorder.distill_name_draft = "Clean".to_string();
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
        });
        window.dispatch_keystroke(Keystroke::parse("cmd-e").unwrap(), cx);
        let _ = window.draw(cx);
        {
            let s = shell_ent.read(cx);
            let bw = s.browser_workspace().unwrap().read(cx);
            assert!(s.sidebar.read(cx).is_account_popover_open);
            assert_eq!(bw.active_input, ActiveBrowserInput::None);
            assert_eq!(s.ime.ime_preedit, None);
        }
        window.dispatch_keystroke(Keystroke::parse("x").unwrap(), cx);
        {
            let s = shell_ent.read(cx);
            let bw = s.browser_workspace().unwrap().read(cx);
            assert_eq!(
                bw.recorder.distill_name_draft, "Clean",
                "Typing while Account Popover is open must not mutate distill_name_draft"
            );
        }
        window.dispatch_keystroke(Keystroke::parse("cmd-e").unwrap(), cx);
        let _ = window.draw(cx);
        assert!(!shell_ent.read(cx).sidebar.read(cx).is_account_popover_open);
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part C: HOSTILE-4J & CAVEAT-7E — Cmd+J Contacts modal & Cmd+P CreatePoll
    //         modal must not inherit browser IME preedit and must close on 1 Esc.
    // ------------------------------------------------------------------------
    for shortcut in ["cmd-j", "cmd-p"] {
        app.update_window(win_handle.into(), |_, window, cx| {
            shell_ent.update(cx, |s, scx| {
                let bw = s.browser_workspace().unwrap().clone();
                bw.update(scx, |w, wcx| {
                    w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                    w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
                });
                s.update_ime_marked_text("ni", scx);
            });
            window.dispatch_keystroke(Keystroke::parse(shortcut).unwrap(), cx);
            {
                let s = shell_ent.read(cx);
                assert!(s.modal_container.read(cx).is_modal_active());
                assert_eq!(
                    s.active_text_and_preedit(cx).1,
                    None,
                    "{shortcut} modal must not inherit browser IME preedit"
                );
            }
            window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
            assert!(
                !shell_ent.read(cx).modal_container.read(cx).is_modal_active(),
                "Single Esc must close {shortcut} modal"
            );
        })
        .unwrap();
    }

    // ------------------------------------------------------------------------
    // Part D: CAVEAT-7G / 7H — Sidebar dialog selection & MainMenuCommand::TaskCenter
    //         transitions out of Browser clear bw.active_input and ime_preedit.
    // ------------------------------------------------------------------------
    // 1. CAVEAT-7G: sidebar dialog selection
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
            s.sidebar.update(scx, |sb, sbcx| {
                sb.select_dialog_from_user(77);
                sbcx.notify();
            });
        });
    })
    .unwrap();
    app.update_window(win_handle.into(), |_, window, cx| {
        let _ = window.draw(cx);
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert!(!bw.is_workspace_visible());
        assert!(bw.is_externally_occluded());
        assert_eq!(bw.active_input, ActiveBrowserInput::None);
        assert_eq!(s.ime.ime_preedit, None);
    })
    .unwrap();

    // 2. CAVEAT-7H: MainMenuCommand::TaskCenter
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
            s.handle_main_menu_command(MainMenuCommand::TaskCenter, scx);
        });
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::TaskCenter);
        assert!(!bw.is_workspace_visible());
        assert!(bw.is_externally_occluded());
        assert_eq!(bw.active_input, ActiveBrowserInput::None);
        assert_eq!(s.ime.ime_preedit, None);
    })
    .unwrap();

    // ------------------------------------------------------------------------
    // Part E: HOSTILE-4H & HOSTILE-4I (1D-ime-same-update)
    // ------------------------------------------------------------------------
    // 1. HOSTILE-4H: programmatic set_active_input(AddressBar, None) while
    //    CdpViewportSurface already holds window focus survives window.draw(cx)
    //    and reclaims focus so typing reaches AddressBar.
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
        });
        let _ = window.draw(cx);
        focus_cdp_and_draw(window, cx);
        backend.key_events.lock().unwrap().clear();

        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.url_input = "https://x.com".to_string();
                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
            });
        });
        let _ = window.draw(cx);
        {
            let s = shell_ent.read(cx);
            let bw = s.browser_workspace().unwrap().read(cx);
            assert_eq!(
                bw.active_input,
                ActiveBrowserInput::AddressBar,
                "Programmatic set_active_input(AddressBar, None) while CDP focused must survive draw"
            );
            assert!(!s.is_browser_cdp_viewport_focused(window, cx));
            assert!(s.focus_handle.is_focused(window));
        }
        window.dispatch_keystroke(Keystroke::parse("q").unwrap(), cx);
        assert_eq!(
            shell_ent
                .read(cx)
                .browser_workspace()
                .unwrap()
                .read(cx)
                .url_input,
            "https://x.comq"
        );
    })
    .unwrap();
    app.background_executor()
        .timer(Duration::from_millis(60))
        .await;
    assert!(
        backend.key_events.lock().unwrap().is_empty(),
        "HOSTILE-4H typing must go to AddressBar, not backend.key_events"
    );

    // 2. HOSTILE-4I / 1D-ime-same-update: same-app.update set_inspector_tab(Store)
    //    + set_active_input(AddressBar) + update_ime_marked_text("hao") preserves "hao".
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(window), wcx);
            });
            s.update_ime_marked_text("ni", scx);
        });
    })
    .unwrap();

    app.update(|cx| {
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_inspector_tab(BrowserInspectorTab::Store, wcx);
                w.url_input = "https://x.com".to_string();
                w.set_active_input(ActiveBrowserInput::AddressBar, None, wcx);
            });
            s.update_ime_marked_text("hao", scx);
        });
    })
    .unwrap();

    app.update(|cx| {
        let s = shell_ent.read(cx);
        assert_eq!(
            s.active_text_and_preedit(cx),
            ("https://x.com".to_string(), Some("hao".to_string())),
            "Same-update tab switch + AddressBar focus + update_ime_marked_text(\"hao\") must preserve \"hao\""
        );
    })
    .unwrap();

    drop(client);
}

// ============================================================================
// Test 17: Regression — Pre-Draw Overlay Occlusion & IME Cleanup Across
//          Creator Modals, RemoteComputer, MiniApp/Confirm/ToS/BotProfile,
//          IvViewer, Passcode Lock, and Occluded user_take_over (Challengers 17 & 18)
// ============================================================================

#[test]
fn test_m4b_regression_predraw_overlay_occlusion_for_creators_miniapp_iv_computer_and_lock() {
    run_in_headless_subprocess_with_window(
        "test_m4b_regression_predraw_overlay_occlusion_for_creators_miniapp_iv_computer_and_lock",
    );
}

async fn body_regression_predraw_overlay_occlusion_for_creators_miniapp_iv_computer_and_lock(
    mut app: AsyncApp,
) {
    let (_dir, client) = new_isolated_client("http://127.0.0.1:9");
    let backend = HttpBackedMockBrowserBackend::new("https://x.com");

    let win_handle = app
        .update(|cx| {
            let _rt_guard = client.runtime().enter();
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |_window, cx| {
                    cx.new(|cx| {
                        let mut s = AppShell::new_with_client(client.clone(), cx);
                        s.is_authenticated = true;
                        s.session_sync.user_id = Some(131);
                        s.session_sync.username = Some("lina".to_string());
                        s.session_sync.session_token = Some("tok_lina".to_string());
                        s.magpie_local.bootstrapped_auth_generation =
                            Some(s.session_sync.auth_generation);
                        s.preferences.local_passcode_hash = Some("pass_hash_1234".to_string());
                        s.set_browser_backend_for_test(backend.clone(), cx);
                        s
                    })
                },
            )
            .expect("open headless window")
        })
        .unwrap();

    let shell_ent = win_handle.entity(&app).unwrap();

    let arm_distill_with_ime = |label: &str, win: &mut gpui::Window, cx: &mut App| {
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_occluded(false, wcx);
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(win), wcx);
            });
            s.update_ime_marked_text("ni", scx);
        });
        let _ = win.draw(cx);
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(
            bw.active_input,
            ActiveBrowserInput::DistillName,
            "{label}: arm_distill_with_ime active_input"
        );
        assert!(
            !bw.is_workspace_occluded(),
            "{label}: arm_distill_with_ime occluded"
        );
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("ni"),
            "{label}: arm_distill_with_ime ime_preedit"
        );
    };

    let assert_predraw_occluded = |label: &str, win: &gpui::Window, cx: &App| {
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(
            bw.active_input,
            ActiveBrowserInput::None,
            "{label}: bw.active_input must be cleared pre-draw"
        );
        assert!(
            bw.is_workspace_occluded(),
            "{label}: bw.is_workspace_occluded() must be true pre-draw"
        );
        assert!(
            bw.is_externally_occluded(),
            "{label}: bw.is_externally_occluded() must be true pre-draw"
        );
        assert_eq!(
            s.ime.ime_preedit, None,
            "{label}: shell.ime.ime_preedit must be None pre-draw"
        );
        assert_eq!(
            s.active_text_and_preedit(cx).1,
            None,
            "{label}: active_text_and_preedit(cx).1 must be None pre-draw"
        );
        assert!(
            !s.is_browser_workspace_input_active(cx),
            "{label}: is_browser_workspace_input_active must be false pre-draw"
        );
        assert!(
            !s.is_browser_cdp_viewport_focused(win, cx),
            "{label}: is_browser_cdp_viewport_focused must be false pre-draw"
        );
        if let Some(surf) = bw.cdp_surface() {
            assert!(
                !surf.read(cx).is_interactive_enabled(),
                "{label}: CdpViewportSurface interactive_enabled must be false pre-draw"
            );
        }
    };

    // 1. MainMenuCommand::NewFabushiBot (open_fabushi_creator_modal)
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("NewFabushiBot", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.handle_main_menu_command(MainMenuCommand::NewFabushiBot, scx);
        });
        assert!(shell_ent
            .read(cx)
            .miniapps
            .create_bot_box
            .as_ref()
            .is_some_and(|b| b.read(cx).is_visible));
        assert_predraw_occluded("NewFabushiBot", window, cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);
    })
    .unwrap();

    // 1b. open_nl_creator_modal & open_coding_agent_creator_modal
    for (label, mode) in [("open_nl_creator_modal", 0u8), ("open_coding_agent_creator_modal", 1u8)] {
        app.update_window(win_handle.into(), |_, window, cx| {
            arm_distill_with_ime(label, window, cx);
            shell_ent.update(cx, |s, scx| {
                if mode == 0 {
                    s.open_nl_creator_modal(scx);
                } else {
                    s.open_coding_agent_creator_modal(scx);
                }
            });
            assert!(shell_ent
                .read(cx)
                .miniapps
                .create_bot_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible));
            assert_predraw_occluded(label, window, cx);
            window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
            let _ = window.draw(cx);
        })
        .unwrap();
    }

    // 2. MainMenuCommand::RemoteComputer (open_computer_workbench_live -> open_remote_computers_hub)
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("RemoteComputer", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.handle_main_menu_command(MainMenuCommand::RemoteComputer, scx);
        });
        assert_eq!(shell_ent.read(cx).active_panel, ActiveWorkspacePanel::Settings);
        assert!(shell_ent.read(cx).settings_router.read(cx).standalone_computers_mode);
        assert_predraw_occluded("RemoteComputer", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    // 3. ensure_miniapp_overlay & launch_miniapp
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("ensure_miniapp_overlay", window, cx);
        shell_ent.update(cx, |s, scx| {
            let ov = s.ensure_miniapp_overlay(scx);
            ov.update(scx, |w, _| {
                w.is_open = true;
            });
        });
        assert_predraw_occluded("ensure_miniapp_overlay", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.miniapps.miniapp_overlay = None;
            s.miniapps.is_miniapp_minimized = false;
            scx.notify();
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("launch_miniapp", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.launch_miniapp(
                "walletbot",
                "app",
                None,
                MiniAppLaunchMode::FullSize,
                None,
                scx,
            );
        });
        assert!(shell_ent
            .read(cx)
            .miniapps
            .miniapp_overlay
            .as_ref()
            .is_some_and(|ov| ov.read(cx).is_open));
        assert_predraw_occluded("launch_miniapp", window, cx);

        // Close/minimize overlay before re-arming browser DistillName, then trigger OpenMiniAppTos
        let ov_clone = shell_ent.read(cx).miniapps.miniapp_overlay.clone().unwrap();
        shell_ent.update(cx, |s, scx| {
            s.miniapps.is_miniapp_minimized = true;
            ov_clone.update(scx, |w, _| {
                w.is_open = false;
            });
            scx.notify();
        });
        arm_distill_with_ime("miniapp_tos_modal_open", window, cx);
        shell_ent.update(cx, |s, scx| {
            ov_clone.update(scx, |w, _| {
                w.pending_actions
                    .push_back(BotWebViewPendingAction::OpenMiniAppTos);
            });
            s.drain_miniapp_overlay_actions(&ov_clone, scx);
        });
        assert!(shell_ent.read(cx).miniapps.miniapp_tos_modal_open);
        assert_predraw_occluded("miniapp_tos_modal_open", window, cx);

        shell_ent.update(cx, |s, scx| {
            s.miniapps.miniapp_tos_modal_open = false;
            s.miniapps.miniapp_overlay = None;
            s.miniapps.is_miniapp_minimized = false;
            scx.notify();
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    // 4. iv_viewer_overlay via handle_stage_pending_actions
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("iv_viewer_overlay", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.stage.update(scx, |st, _| {
                st.pending_iv_url = Some(("https://example.com/iv".to_string(), "IV".to_string()));
            });
            s.handle_stage_pending_actions(scx);
        });
        assert!(shell_ent.read(cx).overlays.iv_viewer_overlay.is_some());
        assert_predraw_occluded("iv_viewer_overlay", window, cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);
    })
    .unwrap();

    // 5. open_bot_profile_popup (cached path -> show_bot_profile_box_entity)
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("open_bot_profile_popup", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.miniapps.bot_profiles_cache.insert(
                "helperbot".to_string(),
                BotPublicProfileDto {
                    bot: BotRegistryEntry {
                        bot_user_id: 900_001,
                        username: "HelperBot".to_string(),
                        display_name: "Helper Bot".to_string(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            s.open_bot_profile_popup("@HelperBot", scx);
        });
        assert!(shell_ent
            .read(cx)
            .miniapps
            .bot_profile_box
            .as_ref()
            .is_some_and(|b| b.read(cx).is_visible));
        assert_predraw_occluded("open_bot_profile_popup", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.miniapps.bot_profile_box = None;
            scx.notify();
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    // 6. pending_miniapp_confirm via request_launch_miniapp_with_confirm_and_url
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("pending_miniapp_confirm", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.request_launch_miniapp_with_confirm_and_url(
                "NewTosBot",
                "mini",
                None,
                MiniAppLaunchMode::FullSize,
                None,
                None,
                scx,
            );
        });
        assert!(shell_ent.read(cx).miniapps.pending_miniapp_confirm.is_some());
        assert_predraw_occluded("pending_miniapp_confirm", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.miniapps.pending_miniapp_confirm = None;
            scx.notify();
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    // 7. lock_app_now (is_passcode_locked) + occluded user_take_over guard
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("lock_app_now", window, cx);
        shell_ent.update(cx, |s, scx| {
            assert!(s.lock_app_now(scx), "lock_app_now must succeed with passcode configured");
        });
        assert!(shell_ent.read(cx).overlays.is_passcode_locked);
        assert_predraw_occluded("lock_app_now", window, cx);

        // Also verify user_take_over while occluded keeps CdpViewportSurface non-interactive
        shell_ent.update(cx, |s, scx| {
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.user_take_over(wcx);
                let surf = w.cdp_surface().unwrap().read(wcx);
                assert!(
                    !surf.is_interactive_enabled(),
                    "user_take_over while occluded must keep interactive_enabled == false"
                );
            });
        });
    })
    .unwrap();

    drop(client);
}

// ============================================================================
// Test 18: Regression — Escape Dismissal for MiniApp Confirm/Profile/ToS
//          Modals, user_cancel_ai Occluded Guard, Minimized Queued
//          request_takeover, and RemoteWorkbench/CallUpdated/Inspector Pre-Draw
// ============================================================================

#[test]
fn test_m4b_regression_escape_miniapp_modals_cancel_ai_occluded_and_async_overlays() {
    run_in_headless_subprocess_with_window(
        "test_m4b_regression_escape_miniapp_modals_cancel_ai_occluded_and_async_overlays",
    );
}

async fn start_delayed_miniapp_launch_mock_server(
    handle: &tokio::runtime::Handle,
) -> (String, tokio::sync::oneshot::Sender<()>) {
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<String>();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    handle.spawn(async move {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind delayed miniapp launch mock server");
        let addr = listener.local_addr().unwrap();
        let _ = addr_tx.send(format!("http://{addr}"));

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((mut sock, _)) = accepted else { break; };
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 32768];
                        let mut total = 0usize;
                        let header_end;
                        loop {
                            let Ok(n) = sock.read(&mut buf[total..]).await else { return; };
                            if n == 0 { return; }
                            total += n;
                            if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                                header_end = pos + 4;
                                break;
                            }
                        }
                        let head = String::from_utf8_lossy(&buf[..header_end]);
                        let path = head
                            .lines()
                            .next()
                            .and_then(|l| l.split_whitespace().nth(1))
                            .unwrap_or("/");
                        let body = if path == "/api/v1/miniapps/takeover_bot/mini/launch" {
                            tokio::time::sleep(Duration::from_millis(75)).await;
                            let ctx = MiniAppLaunchContext {
                                app: MiniAppEntry {
                                    app_id: "app-takeover-18".to_string(),
                                    bot_user_id: 901_111,
                                    bot_username: "takeover_bot".to_string(),
                                    short_name: "mini".to_string(),
                                    title: "Takeover Mini".to_string(),
                                    app_tos_accepted: true,
                                    ..Default::default()
                                },
                                companion_bot: BotRegistryEntry {
                                    bot_user_id: 901_111,
                                    username: "takeover_bot".to_string(),
                                    display_name: "Takeover Bot".to_string(),
                                    ..Default::default()
                                },
                                companion_chat_id: 9118,
                                launch_mode: MiniAppLaunchMode::CompactSheet,
                                ..Default::default()
                            };
                            serde_json::to_string(&ctx).unwrap()
                        } else {
                            r#"{"items":[]}"#.to_string()
                        };
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                    });
                }
            }
        }
    });

    let base_url = addr_rx.await.expect("delayed miniapp mock addr");
    (base_url, shutdown_tx)
}

async fn body_regression_escape_miniapp_modals_cancel_ai_occluded_and_async_overlays(
    mut app: AsyncApp,
) {
    let (_tmp, seed_client) = new_isolated_client("http://127.0.0.1:9");
    let (mock_url, shutdown_tx) = app
        .update(|_| {
            seed_client
                .runtime()
                .block_on(start_delayed_miniapp_launch_mock_server(
                    seed_client.runtime().handle(),
                ))
        })
        .unwrap();

    let (_dir, client) = new_isolated_client(&mock_url);
    client.set_token(Some("tok_lina18".to_string()));
    let _rt_guard = client.runtime().enter();
    let backend = HttpBackedMockBrowserBackend::new("https://x.com");

    let win_handle = app
        .update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                |_window, cx| {
                    cx.new(|cx| {
                        let mut s = AppShell::new_with_client(client.clone(), cx);
                        s.session_sync.begin_session(
                            "tok_lina18".to_string(),
                            "sess_lina18".to_string(),
                            132,
                            "lina18".to_string(),
                        );
                        s.is_authenticated = true;
                        s.magpie_local.bootstrapped_auth_generation =
                            Some(s.session_sync.auth_generation);
                        s.miniapps.bot_profiles_cache.insert(
                            "helperbot".to_string(),
                            BotPublicProfileDto {
                                bot: BotRegistryEntry {
                                    bot_user_id: 900_018,
                                    username: "HelperBot".to_string(),
                                    display_name: "Helper Bot".to_string(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                        );
                        s.set_browser_backend_for_test(backend.clone(), cx);
                        s
                    })
                },
            )
            .expect("open headless window")
        })
        .unwrap();

    let shell_ent = win_handle.entity(&app).unwrap();

    let arm_distill_with_ime = |label: &str, win: &mut gpui::Window, cx: &mut App| {
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_occluded(false, wcx);
                w.set_inspector_tab(BrowserInspectorTab::Recorder, wcx);
                w.set_active_input(ActiveBrowserInput::DistillName, Some(win), wcx);
            });
            s.update_ime_marked_text("ni", scx);
        });
        let _ = win.draw(cx);
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(bw.active_input, ActiveBrowserInput::DistillName, "{label}: armed input");
        assert!(!bw.is_workspace_occluded(), "{label}: armed unoccluded");
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("ni"), "{label}: armed ime");
    };

    let assert_predraw_occluded = |label: &str, win: &gpui::Window, cx: &App| {
        let s = shell_ent.read(cx);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert_eq!(bw.active_input, ActiveBrowserInput::None, "{label}: active_input cleared");
        assert!(bw.is_workspace_occluded(), "{label}: workspace occluded");
        assert!(bw.is_externally_occluded(), "{label}: externally occluded");
        assert_eq!(s.ime.ime_preedit, None, "{label}: ime_preedit cleared");
        assert!(!s.is_browser_workspace_input_active(cx), "{label}: workspace input inactive");
        assert!(!s.is_browser_cdp_viewport_focused(win, cx), "{label}: cdp viewport unfocused");
        if let Some(surf) = bw.cdp_surface() {
            assert!(!surf.read(cx).is_interactive_enabled(), "{label}: interactive disabled");
        }
    };

    // Part A1: Stacked z-order Escape dismissal:
    // pending_miniapp_confirm -> bot_profile_box -> miniapp_tos_modal_open (topmost).
    // Each Esc closes exactly one topmost modal, never exits BrowserWorkspace, and restores CDP typing.
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
            let bw = s.browser_workspace().unwrap().clone();
            let surf = bw.update(scx, |w, wcx| {
                w.set_active_input(ActiveBrowserInput::None, None, wcx);
                w.cdp_surface().cloned().unwrap()
            });
            surf.update(scx, |sr, srcx| sr.focus_handle(srcx).focus(window));
        });
        let _ = window.draw(cx);
        assert!(shell_ent.read(cx).is_browser_cdp_viewport_focused(window, cx));

        shell_ent.update(cx, |s, scx| {
            s.request_launch_miniapp_with_confirm_and_url(
                "ConfirmBot",
                "mini",
                None,
                MiniAppLaunchMode::FullSize,
                None,
                None,
                scx,
            );
            s.open_bot_profile_popup("@HelperBot", scx);
            let box_ent = s.miniapps.bot_profile_box.clone().expect("bot_profile_box");
            box_ent.update(scx, |bx, _| bx.emit(BotProfileBoxAction::OpenMiniAppTos));
            s.drain_bot_profile_box_actions(&box_ent, scx);
        });
        let _ = window.draw(cx);
        {
            let s = shell_ent.read(cx);
            assert!(s.miniapps.pending_miniapp_confirm.is_some());
            assert!(s.miniapps.bot_profile_box.as_ref().is_some_and(|b| b.read(cx).is_visible));
            assert!(s.miniapps.miniapp_tos_modal_open);
        }

        // Esc #1: closes miniapp_tos_modal_open only
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);
        {
            let s = shell_ent.read(cx);
            assert!(!s.miniapps.miniapp_tos_modal_open, "Esc #1 must close ToS modal");
            assert!(s.miniapps.bot_profile_box.as_ref().is_some_and(|b| b.read(cx).is_visible), "bot_profile_box must remain open");
            assert!(s.miniapps.pending_miniapp_confirm.is_some(), "pending_miniapp_confirm must remain open");
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            let bw = s.browser_workspace().unwrap().read(cx);
            assert!(bw.is_workspace_visible());
            assert!(bw.is_workspace_occluded());
        }

        // Esc #2: closes bot_profile_box only
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);
        {
            let s = shell_ent.read(cx);
            assert!(s.miniapps.bot_profile_box.is_none(), "Esc #2 must close bot_profile_box");
            assert!(s.miniapps.pending_miniapp_confirm.is_some(), "pending_miniapp_confirm must remain open");
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            let bw = s.browser_workspace().unwrap().read(cx);
            assert!(bw.is_workspace_visible());
            assert!(bw.is_workspace_occluded());
        }

        // Esc #3: closes pending_miniapp_confirm and un-occludes BrowserWorkspace
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        {
            let s = shell_ent.read(cx);
            assert!(s.miniapps.pending_miniapp_confirm.is_none(), "Esc #3 must close pending_miniapp_confirm");
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
            let bw = s.browser_workspace().unwrap().read(cx);
            assert!(bw.is_workspace_visible());
            assert!(!bw.is_externally_occluded(), "pre-draw occlusion must reconcile on Esc");
        }
        let _ = window.draw(cx);

        // Subsequent CDP typing reaches backend.key_events
        backend.key_events.lock().unwrap().clear();
        shell_ent.update(cx, |s, scx| {
            let surf = s.browser_workspace().unwrap().read(scx).cdp_surface().cloned().unwrap();
            surf.update(scx, |sr, srcx| sr.focus_handle(srcx).focus(window));
        });
        let _ = window.draw(cx);
        window.dispatch_keystroke(Keystroke::parse("k").unwrap(), cx);
    })
    .unwrap();

    let got_k = pump_until(&mut app, Duration::from_secs(2), |_| {
        backend.key_events.lock().unwrap().iter().any(|ev| ev.key == "k")
    })
    .await;
    assert!(got_k, "CDP typing after stacked Esc dismissal must reach backend.key_events");

    // Part A2: Individual single-modal Escape dismissal for each of the 3 modals
    for (label, mode) in [
        ("bot_profile_box", 0u8),
        ("miniapp_tos_modal_open", 1u8),
        ("pending_miniapp_confirm", 2u8),
    ] {
        app.update_window(win_handle.into(), |_, window, cx| {
            backend.key_events.lock().unwrap().clear();
            shell_ent.update(cx, |s, scx| {
                s.open_browser_workspace(scx);
                match mode {
                    0 => s.open_bot_profile_popup("@HelperBot", scx),
                    1 => {
                        s.occlude_browser_for_overlay(scx);
                        s.miniapps.miniapp_tos_modal_open = true;
                        scx.notify();
                    }
                    _ => s.request_launch_miniapp_with_confirm_and_url(
                        "SingleConfirmBot",
                        "mini",
                        None,
                        MiniAppLaunchMode::FullSize,
                        None,
                        None,
                        scx,
                    ),
                }
            });
            let _ = window.draw(cx);
            window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
            {
                let s = shell_ent.read(cx);
                assert!(s.miniapps.bot_profile_box.is_none(), "{label}: bot_profile_box closed");
                assert!(!s.miniapps.miniapp_tos_modal_open, "{label}: miniapp_tos_modal_open closed");
                assert!(s.miniapps.pending_miniapp_confirm.is_none(), "{label}: pending_miniapp_confirm closed");
                assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser, "{label}: stayed in Browser");
                let bw = s.browser_workspace().unwrap().read(cx);
                assert!(bw.is_workspace_visible(), "{label}: BrowserWorkspace stays visible");
                assert!(!bw.is_externally_occluded(), "{label}: BrowserWorkspace un-occluded");
            }
            let _ = window.draw(cx);
            shell_ent.update(cx, |s, scx| {
                let surf = s.browser_workspace().unwrap().read(scx).cdp_surface().cloned().unwrap();
                surf.update(scx, |sr, srcx| sr.focus_handle(srcx).focus(window));
            });
            let _ = window.draw(cx);
            window.dispatch_keystroke(Keystroke::parse("z").unwrap(), cx);
        })
        .unwrap();

        let got_z = pump_until(&mut app, Duration::from_secs(2), |_| {
            backend.key_events.lock().unwrap().iter().any(|ev| ev.key == "z")
        })
        .await;
        assert!(got_z, "{label}: subsequent CDP typing must reach backend.key_events");
    }

    // Part B: BrowserWorkspace::user_cancel_ai while occluded keeps interactive_enabled == false
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.open_browser_workspace(scx);
            let bw = s.browser_workspace().unwrap().clone();
            bw.update(scx, |w, wcx| {
                w.set_occluded(true, wcx);
                w.user_cancel_ai(wcx);
                assert!(
                    !w.cdp_surface().unwrap().read(wcx).is_interactive_enabled(),
                    "user_cancel_ai while occluded must keep cdp_surface.interactive_enabled == false"
                );
                w.set_occluded(false, wcx);
            });
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    // Part C: Minimized Mini-App + queued request_takeover clears is_miniapp_minimized & occludes pre-draw and post-draw
    app.update_window(win_handle.into(), |_, window, cx| {
        shell_ent.update(cx, |s, scx| {
            s.launch_miniapp_for_webmcp(
                MiniAppWebMcpCommand {
                    request_id: "req-bg-open".to_string(),
                    bot_username: "takeover_bot".to_string(),
                    short_name: "mini".to_string(),
                    chat_id: 9118,
                    bot_user_id: 901_111,
                    app_title: "Takeover Mini".to_string(),
                    operation: "discover".to_string(),
                    tool_name: None,
                    arguments: serde_json::json!({}),
                    status_message: None,
                },
                scx,
            );
            assert!(s.miniapps.is_miniapp_minimized, "non-takeover launch starts minimized in PiP");
            assert!(!s.has_browser_visual_occlusion_overlay(scx), "minimized loading PiP must not occlude");
        });
        arm_distill_with_ime("queued_request_takeover", window, cx);
        shell_ent.update(cx, |s, _| {
            s.miniapps.pending_webmcp_commands.push_back(MiniAppWebMcpCommand {
                request_id: "req-takeover-queued".to_string(),
                bot_username: "takeover_bot".to_string(),
                short_name: "mini".to_string(),
                chat_id: 9118,
                bot_user_id: 901_111,
                app_title: "Takeover Mini".to_string(),
                operation: "request_takeover".to_string(),
                tool_name: None,
                arguments: serde_json::json!({}),
                status_message: Some("Need user takeover".to_string()),
            });
        });
    })
    .unwrap();

    let launch_settled = pump_until(&mut app, Duration::from_secs(3), |cx| {
        let s = shell_ent.read(cx);
        s.miniapps
            .miniapp_overlay
            .as_ref()
            .is_some_and(|ov| !ov.read(cx).is_loading)
    })
    .await;
    assert!(launch_settled, "delayed /launch with queued request_takeover must settle");

    app.update_window(win_handle.into(), |_, window, cx| {
        // Strictly BEFORE window.draw(cx):
        {
            let s = shell_ent.read(cx);
            assert!(
                !s.miniapps.is_miniapp_minimized,
                "queued request_takeover must clear is_miniapp_minimized pre-draw"
            );
            assert!(
                s.has_browser_visual_occlusion_overlay(cx),
                "has_browser_visual_occlusion_overlay must be true pre-draw after queued request_takeover"
            );
            assert!(
                s.miniapps.miniapp_overlay.as_ref().is_some_and(|ov| {
                    let w = ov.read(cx);
                    w.is_open && !w.is_floating_pip
                }),
                "miniapp overlay must be open and out of PiP"
            );
        }
        assert_predraw_occluded("queued_request_takeover_predraw", window, cx);

        // Post-draw invariant must also remain true:
        let _ = window.draw(cx);
        {
            let s = shell_ent.read(cx);
            let bw = s.browser_workspace().unwrap().read(cx);
            assert!(s.has_browser_visual_occlusion_overlay(cx), "post-draw occlusion overlay must remain true");
            assert!(bw.is_workspace_occluded(), "post-draw bw.is_workspace_occluded() must remain true");
        }

        // Clean up miniapp overlay
        shell_ent.update(cx, |s, scx| {
            s.miniapps.miniapp_overlay = None;
            s.miniapps.is_miniapp_minimized = false;
            s.miniapps.floating_pip_status = None;
            scx.notify();
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    // Part D: Remaining pre-draw occlude_browser_for_overlay sites:
    // 1. open_remote_workbench_for_node (shell_computers_sync.rs:944)
    // 2. apply_call_updated_event ringing (shell_pts_updates.rs:1856)
    // 3. InspectorAction::OpenMiniAppTos & OpenMedia (shell_panels_sync.rs:532, 183)
    app.update_window(win_handle.into(), |_, window, cx| {
        arm_distill_with_ime("open_remote_workbench_for_node", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.open_remote_workbench_for_node("assist-node-18".to_string(), scx);
        });
        assert!(shell_ent.read(cx).overlays.computer_overlay.is_some());
        assert_predraw_occluded("open_remote_workbench_for_node", window, cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);

        arm_distill_with_ime("apply_call_updated_ringing", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.apply_update_event(
                ChatId(1),
                UpdateEvent::CallUpdated {
                    call_id: "call-ring-18".to_string(),
                    state: "ringing".to_string(),
                    signal: None,
                },
                scx,
            );
        });
        assert!(shell_ent.read(cx).rtc_talk.talk_mode_overlay.is_some());
        assert_predraw_occluded("apply_call_updated_ringing", window, cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);

        arm_distill_with_ime("inspector_open_miniapp_tos", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.handle_inspector_action(InspectorAction::OpenMiniAppTos, scx);
        });
        assert!(shell_ent.read(cx).miniapps.miniapp_tos_modal_open);
        assert_predraw_occluded("inspector_open_miniapp_tos", window, cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);

        arm_distill_with_ime("inspector_open_media", window, cx);
        shell_ent.update(cx, |s, scx| {
            s.handle_inspector_action(
                InspectorAction::OpenMedia {
                    url: "https://example.com/photo18.png".to_string(),
                    is_video: false,
                },
                scx,
            );
        });
        assert!(shell_ent.read(cx).overlays.media_viewer_overlay.is_some());
        assert_predraw_occluded("inspector_open_media", window, cx);
        window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx);
        let _ = window.draw(cx);
    })
    .unwrap();

    let _ = shutdown_tx.send(());
    drop(client);
    drop(seed_client);
}
