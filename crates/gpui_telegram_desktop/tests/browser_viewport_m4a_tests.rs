//! M4A Integration & Unit Tests: `CdpViewportSurface`, `EmbeddedWkWebView`, and `WkWebViewBackend`.
//!
//! Uses `gpui::Application::headless().with_assets(TelegramAssetSource::new())` with
//! macOS `__DATA,__mod_init_func` subprocess re-execution matching the repository desktop suite.

use std::collections::VecDeque;
use std::future::Future;
use std::io::Cursor;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bot_browser::types::BotBrowserError;
use bot_browser::{
    ActionOutcome, BrowserBackendKind, BrowserCookie, BrowserPerformanceMetrics,
    BrowserSessionBackend, BrowserSnapshot, KeyEventKind, KeyInputEvent, NavigationResult,
    PointerButton, PointerEventKind, PointerInputEvent, SameSitePolicy, SemanticLocator,
};
use futures::FutureExt;
use gpui::{
    bounds, point, px, size, App, AppContext, Application, AsyncApp, Bounds, DevicePixels,
    Entity, Focusable,
};
use gpui_telegram_desktop::theme::TelegramAssetSource;
use gpui_telegram_desktop::views::browser::{
    decode_png_to_render_image, map_point_contain, BrowserEngineKind, CdpViewportSurface,
};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};

const CHILD_ENV: &str = "GPUI_M4A_HEADLESS_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_M4A_HEADLESS_TARGET";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

struct TempConfigDirGuard {
    path: PathBuf,
}

impl TempConfigDirGuard {
    fn new(prefix: &str) -> Self {
        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gpui_tg_desktop_m4a_{prefix}_{}_{}",
            std::process::id(),
            seq
        ));
        std::fs::create_dir_all(&dir).expect("create isolated temp config dir");
        Self { path: dir }
    }
}

impl Drop for TempConfigDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
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

fn dispatch_headless_target(target: &str) -> ! {
    type AsyncBody = fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()> + 'static>>;
    let async_fn: AsyncBody = match target {
        "test_headless_cdp_surface_valid_png_decode_and_window_render" => {
            |app| Box::pin(body_valid_png_decode_and_window_render(app))
        }
        "test_headless_cdp_surface_invalid_and_empty_png_preserves_prior_frame" => {
            |app| Box::pin(body_invalid_and_empty_png_preserves_prior_frame(app))
        }
        "test_headless_cdp_surface_non_blocking_slow_backend_and_stale_frame_guard" => {
            |app| Box::pin(body_non_blocking_slow_backend_and_stale_frame_guard(app))
        }
        "test_headless_cdp_surface_input_and_navigation_forwarding" => {
            |app| Box::pin(body_input_and_navigation_forwarding(app))
        }
        "test_headless_cdp_surface_stale_navigation_does_not_overwrite_url_or_title" => {
            |app| Box::pin(body_stale_navigation_does_not_overwrite_url_or_title(app))
        }
        other => {
            eprintln!("unknown m4a headless gpui test target: {other}");
            std::process::exit(101);
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
                        Ok(()) => std::process::exit(0),
                        Err(payload) => {
                            let msg = panic_payload_to_string(payload);
                            eprintln!("[headless async child panic] {t}: {msg}");
                            std::process::exit(101);
                        }
                    }
                })
                .detach();
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

    let cfg_guard = TempConfigDirGuard::new(test_fn_name);
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", &cfg_guard.path)
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

fn make_test_png(width: u32, height: u32, top_left_rgba: [u8; 4]) -> Vec<u8> {
    let mut img = RgbaImage::from_pixel(width, height, Rgba([0, 128, 255, 255]));
    img.put_pixel(0, 0, Rgba(top_left_rgba));
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encode test PNG");
    bytes
}

#[derive(Clone)]
struct ScreenshotResponse {
    delay_ms: u64,
    payload: Result<Vec<u8>, String>,
}

struct MockViewportBackend {
    url: Mutex<String>,
    title: Mutex<String>,
    screenshot_queue: Mutex<VecDeque<ScreenshotResponse>>,
    fallback_screenshot: Mutex<ScreenshotResponse>,
    nav_delay_ms_queue: Mutex<VecDeque<u64>>,
    back_delay_ms_queue: Mutex<VecDeque<u64>>,
    pointer_events: Mutex<Vec<PointerInputEvent>>,
    key_events: Mutex<Vec<KeyInputEvent>>,
    nav_history: Mutex<Vec<String>>,
    back_count: AtomicU64,
    forward_count: AtomicU64,
    reload_count: AtomicU64,
}

impl MockViewportBackend {
    fn new(initial_png: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            url: Mutex::new("https://example.com".to_string()),
            title: Mutex::new("Example".to_string()),
            screenshot_queue: Mutex::new(VecDeque::new()),
            fallback_screenshot: Mutex::new(ScreenshotResponse {
                delay_ms: 0,
                payload: Ok(initial_png),
            }),
            nav_delay_ms_queue: Mutex::new(VecDeque::new()),
            back_delay_ms_queue: Mutex::new(VecDeque::new()),
            pointer_events: Mutex::new(Vec::new()),
            key_events: Mutex::new(Vec::new()),
            nav_history: Mutex::new(Vec::new()),
            back_count: AtomicU64::new(0),
            forward_count: AtomicU64::new(0),
            reload_count: AtomicU64::new(0),
        })
    }

    fn push_nav_delay(&self, delay_ms: u64) {
        self.nav_delay_ms_queue.lock().unwrap().push_back(delay_ms);
    }

    fn push_back_delay(&self, delay_ms: u64) {
        self.back_delay_ms_queue.lock().unwrap().push_back(delay_ms);
    }

    fn push_screenshot(&self, delay_ms: u64, payload: Result<Vec<u8>, String>) {
        self.screenshot_queue
            .lock()
            .unwrap()
            .push_back(ScreenshotResponse { delay_ms, payload });
    }

    fn set_fallback_screenshot(&self, delay_ms: u64, payload: Result<Vec<u8>, String>) {
        *self.fallback_screenshot.lock().unwrap() = ScreenshotResponse { delay_ms, payload };
    }
}

type MockFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl BrowserSessionBackend for MockViewportBackend {
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
            let delay_ms = self.nav_delay_ms_queue.lock().unwrap().pop_front().unwrap_or(0);
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            let trimmed = url.trim().to_string();
            *self.url.lock().unwrap() = trimmed.clone();
            let title = if trimmed.contains("x.com") {
                "Explore / Mock".to_string()
            } else {
                format!("Page: {trimmed}")
            };
            *self.title.lock().unwrap() = title.clone();
            self.nav_history.lock().unwrap().push(trimmed.clone());
            Ok(NavigationResult {
                url: trimmed.clone(),
                origin: trimmed,
                title,
                duration_ms: delay_ms.max(1),
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
            let delay_ms = self.back_delay_ms_queue.lock().unwrap().pop_front().unwrap_or(0);
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            self.back_count.fetch_add(1, Ordering::Relaxed);
            *self.url.lock().unwrap() = "https://example.com/back".to_string();
            *self.title.lock().unwrap() = "Back Page".to_string();
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
            self.forward_count.fetch_add(1, Ordering::Relaxed);
            *self.url.lock().unwrap() = "https://example.com/forward".to_string();
            *self.title.lock().unwrap() = "Forward Page".to_string();
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
            self.reload_count.fetch_add(1, Ordering::Relaxed);
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
                origin: "https://example.com".to_string(),
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
                operation: "click_locator".to_string(),
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
                operation: "fill_locator".to_string(),
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
        delta_x: f64,
        delta_y: f64,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            self.pointer_events.lock().unwrap().push(PointerInputEvent {
                kind: PointerEventKind::Wheel,
                x: 0.0,
                y: 0.0,
                button: PointerButton::None,
                click_count: 0,
                delta_x,
                delta_y,
                modifiers: 0,
            });
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
        _locator: &'life1 SemanticLocator,
        _timeout_ms: u64,
    ) -> MockFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            Ok(ActionOutcome {
                ok: true,
                operation: "wait_for_locator".to_string(),
                duration_ms: 1,
                matched_strategy: None,
                matched_selector: None,
                x: None,
                y: None,
                extracted_text: None,
            })
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
            let next = self
                .screenshot_queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| self.fallback_screenshot.lock().unwrap().clone());
            if next.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(next.delay_ms)).await;
            }
            next.payload.map_err(BotBrowserError::Cdp)
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
        let count = cookies.len();
        Box::pin(async move { Ok(count) })
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
            Ok(())
        })
    }

    fn performance_metrics(&self) -> BrowserPerformanceMetrics {
        BrowserPerformanceMetrics {
            backend: Some(BrowserBackendKind::MockCdp),
            ..Default::default()
        }
    }
}

fn open_surface_in_headless_window(
    app: &mut AsyncApp,
    backend: Arc<dyn BrowserSessionBackend>,
) -> Entity<CdpViewportSurface> {
    app.update(|cx| cx.new(|cx| CdpViewportSurface::new(backend, cx)))
        .expect("create CdpViewportSurface entity in headless GPUI app")
}

async fn body_valid_png_decode_and_window_render(mut app: AsyncApp) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();

    let png = make_test_png(320, 180, [255, 10, 20, 255]);
    let backend = MockViewportBackend::new(png);
    let surface = open_surface_in_headless_window(&mut app, backend);

    let _ = app.update_entity(&surface, |s, cx| {
        assert!(!s.has_rendered_frame());
        assert_eq!(s.last_frame_revision(), 0);
        let _fh = s.focus_handle(cx);
        s.refresh_frame(cx);
    });

    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        let done = app
            .read_entity(&surface, |s, _| s.has_rendered_frame() && !s.is_refreshing())
            .unwrap();
        if done {
            break;
        }
    }

    app.read_entity(&surface, |s, _| {
        assert!(s.has_rendered_frame(), "expected rendered frame");
        assert_eq!(s.last_frame_revision(), 1);
        assert_eq!(s.frame_pixel_size(), Some((320, 180)));
        assert_eq!(s.last_error(), None);
        assert!(!s.is_refreshing());
        let img = s.rendered_image().expect("RenderImage present");
        assert_eq!(
            img.size(0),
            size(DevicePixels(320), DevicePixels(180)),
            "RenderImage dimensions must match PNG"
        );
        let bytes = img.as_bytes(0).expect("BGRA bytes present");
        // RGBA (255, 10, 20, 255) -> BGRA (20, 10, 255, 255)
        assert_eq!(&bytes[0..4], &[20, 10, 255, 255]);
    })
    .unwrap();
}

async fn body_invalid_and_empty_png_preserves_prior_frame(mut app: AsyncApp) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();

    let valid_png = make_test_png(64, 48, [12, 34, 56, 255]);
    let backend = MockViewportBackend::new(valid_png);
    let surface = open_surface_in_headless_window(&mut app, backend.clone());

    let _ = app.update_entity(&surface, |s, cx| s.refresh_frame(cx));
    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if app
            .read_entity(&surface, |s, _| s.has_rendered_frame() && !s.is_refreshing())
            .unwrap()
        {
            break;
        }
    }
    assert_eq!(
        app.read_entity(&surface, |s, _| s.last_frame_revision())
            .unwrap(),
        1
    );

    // 1) Empty PNG payload
    backend.set_fallback_screenshot(0, Ok(Vec::new()));
    let _ = app.update_entity(&surface, |s, cx| s.refresh_frame(cx));
    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if app
            .read_entity(&surface, |s, _| !s.is_refreshing())
            .unwrap()
        {
            break;
        }
    }
    app.read_entity(&surface, |s, _| {
        assert!(s.last_error().is_some(), "empty PNG should set last_error");
        assert!(s.has_rendered_frame(), "prior valid frame must be preserved");
        assert_eq!(s.last_frame_revision(), 1);
        assert_eq!(s.frame_pixel_size(), Some((64, 48)));
    })
    .unwrap();

    // 2) Corrupt non-PNG bytes
    backend.set_fallback_screenshot(0, Ok(b"not-a-valid-png-payload".to_vec()));
    let _ = app.update_entity(&surface, |s, cx| s.refresh_frame(cx));
    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if app
            .read_entity(&surface, |s, _| !s.is_refreshing())
            .unwrap()
        {
            break;
        }
    }
    app.read_entity(&surface, |s, _| {
        assert!(
            s.last_error().is_some(),
            "corrupt PNG should set last_error"
        );
        assert!(s.has_rendered_frame(), "prior valid frame must remain");
        assert_eq!(s.last_frame_revision(), 1);
    })
    .unwrap();
}

async fn body_non_blocking_slow_backend_and_stale_frame_guard(mut app: AsyncApp) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();

    let slow_png = make_test_png(100, 80, [10, 20, 30, 255]);
    let fast_png = make_test_png(200, 120, [40, 50, 60, 255]);
    let backend = MockViewportBackend::new(fast_png.clone());
    // Call 1: slow 240ms; Call 2: fast 5ms
    backend.push_screenshot(240, Ok(slow_png));
    backend.push_screenshot(5, Ok(fast_png));

    let surface = open_surface_in_headless_window(&mut app, backend);

    let tick_start = Instant::now();
    let _ = app.update_entity(&surface, |s, cx| {
        s.refresh_frame(cx); // seq 1 (slow)
    });
    // Verify UI/background timer tick advances immediately without blocking for 240ms.
    app.background_executor()
        .timer(Duration::from_millis(10))
        .await;
    let tick_elapsed = tick_start.elapsed();
    assert!(
        tick_elapsed < Duration::from_millis(120),
        "UI timer tick blocked on slow backend screenshot: {tick_elapsed:?}"
    );
    assert!(
        app.read_entity(&surface, |s, _| s.is_refreshing()).unwrap(),
        "slow refresh should still be in flight"
    );

    // Fire seq 2 (fast) while seq 1 is still sleeping.
    let _ = app.update_entity(&surface, |s, cx| {
        s.refresh_frame(cx);
    });

    // Wait for fast seq 2 to apply.
    for _ in 0..40 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if app
            .read_entity(&surface, |s, _| s.has_rendered_frame())
            .unwrap()
        {
            break;
        }
    }
    app.read_entity(&surface, |s, _| {
        assert_eq!(s.frame_pixel_size(), Some((200, 120)));
        assert_eq!(s.last_frame_revision(), 1);
    })
    .unwrap();

    // Now wait until slow seq 1 finishes completely, and verify it did NOT overwrite seq 2.
    for _ in 0..40 {
        app.background_executor()
            .timer(Duration::from_millis(20))
            .await;
        if !app
            .read_entity(&surface, |s, _| s.is_refreshing())
            .unwrap()
        {
            break;
        }
    }
    app.read_entity(&surface, |s, _| {
        assert!(!s.is_refreshing());
        assert_eq!(
            s.frame_pixel_size(),
            Some((200, 120)),
            "stale slow frame must NOT overwrite newer fast frame"
        );
        assert_eq!(s.last_frame_revision(), 1);
        assert_eq!(s.last_error(), None);
    })
    .unwrap();
}

async fn body_input_and_navigation_forwarding(mut app: AsyncApp) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();

    let png = make_test_png(160, 90, [99, 88, 77, 255]);
    let backend = MockViewportBackend::new(png);
    let surface = open_surface_in_headless_window(&mut app, backend.clone());

    let _ = app.update_entity(&surface, |s, cx| {
        s.navigate_and_refresh("https://x.com/explore", cx);
    });
    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if app
            .read_entity(&surface, |s, _| s.has_rendered_frame() && !s.is_refreshing())
            .unwrap()
        {
            break;
        }
    }
    app.read_entity(&surface, |s, _| {
        assert_eq!(s.current_url(), "https://x.com/explore");
        assert_eq!(s.page_title(), "Explore / Mock");
        assert_eq!(s.last_frame_revision(), 1);
    })
    .unwrap();

    let _ = app.update_entity(&surface, |s, cx| {
        s.handle_click(120.5, 240.25, PointerButton::Left, 1, 8, cx);
        s.handle_mouse_move(55.0, 66.0, PointerButton::None, 0, cx);
        s.handle_scroll(300.0, 400.0, 0.0, 120.0, 2, cx);
        s.handle_key_down("Enter", Some("Enter".into()), None, Some(13), 0, cx);
        s.handle_key_up("Enter", Some("Enter".into()), Some(13), 0, cx);
        s.send_text_input("hello world", cx);
    });

    for _ in 0..60 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        let ptr_len = backend.pointer_events.lock().unwrap().len();
        let key_len = backend.key_events.lock().unwrap().len();
        if ptr_len >= 4 && key_len >= 3 {
            break;
        }
    }

    {
        let ptrs = backend.pointer_events.lock().unwrap();
        assert!(
            ptrs.iter().any(|e| {
                e.kind == PointerEventKind::Down
                    && (e.x - 120.5).abs() < f64::EPSILON
                    && (e.y - 240.25).abs() < f64::EPSILON
                    && e.button == PointerButton::Left
                    && e.modifiers == 8
            }),
            "expected Down event in {ptrs:?}"
        );
        assert!(
            ptrs.iter().any(|e| {
                e.kind == PointerEventKind::Up
                    && (e.x - 120.5).abs() < f64::EPSILON
                    && (e.y - 240.25).abs() < f64::EPSILON
                    && e.button == PointerButton::Left
            }),
            "expected Up event in {ptrs:?}"
        );
        assert!(
            ptrs.iter().any(|e| {
                e.kind == PointerEventKind::Move
                    && (e.x - 55.0).abs() < f64::EPSILON
                    && (e.y - 66.0).abs() < f64::EPSILON
            }),
            "expected Move event in {ptrs:?}"
        );
        assert!(
            ptrs.iter().any(|e| {
                e.kind == PointerEventKind::Wheel
                    && (e.delta_y - 120.0).abs() < f64::EPSILON
                    && e.modifiers == 2
            }),
            "expected Wheel event in {ptrs:?}"
        );
    }

    {
        let keys = backend.key_events.lock().unwrap();
        assert!(
            keys.iter()
                .any(|k| k.kind == KeyEventKind::KeyDown && k.key == "Enter"),
            "expected KeyDown Enter in {keys:?}"
        );
        assert!(
            keys.iter()
                .any(|k| k.kind == KeyEventKind::KeyUp && k.key == "Enter"),
            "expected KeyUp Enter in {keys:?}"
        );
        assert!(
            keys.iter().any(|k| {
                k.kind == KeyEventKind::Char && k.text.as_deref() == Some("hello world")
            }),
            "expected Char 'hello world' in {keys:?}"
        );
    }

    // Disabling interaction ignores handle_click
    let count_before = backend.pointer_events.lock().unwrap().len();
    let _ = app.update_entity(&surface, |s, cx| {
        assert!(s.is_interactive_enabled());
        s.set_interactive_enabled(false);
        assert!(!s.is_interactive_enabled());
        s.handle_click(10.0, 10.0, PointerButton::Left, 1, 0, cx);
    });
    app.background_executor()
        .timer(Duration::from_millis(30))
        .await;
    assert_eq!(
        backend.pointer_events.lock().unwrap().len(),
        count_before,
        "disabled interactive surface must not forward pointer events"
    );

    // History navigation helpers
    let _ = app.update_entity(&surface, |s, cx| s.go_back_and_refresh(cx));
    for _ in 0..40 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if backend.back_count.load(Ordering::Relaxed) == 1
            && app
                .read_entity(&surface, |s, _| !s.is_refreshing())
                .unwrap()
        {
            break;
        }
    }
    assert_eq!(backend.back_count.load(Ordering::Relaxed), 1);

    let _ = app.update_entity(&surface, |s, cx| s.go_forward_and_refresh(cx));
    for _ in 0..40 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if backend.forward_count.load(Ordering::Relaxed) == 1
            && app
                .read_entity(&surface, |s, _| !s.is_refreshing())
                .unwrap()
        {
            break;
        }
    }
    assert_eq!(backend.forward_count.load(Ordering::Relaxed), 1);

    let _ = app.update_entity(&surface, |s, cx| s.reload_and_refresh(cx));
    for _ in 0..40 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if backend.reload_count.load(Ordering::Relaxed) == 1
            && app
                .read_entity(&surface, |s, _| !s.is_refreshing())
                .unwrap()
        {
            break;
        }
    }
    assert_eq!(backend.reload_count.load(Ordering::Relaxed), 1);
}

async fn body_stale_navigation_does_not_overwrite_url_or_title(mut app: AsyncApp) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _rt = rt.enter();

    let slow_png = make_test_png(120, 80, [11, 22, 33, 255]);
    let fast_png = make_test_png(240, 140, [44, 55, 66, 255]);
    let backend = MockViewportBackend::new(fast_png.clone());

    // 1) Stale navigate_and_refresh race: seq 1 slow (220ms) vs seq 2 fast (0ms)
    backend.push_nav_delay(220);
    backend.push_nav_delay(0);
    backend.push_screenshot(0, Ok(fast_png.clone())); // seq 2 finishes first
    backend.push_screenshot(0, Ok(slow_png.clone())); // seq 1 finishes second

    let surface = open_surface_in_headless_window(&mut app, backend.clone());

    let _ = app.update_entity(&surface, |s, cx| {
        s.navigate_and_refresh("https://slow.example/stale", cx);
        s.navigate_and_refresh("https://fast.example/current", cx);
    });

    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if app
            .read_entity(&surface, |s, _| !s.is_refreshing())
            .unwrap()
        {
            break;
        }
    }

    app.read_entity(&surface, |s, _| {
        assert!(!s.is_refreshing());
        assert_eq!(
            s.current_url(),
            "https://fast.example/current",
            "stale slow navigate_and_refresh must not overwrite newer URL"
        );
        assert_eq!(
            s.page_title(),
            "Page: https://fast.example/current",
            "stale slow navigate_and_refresh must not overwrite newer title"
        );
        assert_eq!(s.frame_pixel_size(), Some((240, 140)));
    })
    .unwrap();

    // 2) Stale go_back_and_refresh race: seq 3 slow go_back (220ms) vs seq 4 fast navigate (0ms)
    let latest_png = make_test_png(300, 160, [77, 88, 99, 255]);
    backend.push_back_delay(220);
    backend.push_nav_delay(0);
    backend.push_screenshot(0, Ok(latest_png));
    backend.push_screenshot(0, Ok(slow_png));

    let _ = app.update_entity(&surface, |s, cx| {
        s.go_back_and_refresh(cx);
        s.navigate_and_refresh("https://newer.example/final", cx);
    });

    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
        if app
            .read_entity(&surface, |s, _| !s.is_refreshing())
            .unwrap()
        {
            break;
        }
    }

    app.read_entity(&surface, |s, _| {
        assert!(!s.is_refreshing());
        assert_eq!(
            s.current_url(),
            "https://newer.example/final",
            "stale go_back_and_refresh must not overwrite newer URL"
        );
        assert_eq!(
            s.page_title(),
            "Page: https://newer.example/final",
            "stale go_back_and_refresh must not overwrite newer title"
        );
        assert_eq!(s.frame_pixel_size(), Some((300, 160)));
    })
    .unwrap();

    // 3) Fallback map_window_point_to_viewport (before bounds are recorded) sanitizes NaN
    let _ = app.update_entity(&surface, |s, _| {
        s.set_viewport_bounds_for_test(None);
        let (mx, my) = s.map_window_point_to_viewport(point(px(f32::NAN), px(120.0)));
        assert!(mx.is_finite() && (0.0..=1280.0).contains(&mx));
        assert!(my.is_finite() && (0.0..=800.0).contains(&my));
    });
}

#[test]
fn test_headless_cdp_surface_valid_png_decode_and_window_render() {
    run_in_headless_subprocess("test_headless_cdp_surface_valid_png_decode_and_window_render");
}

#[test]
fn test_headless_cdp_surface_invalid_and_empty_png_preserves_prior_frame() {
    run_in_headless_subprocess(
        "test_headless_cdp_surface_invalid_and_empty_png_preserves_prior_frame",
    );
}

#[test]
fn test_headless_cdp_surface_non_blocking_slow_backend_and_stale_frame_guard() {
    run_in_headless_subprocess(
        "test_headless_cdp_surface_non_blocking_slow_backend_and_stale_frame_guard",
    );
}

#[test]
fn test_headless_cdp_surface_input_and_navigation_forwarding() {
    run_in_headless_subprocess("test_headless_cdp_surface_input_and_navigation_forwarding");
}

#[test]
fn test_headless_cdp_surface_stale_navigation_does_not_overwrite_url_or_title() {
    run_in_headless_subprocess(
        "test_headless_cdp_surface_stale_navigation_does_not_overwrite_url_or_title",
    );
}

#[test]
fn test_cdp_surface_contain_coordinate_mapping_and_png_decoder_unit() {
    // 1. Vertical letterboxing: 1000x500 image inside 200x200 box at origin (10, 20)
    // Image occupies x=10..210, y=70..170 (height=100)
    let vp_bounds: Bounds<gpui::Pixels> =
        bounds(point(px(10.0), px(20.0)), size(px(200.0), px(200.0)));
    let (cx, cy) = map_point_contain(
        point(px(110.0), px(120.0)),
        vp_bounds,
        (1000, 500),
        (1000, 500),
    );
    assert!((cx - 500.0).abs() < 1e-6);
    assert!((cy - 250.0).abs() < 1e-6);

    // Point in top letterbox bar clamps to y = 0.0
    let (tx, ty) = map_point_contain(
        point(px(110.0), px(35.0)),
        vp_bounds,
        (1000, 500),
        (1000, 500),
    );
    assert!((tx - 500.0).abs() < 1e-6);
    assert!((ty - 0.0).abs() < 1e-6);

    // 2. Horizontal pillarboxing: 500x1000 image inside 400x200 box at origin (0, 0)
    // Image occupies x=150..250 (width=100), y=0..200
    let wide_bounds: Bounds<gpui::Pixels> =
        bounds(point(px(0.0), px(0.0)), size(px(400.0), px(200.0)));
    let (px_x, px_y) = map_point_contain(
        point(px(150.0), px(100.0)),
        wide_bounds,
        (500, 1000),
        (500, 1000),
    );
    assert!((px_x - 0.0).abs() < 1e-6);
    assert!((px_y - 500.0).abs() < 1e-6);

    let (px_x2, px_y2) = map_point_contain(
        point(px(200.0), px(100.0)),
        wide_bounds,
        (500, 1000),
        (500, 1000),
    );
    assert!((px_x2 - 250.0).abs() < 1e-6);
    assert!((px_y2 - 500.0).abs() < 1e-6);

    // 2b. Non-finite coordinates (NaN / Inf / -Inf) must sanitize to finite clamped bounds
    let (nx_nan, ny_nan) = map_point_contain(
        point(px(f32::NAN), px(100.0)),
        wide_bounds,
        (500, 1000),
        (500, 1000),
    );
    assert!(nx_nan.is_finite() && (0.0..=500.0).contains(&nx_nan));
    assert!(ny_nan.is_finite() && (0.0..=1000.0).contains(&ny_nan));

    let (nx_inf, ny_ninf) = map_point_contain(
        point(px(f32::INFINITY), px(f32::NEG_INFINITY)),
        vp_bounds,
        (1000, 500),
        (1000, 500),
    );
    assert!(nx_inf.is_finite() && (0.0..=1000.0).contains(&nx_inf));
    assert!(ny_ninf.is_finite() && (0.0..=500.0).contains(&ny_ninf));

    let nan_bounds: Bounds<gpui::Pixels> =
        bounds(point(px(f32::NAN), px(f32::INFINITY)), size(px(f32::NAN), px(0.0)));
    let (bx, by) = map_point_contain(
        point(px(f32::NAN), px(f32::NAN)),
        nan_bounds,
        (0, 0),
        (1280, 800),
    );
    assert!(bx.is_finite() && (0.0..=1280.0).contains(&bx));
    assert!(by.is_finite() && (0.0..=800.0).contains(&by));

    // 3. Direct PNG decoder checks
    assert!(decode_png_to_render_image(&[]).is_err());
    assert!(decode_png_to_render_image(b"invalid").is_err());
    let valid = make_test_png(4, 3, [10, 20, 30, 255]);
    let (img, dims) = decode_png_to_render_image(&valid).expect("decode valid png");
    assert_eq!(dims, (4, 3));
    assert_eq!(&img.as_bytes(0).unwrap()[0..4], &[30, 20, 10, 255]);

    assert_eq!(BrowserEngineKind::WkWebView.display_name(), "WKWebView");
    assert_eq!(BrowserEngineKind::RustCdp.display_name(), "Rust CDP");
    assert!(BrowserEngineKind::WkWebView.is_native_wkwebview());
    assert!(BrowserEngineKind::RustCdp.supports_cdp_surface());
}

#[cfg(target_os = "macos")]
#[test]
fn test_wkwebview_config_recorder_redaction_cookies_and_closed_backend() {
    use gpui_telegram_desktop::components::bots::macos_wkwebview::{
        parse_recorded_dom_event, validate_and_normalize_wk_cookie, validate_wk_navigation_url,
        SharedWebViewState, WkWebViewBackend, WkWebViewConfig, WK_DOM_RECORDER_JS,
    };

    // 1. Config profiles
    let mini = WkWebViewConfig::miniapp_default();
    assert!(mini.inject_telegram_proxy);
    assert!(!mini.enable_dom_recorder);
    assert!(!mini.load_popups_in_same_tab);

    let browser = WkWebViewConfig::browser_profile("x-work");
    assert_eq!(browser.profile_id.as_deref(), Some("x-work"));
    assert!(!browser.inject_telegram_proxy);
    assert!(browser.enable_dom_recorder);
    assert!(browser.load_popups_in_same_tab);

    // 2. Recorder JS & Rust sensitive-input redaction
    assert!(WK_DOM_RECORDER_JS.contains("gpuiBrowserRecorder"));
    assert!(WK_DOM_RECORDER_JS.contains("[REDACTED]"));

    let pwd_payload = serde_json::json!({
        "event_kind": "input",
        "locator": {
            "name_attr": "password",
            "css_selector": "input[name=\"password\"]",
            "text_anchor": "secret-val"
        },
        "value": "hunter2",
        "is_sensitive": false,
        "page_url": "https://x.com/i/flow/login",
        "timestamp_ms": 1700000000000_i64
    });
    let parsed_pwd = parse_recorded_dom_event(&pwd_payload).expect("parse recorded event");
    assert!(parsed_pwd.is_sensitive);
    assert_eq!(parsed_pwd.value.as_deref(), Some("[REDACTED]"));
    assert_eq!(parsed_pwd.locator.text_anchor.as_deref(), Some("[REDACTED]"));

    let safe_payload = serde_json::json!({
        "event_kind": "input",
        "locator": {
            "test_id": "SearchBox_Search_Input",
            "role": "textbox"
        },
        "value": "rust gpui",
        "is_sensitive": false,
        "page_url": "https://x.com/explore",
        "timestamp_ms": 1700000000001_i64
    });
    let parsed_safe = parse_recorded_dom_event(&safe_payload).expect("parse safe event");
    assert!(!parsed_safe.is_sensitive);
    assert_eq!(parsed_safe.value.as_deref(), Some("rust gpui"));

    // 3. Cookie validation & normalization
    let mut good_cookie = BrowserCookie::new(" auth_token ", "tok123", " X.COM ");
    good_cookie.path = String::new();
    good_cookie.same_site = Some(SameSitePolicy::Lax);
    validate_and_normalize_wk_cookie(&mut good_cookie).expect("valid cookie");
    assert_eq!(good_cookie.name, "auth_token");
    assert_eq!(good_cookie.domain, "x.com");
    assert_eq!(good_cookie.path, "/");
    assert!(good_cookie.session);

    let mut bad_cookie = BrowserCookie::new("bad=name", "v", "x.com");
    assert!(validate_and_normalize_wk_cookie(&mut bad_cookie).is_err());
    let mut bad_semi = BrowserCookie::new("ok", "a;b", "x.com");
    assert!(validate_and_normalize_wk_cookie(&mut bad_semi).is_err());

    // 4. URL validation
    assert!(validate_wk_navigation_url("https://x.com/home", false).is_ok());
    assert!(validate_wk_navigation_url("http://169.254.169.254/latest/meta-data", false).is_err());
    assert!(validate_wk_navigation_url("https://metadata.google.internal/", false).is_err());
    assert!(validate_wk_navigation_url("file:///etc/passwd", false).is_err());
    assert!(validate_wk_navigation_url("file:///tmp/test.html", true).is_ok());

    // 5. Closed WkWebViewBackend returns clean Err without hanging
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let backend = WkWebViewBackend::from_shared(SharedWebViewState::new_detached_closed_for_test());
    assert_eq!(backend.backend_kind(), BrowserBackendKind::WkWebView);
    assert!(!backend.is_alive());

    rt.block_on(async {
        let loc = SemanticLocator::from_test_id("tweetButton");
        assert!(backend.navigate("https://x.com").await.is_err());
        assert!(backend.go_back().await.is_err());
        assert!(backend.go_forward().await.is_err());
        assert!(backend.reload().await.is_err());
        assert!(backend.current_url().await.is_err());
        assert!(backend.page_title().await.is_err());
        assert!(backend.snapshot_dom().await.is_err());
        assert!(backend.eval_js("1+1").await.is_err());
        assert!(backend.click_locator(&loc).await.is_err());
        assert!(backend.fill_locator(&loc, "hi", false).await.is_err());
        assert!(backend.press_key("Enter").await.is_err());
        assert!(backend.scroll(0.0, 100.0).await.is_err());
        assert!(backend.wait_for_locator(&loc, 50).await.is_err());
        assert!(backend.capture_screenshot().await.is_err());
        assert!(backend.export_cookies(None).await.is_err());
        assert!(backend.import_cookies(&[good_cookie]).await.is_err());
        assert!(backend
            .dispatch_pointer_event(PointerInputEvent {
                kind: PointerEventKind::Down,
                x: 10.0,
                y: 10.0,
                button: PointerButton::Left,
                click_count: 1,
                delta_x: 0.0,
                delta_y: 0.0,
                modifiers: 0,
            })
            .await
            .is_err());
        assert!(backend
            .dispatch_key_event(KeyInputEvent {
                kind: KeyEventKind::KeyDown,
                key: "Enter".to_string(),
                code: None,
                text: None,
                windows_virtual_key_code: None,
                modifiers: 0,
            })
            .await
            .is_err());
        assert!(backend.close().await.is_ok());
    });
}

#[cfg(target_os = "macos")]
#[test]
fn test_wkwebview_remediation_redaction_locator_targeting_cookies_ssrf_and_pointer() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use gpui_telegram_desktop::components::bots::macos_wkwebview::{
        build_wk_locator_action_js, build_wk_resolve_locator_js, has_sensitive_marker_text,
        parse_recorded_dom_event, validate_wk_navigation_url, wk_ns_cookie_domain,
        SharedWebViewState, WkLocatorAction, WkWebViewBackend, WK_CAPTURE_SNAPSHOT_JS,
        WK_DOM_RECORDER_JS,
    };

    // 1. Normalized sensitivity detection across separators & camelCase
    for marker in [
        "API Key",
        "api-key",
        "api_key",
        "apikey",
        "apiKey",
        "Private Key",
        "private-key",
        "Secret Key",
        "secret-key",
        "access-token",
        "access token",
        "auth-token",
        "auth token",
        "session-id",
        "session id",
        "sessionId",
        "passcode",
        "totp",
        "otp",
        "ct0",
        "authorization",
        "cookie",
        "credential",
        "mnemonic",
        "seed",
        "password",
    ] {
        assert!(
            has_sensitive_marker_text(marker),
            "expected sensitive marker match for `{marker}`"
        );
    }
    for benign in ["SearchBox_Search_Input", "Submit", "Cancel", "Email", "shippingAddress"] {
        assert!(
            !has_sensitive_marker_text(benign),
            "unexpected sensitive match for benign `{benign}`"
        );
    }

    // 2. parse_recorded_dom_event checks ALL locator fields and redacts value + text_anchor
    let locator_field_cases = vec![
        serde_json::json!({ "aria_label": "API Key", "text_anchor": "sk-111" }),
        serde_json::json!({ "name_attr": "api-key", "text_anchor": "sk-222" }),
        serde_json::json!({ "test_id": "Private Key", "text_anchor": "priv-333" }),
        serde_json::json!({ "css_selector": "input[name=\"secret-key\"]", "text_anchor": "sec-444" }),
        serde_json::json!({ "role": "auth-token", "text_anchor": "tok-555" }),
        serde_json::json!({ "text_anchor": "Enter access token" }),
        serde_json::json!({ "aria_label": "session-id", "text_anchor": "sess-666" }),
        serde_json::json!({ "name_attr": "mnemonic", "text_anchor": "twelve words" }),
    ];
    for loc in locator_field_cases {
        let ev = serde_json::json!({
            "event_kind": "input",
            "locator": loc,
            "value": "raw-secret-value",
            "is_sensitive": false,
            "page_url": "https://example.com/settings",
            "timestamp_ms": 1700000000123_i64
        });
        let parsed = parse_recorded_dom_event(&ev).expect("parse sensitive recorded event");
        assert!(parsed.is_sensitive, "expected is_sensitive=true for {ev}");
        assert_eq!(parsed.value.as_deref(), Some("[REDACTED]"));
        assert_eq!(parsed.locator.text_anchor.as_deref(), Some("[REDACTED]"));
    }

    // 3. host_only: false cookie domain preservation for WKHTTPCookieStore
    let mut sub_cookie = BrowserCookie::new("auth_token", "tok", "x.com");
    assert!(!sub_cookie.host_only);
    assert_eq!(wk_ns_cookie_domain(&sub_cookie), ".x.com");
    sub_cookie.domain = ".x.com".to_string();
    assert_eq!(wk_ns_cookie_domain(&sub_cookie), ".x.com");
    sub_cookie.host_only = true;
    assert_eq!(wk_ns_cookie_domain(&sub_cookie), "x.com");

    // 4. Cloud metadata SSRF bypasses blocked in validate_wk_navigation_url
    for blocked in [
        "https://metadata.google.internal./",
        "https://metadata.google.internal./computeMetadata/v1/",
        "https://metadata.goog./",
        "http://[::ffff:169.254.169.254]/latest/meta-data",
        "http://[fd00:ec2::254]/latest/meta-data",
        "http://169.254.10.20/metadata",
    ] {
        assert!(
            validate_wk_navigation_url(blocked, false).is_err(),
            "expected SSRF block for `{blocked}`"
        );
    }
    assert!(validate_wk_navigation_url("http://127.0.0.1:8080/mock", false).is_ok());
    assert!(validate_wk_navigation_url("https://x.com/home", false).is_ok());

    // 5. dispatch_pointer_event rejects non-finite coordinates/deltas immediately
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let closed_backend = WkWebViewBackend::from_shared(SharedWebViewState::new_detached_closed_for_test());
    assert!(closed_backend.is_alive() == false);
    // Construct an alive detached state so ensure_alive() passes and finite validation is exercised.
    let alive_shared = SharedWebViewState::new_detached_closed_for_test();
    alive_shared.alive.store(true, std::sync::atomic::Ordering::Release);
    let alive_detached = WkWebViewBackend::from_shared(alive_shared);
    rt.block_on(async {
        let err = alive_detached
            .dispatch_pointer_event(PointerInputEvent {
                kind: PointerEventKind::Down,
                x: f64::NAN,
                y: 10.0,
                button: PointerButton::Left,
                click_count: 1,
                delta_x: 0.0,
                delta_y: 0.0,
                modifiers: 0,
            })
            .await
            .expect_err("NaN pointer coordinate must fail");
        assert!(
            err.to_string().contains("finite coordinates"),
            "unexpected error: {err}"
        );
    });

    // 6. Execute WK_DOM_RECORDER_JS, WK_CAPTURE_SNAPSHOT_JS, build_wk_resolve_locator_js, and
    //    build_wk_locator_action_js in Node.js against a real DOM tree to prove zero secret leaks
    //    and exact element targeting.
    let resolve_submit_js = build_wk_resolve_locator_js(
        &SemanticLocator::by_text("Submit"),
        true,
        false,
    )
    .expect("build resolve submit js");
    let click_submit_js = build_wk_locator_action_js(
        &SemanticLocator::by_text("Submit"),
        true,
        false,
        WkLocatorAction::Click,
    )
    .expect("build click submit js");
    let fill_email_js = build_wk_locator_action_js(
        &SemanticLocator {
            aria_label: Some("Email".to_string()),
            ..Default::default()
        },
        true,
        true,
        WkLocatorAction::Fill {
            value: "alice@example.com",
            allow_sensitive: false,
        },
    )
    .expect("build fill email js");
    let fill_api_key_refused_js = build_wk_locator_action_js(
        &SemanticLocator {
            aria_label: Some("API Key".to_string()),
            ..Default::default()
        },
        true,
        true,
        WkLocatorAction::Fill {
            value: "should-not-fill",
            allow_sensitive: false,
        },
    )
    .expect("build fill api key refused js");

    let node_harness = format!(
        r#"
const assert = require('node:assert');

class FakeEl {{
  constructor(tag, attrs = {{}}, text = '') {{
    this.nodeType = 1;
    this.tagName = tag.toUpperCase();
    this._attrs = Object.assign({{}}, attrs);
    this.children = [];
    this.parentElement = null;
    this.value = attrs.value || '';
    this._text = text;
    this.clickedCount = 0;
    this.focused = false;
    this.events = [];
  }}
  get id() {{ return this._attrs.id || ''; }}
  get type() {{ return this._attrs.type || ''; }}
  get isContentEditable() {{
    const ce = this._attrs.contenteditable;
    return ce === 'true' || ce === '' || ce === 'plaintext-only';
  }}
  get innerText() {{ return this.children.length ? this.children.map(c => c.innerText).join(' ') : this._text; }}
  get textContent() {{ return this.innerText; }}
  set textContent(v) {{ this._text = String(v); }}
  get previousElementSibling() {{
    if (!this.parentElement) return null;
    const i = this.parentElement.children.indexOf(this);
    return i > 0 ? this.parentElement.children[i - 1] : null;
  }}
  getAttribute(k) {{ return Object.prototype.hasOwnProperty.call(this._attrs, k) ? this._attrs[k] : null; }}
  hasAttribute(k) {{ return Object.prototype.hasOwnProperty.call(this._attrs, k); }}
  appendChild(c) {{ c.parentElement = this; this.children.push(c); return c; }}
  getBoundingClientRect() {{ return {{ x: 10, y: 10, width: 100, height: 30 }}; }}
  scrollIntoView() {{}}
  focus() {{ this.focused = true; }}
  click() {{ this.clickedCount += 1; }}
  dispatchEvent(ev) {{ this.events.push(ev && ev.type); return true; }}
  querySelectorAll(sel) {{ return walk(this).filter(el => el !== this && matchSelector(el, sel)); }}
}}

const htmlEl = new FakeEl('html');
const bodyEl = new FakeEl('body');
htmlEl.appendChild(bodyEl);

// Ambiguous buttons & inputs (no id/testid/name)
const btnCancel = new FakeEl('button', {{}}, 'Cancel');
const btnSubmit = new FakeEl('button', {{}}, 'Submit');
const inpFirst = new FakeEl('input', {{ placeholder: 'First', type: 'text' }});
const inpEmail = new FakeEl('input', {{ 'aria-label': 'Email', type: 'text' }});

// Sensitive fields
const inpApiKey = new FakeEl('input', {{ 'aria-label': 'API Key', type: 'text', value: 'sk-live-secret-999' }});
const inpApiHyphen = new FakeEl('input', {{ name: 'api-key', type: 'text', value: 'sk-hyphen-secret' }});
const secContainer = new FakeEl('div', {{ 'data-testid': 'api_key_container', 'aria-label': 'Secret Token Box' }});
const ceSecret = new FakeEl('div', {{ contenteditable: 'true' }}, 'sk-container-leak-777');
secContainer.appendChild(ceSecret);

const lblFor = new FakeEl('label', {{ for: 'priv1' }}, 'Private Key');
const inpPrivFor = new FakeEl('input', {{ id: 'priv1', type: 'text', value: 'sk-label-secret' }});
const lblRef = new FakeEl('span', {{ id: 'lbl_tok' }}, 'Access Token');
const inpLabelledBy = new FakeEl('input', {{ 'aria-labelledby': 'lbl_tok', type: 'text', value: 'sk-labelledby-secret' }});
const inpSafe = new FakeEl('input', {{ 'aria-label': 'Search', type: 'text', value: 'rust gpui' }});

for (const c of [btnCancel, btnSubmit, inpFirst, inpEmail, inpApiKey, inpApiHyphen, secContainer, lblFor, inpPrivFor, lblRef, inpLabelledBy, inpSafe]) {{
  bodyEl.appendChild(c);
}}

function walk(node, out = []) {{
  out.push(node);
  for (const ch of node.children) walk(ch, out);
  return out;
}}

function matchSinglePart(el, part) {{
  part = part.trim();
  if (part === '*') return true;
  if (part.startsWith('#')) return el.id === part.slice(1).replace(/\\/g, '');
  let m = part.match(/^([a-z0-9]+):nth-of-type\((\d+)\)$/i);
  if (m) {{
    if (el.tagName.toLowerCase() !== m[1].toLowerCase()) return false;
    let idx = 1;
    let sib = el.previousElementSibling;
    while (sib) {{ if (sib.tagName === el.tagName) idx++; sib = sib.previousElementSibling; }}
    return idx === Number(m[2]);
  }}
  m = part.match(/^\[data-testid="([^"]+)"\]$/);
  if (m) return el.getAttribute('data-testid') === m[1];
  m = part.match(/^([a-z0-9]+)\[name="([^"]+)"\]$/i);
  if (m) return el.tagName.toLowerCase() === m[1].toLowerCase() && el.getAttribute('name') === m[2];
  if (part === 'label[for]') return el.tagName === 'LABEL' && el.hasAttribute('for');
  if (part === '[contenteditable]:not([contenteditable="false"])') return el.isContentEditable;
  if (part.includes(',')) return part.split(',').some(sub => matchSinglePart(el, sub));
  return el.tagName.toLowerCase() === part.toLowerCase();
}}

function matchSelector(el, sel) {{
  sel = sel.trim();
  if (sel.includes(',') && !sel.includes('>')) {{
    return sel.split(',').some(sub => matchSinglePart(el, sub));
  }}
  const parts = sel.split('>').map(s => s.trim());
  let cur = el;
  for (let i = parts.length - 1; i >= 0; i--) {{
    if (!cur || !matchSinglePart(cur, parts[i])) return false;
    cur = cur.parentElement;
  }}
  return true;
}}

const listeners = {{}};
const recorded = [];
global.Event = class Event {{ constructor(t) {{ this.type = t; }} }};
global.MouseEvent = class MouseEvent {{ constructor(t) {{ this.type = t; }} }};
global.HTMLInputElement = function() {{}};
global.HTMLInputElement.prototype = {{}};
global.document = {{
  body: bodyEl,
  documentElement: htmlEl,
  title: 'Test Page',
  readyState: 'complete',
  querySelectorAll(sel) {{ return walk(htmlEl).filter(el => matchSelector(el, sel)); }},
  querySelector(sel) {{ return this.querySelectorAll(sel)[0] || null; }},
  getElementById(id) {{ return walk(htmlEl).find(el => el.id === id) || null; }},
  addEventListener(kind, fn) {{ listeners[kind] = fn; }}
}};
global.window = {{
  location: {{ href: 'https://example.com', origin: 'https://example.com' }},
  devicePixelRatio: 1,
  CSS: {{ escape: s => s }},
  getComputedStyle: () => ({{ display: 'block', visibility: 'visible', opacity: '1' }}),
  HTMLInputElement: global.HTMLInputElement,
  webkit: {{
    messageHandlers: {{
      gpuiBrowserRecorder: {{ postMessage(p) {{ recorded.push(p); }} }}
    }}
  }}
}};

// 1) Recorder JS
{recorder_js}
global.window.__gpuiDomRecorderEnabled = true;
listeners.input({{ target: inpApiKey }});
listeners.input({{ target: inpApiHyphen }});
listeners.input({{ target: ceSecret }});
listeners.click({{ target: ceSecret }});
listeners.input({{ target: inpPrivFor }});
listeners.input({{ target: inpLabelledBy }});
listeners.input({{ target: inpSafe }});

assert.strictEqual(recorded.length, 7);
for (let i = 0; i < 6; i++) {{
  assert.strictEqual(recorded[i].is_sensitive, true, 'expected sensitive for index ' + i);
  if (recorded[i].event_kind === 'input') {{
    assert.strictEqual(recorded[i].value, '[REDACTED]');
  }}
  if (recorded[i].locator.text_anchor !== null) {{
    assert.strictEqual(recorded[i].locator.text_anchor, '[REDACTED]');
  }}
}}
assert.strictEqual(recorded[6].is_sensitive, false);
assert.strictEqual(recorded[6].value, 'rust gpui');

// 2) Snapshot JS
const snap = JSON.parse({snapshot_js});
assert(!snap.text_excerpt.includes('sk-container-leak-777'), 'text_excerpt leaked contenteditable secret');
assert(!snap.text_excerpt.includes('sk-live-secret-999'), 'text_excerpt leaked aria API Key secret');
assert(!snap.text_excerpt.includes('sk-hyphen-secret'), 'text_excerpt leaked api-key secret');
assert(!snap.text_excerpt.includes('sk-label-secret'), 'text_excerpt leaked label[for] secret');
assert(!snap.text_excerpt.includes('sk-labelledby-secret'), 'text_excerpt leaked aria-labelledby secret');
const ceSnapNode = snap.interactive_elements.find(n => n.tag === 'div' && n.text === '[REDACTED]' && n.test_id === null);
assert(ceSnapNode && ceSnapNode.is_password === true && ceSnapNode.value === '[REDACTED]', 'inner contenteditable must be redacted');
const safeSnapNode = snap.interactive_elements.find(n => n.aria_label === 'Search');
assert(safeSnapNode && !safeSnapNode.is_password && safeSnapNode.value === 'rust gpui');

// 3) Unique selector & direct targeting in resolve/click/fill
const resolvedSubmit = JSON.parse({resolve_submit_js});
assert.strictEqual(resolvedSubmit.found, true);
assert.notStrictEqual(resolvedSubmit.selector, 'button');
assert.strictEqual(document.querySelectorAll(resolvedSubmit.selector).length, 1);
assert.strictEqual(document.querySelector(resolvedSubmit.selector), btnSubmit);

const clickSubmitRes = JSON.parse({click_submit_js});
assert.strictEqual(clickSubmitRes.acted, true);
assert.strictEqual(btnCancel.clickedCount, 0, 'Cancel must NOT be clicked');
assert.strictEqual(btnSubmit.clickedCount, 1, 'Submit MUST be clicked');

const fillEmailRes = JSON.parse({fill_email_js});
assert.strictEqual(fillEmailRes.acted, true);
assert.strictEqual(inpFirst.value, '', 'First input must NOT be filled');
assert.strictEqual(inpEmail.value, 'alice@example.com', 'Email input MUST be filled');

const fillRefusedRes = JSON.parse({fill_api_key_refused_js});
assert.strictEqual(fillRefusedRes.refused_sensitive, true);
assert.strictEqual(fillRefusedRes.acted, false);
assert.strictEqual(inpApiKey.value, 'sk-live-secret-999', 'Refused fill must not overwrite field');
console.log('NODE_DOM_PROBES_OK');
"#,
        recorder_js = WK_DOM_RECORDER_JS,
        snapshot_js = WK_CAPTURE_SNAPSHOT_JS,
        resolve_submit_js = resolve_submit_js,
        click_submit_js = click_submit_js,
        fill_email_js = fill_email_js,
        fill_api_key_refused_js = fill_api_key_refused_js,
    );

    let mut child = Command::new("node")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn node to verify WKWebView JS contracts");
    child
        .stdin
        .take()
        .expect("node stdin")
        .write_all(node_harness.as_bytes())
        .expect("write JS harness to node stdin");
    let output = child.wait_with_output().expect("wait for node harness");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("NODE_DOM_PROBES_OK"),
        "Node.js DOM verification failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );
}
