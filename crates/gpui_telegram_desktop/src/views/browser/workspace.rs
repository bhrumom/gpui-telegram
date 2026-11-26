//! Split-pane `BrowserWorkspace` GPUI view (`F7.3`):
//! - Top bar: Back/Forward/Reload, URL/address bar, Profile selector, Engine toggle (`WKWebView` / `Rust CDP`),
//!   and `BrowserControlMode` takeover banner (`Take Over`, `Resume AI`, `Cancel AI`).
//! - Left/Center Viewport: `EmbeddedWkWebView` on live macOS (when `WkWebView` active and not headless/offline)
//!   or `CdpViewportSurface` (`RenderImage` + pointer/key/scroll input) on `RustCdp`/headless/non-macOS.
//! - Right Collapsible Inspector: Tabs for Copilot, Recorder, Session, and App Store (`F8.1`, `F8.2`).

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use bot_browser::e2ee_sync::UnlockedUserVaultKey;
use bot_browser::orchestrator::BotBrowserOrchestrator;
use bot_browser::{
    import_cookies_from_chrome_sqlite, import_cookies_from_text, probe_local_chrome_cookies,
    ActionOutcome, AutomationScriptApp, BotBrowserError, BrowserBackendKind, BrowserControlMode,
    BrowserCookie, BrowserPerformanceMetrics, BrowserProfileStore, BrowserSessionBackend,
    BrowserSnapshot, ChromeImportOptions, ChromeImportProbe, CredentialVault,
    CredentialVaultSecretResolver, KeyInputEvent, NavigationResult, OffscreenFrameBuffer,
    PointerInputEvent, ScriptAppMetadata, ScriptReplayReport, SemanticLocator,
    SystemChromeKeyProvider, VaultSecretResolver,
};
use chrono::Utc;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_agent::fabushi_bot::FabushiBotHost;
use gpui_telegram_agent::hermes::browser_session::{
    BrowserAutomationEvent, BrowserAutomationSession, BrowserSessionRegistry,
};
use gpui_telegram_agent::hermes::toolsets::ToolsetId;
use gpui_telegram_agent::hermes::HermesTurnRequest;
use gpui_telegram_client::{HttpClientError, TelegramClient};
use gpui_telegram_proto::automation_apps::{
    InstalledAutomationApp, RecordAutomationRunRequest,
};
use parking_lot::RwLock as SyncRwLock;

#[cfg(target_os = "macos")]
use crate::components::bots::{
    EmbeddedWkWebView, RecordedDomEvent, WkWebViewConfig, WkWebViewEvent,
};
use crate::components::ui::{pop_last_char, sanitize_paste_text};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::browser::design_mode::{
    render_design_mode_overlay, render_design_mode_tab, BrowserWorkspaceEvent, DesignHandoffTarget,
    DesignModePanelState,
};
use crate::views::browser::recorder_panel::{
    format_locator_summary, render_copilot_tab, render_recorder_tab, render_session_tab,
    BrowserLogLevel, ChromeProbeUiState, RecorderPanelState,
};
use crate::views::browser::store_panel::{render_store_tab, StorePanelState};
use crate::views::browser::{
    decode_png_to_render_image, map_point_contain, BrowserEngineKind, CdpViewportSurface,
};
use crate::views::tdesktop_theme::tr_str;

const DEFAULT_BROWSER_SESSION_ID: &str = "browser_tab_1";
const MAX_VAULT_COOKIE_BUNDLE_BYTES: usize = 512 * 1024;

static FALLBACK_WORKSPACE_TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub(crate) fn resolve_browser_tokio_handle(
    preferred: Option<&tokio::runtime::Handle>,
) -> tokio::runtime::Handle {
    if let Some(h) = preferred {
        return h.clone();
    }
    if let Ok(h) = tokio::runtime::Handle::try_current() {
        return h;
    }
    FALLBACK_WORKSPACE_TOKIO_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("gpui-browser-workspace")
                .enable_all()
                .build()
                .expect("build fallback Tokio runtime for BrowserWorkspace")
        })
        .handle()
        .clone()
}

pub(crate) async fn run_on_tokio<F, T>(handle: &tokio::runtime::Handle, fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    handle.spawn(async move {
        let out = fut.await;
        let _ = tx.send(out);
    });
    rx.await
        .map_err(|_| "browser Tokio task dropped before completion".to_string())
}

type NavBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Fair `tokio::sync::Mutex<()>` serializer for `navigate`/`go_back`/`go_forward`/`reload`
/// across `CdpViewportSurface` and `BrowserSessionRegistry`. UI single-flight coalescing is
/// handled directly by `BrowserWorkspace` (`pending_cdp_nav`) so AI/script calls through
/// `BrowserSessionRegistry` never get skipped on URL collisions.
pub(crate) struct SingleFlightNavBackend {
    inner: Arc<dyn BrowserSessionBackend>,
    nav_mutex: tokio::sync::Mutex<()>,
}

impl SingleFlightNavBackend {
    pub(crate) fn new(inner: Arc<dyn BrowserSessionBackend>) -> Self {
        Self {
            inner,
            nav_mutex: tokio::sync::Mutex::new(()),
        }
    }
}

impl BrowserSessionBackend for SingleFlightNavBackend {
    fn backend_kind(&self) -> BrowserBackendKind {
        self.inner.backend_kind()
    }

    fn navigate<'life0, 'life1, 'async_trait>(
        &'life0 self,
        url: &'life1 str,
    ) -> NavBoxFuture<'async_trait, Result<NavigationResult, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let _guard = self.nav_mutex.lock().await;
            self.inner.navigate(url).await
        })
    }

    fn go_back<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let _guard = self.nav_mutex.lock().await;
            self.inner.go_back().await
        })
    }

    fn go_forward<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let _guard = self.nav_mutex.lock().await;
            self.inner.go_forward().await
        })
    }

    fn reload<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let _guard = self.nav_mutex.lock().await;
            self.inner.reload().await
        })
    }

    fn current_url<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.current_url()
    }

    fn page_title<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.page_title()
    }

    fn snapshot_dom<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<BrowserSnapshot, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.snapshot_dom()
    }

    fn eval_js<'life0, 'life1, 'async_trait>(
        &'life0 self,
        script: &'life1 str,
    ) -> NavBoxFuture<'async_trait, Result<serde_json::Value, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.eval_js(script)
    }

    fn click_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
    ) -> NavBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.click_locator(locator)
    }

    fn fill_locator<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
        value: &'life2 str,
        is_secret: bool,
    ) -> NavBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.fill_locator(locator, value, is_secret)
    }

    fn press_key<'life0, 'life1, 'async_trait>(
        &'life0 self,
        key: &'life1 str,
    ) -> NavBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.press_key(key)
    }

    fn scroll<'life0, 'async_trait>(
        &'life0 self,
        delta_x: f64,
        delta_y: f64,
    ) -> NavBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.scroll(delta_x, delta_y)
    }

    fn wait_for_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
        timeout_ms: u64,
    ) -> NavBoxFuture<'async_trait, Result<ActionOutcome, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.wait_for_locator(locator, timeout_ms)
    }

    fn extract_text_locator<'life0, 'life1, 'async_trait>(
        &'life0 self,
        locator: &'life1 SemanticLocator,
    ) -> NavBoxFuture<'async_trait, Result<String, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.extract_text_locator(locator)
    }

    fn capture_screenshot<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<Vec<u8>, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.capture_screenshot()
    }

    fn capture_frame_rgba<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<OffscreenFrameBuffer, BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.capture_frame_rgba()
    }

    fn export_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        domain_filter: Option<&'life1 str>,
    ) -> NavBoxFuture<'async_trait, Result<Vec<BrowserCookie>, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.export_cookies(domain_filter)
    }

    fn import_cookies<'life0, 'life1, 'async_trait>(
        &'life0 self,
        cookies: &'life1 [BrowserCookie],
    ) -> NavBoxFuture<'async_trait, Result<usize, BotBrowserError>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.import_cookies(cookies)
    }

    fn dispatch_pointer_event<'life0, 'async_trait>(
        &'life0 self,
        event: PointerInputEvent,
    ) -> NavBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.dispatch_pointer_event(event)
    }

    fn dispatch_key_event<'life0, 'async_trait>(
        &'life0 self,
        event: KeyInputEvent,
    ) -> NavBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.dispatch_key_event(event)
    }

    fn performance_metrics(&self) -> BrowserPerformanceMetrics {
        self.inner.performance_metrics()
    }

    fn cdp_bridge(&self) -> Option<&Arc<bot_browser::cdp::CdpBridge>> {
        self.inner.cdp_bridge()
    }

    fn close<'life0, 'async_trait>(
        &'life0 self,
    ) -> NavBoxFuture<'async_trait, Result<(), BotBrowserError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.close()
    }
}

/// Compares two `SemanticLocator`s by stable DOM identity (`test_id`, `aria_label`, `role`,
/// `css_selector`, `name_attr`), ignoring mutable `text_anchor` so consecutive keystrokes
/// inside `contenteditable` editors coalesce cleanly into a single `Fill` step.
pub(crate) fn same_editable_element_locator(a: &SemanticLocator, b: &SemanticLocator) -> bool {
    let has_stable_anchor = |loc: &SemanticLocator| {
        loc.test_id.is_some()
            || loc.aria_label.is_some()
            || loc.css_selector.is_some()
            || loc.name_attr.is_some()
    };
    if has_stable_anchor(a) || has_stable_anchor(b) {
        a.test_id == b.test_id
            && a.aria_label == b.aria_label
            && a.role == b.role
            && a.css_selector == b.css_selector
            && a.name_attr == b.name_attr
    } else {
        a == b
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingCdpNav {
    Navigate(String),
    GoBack,
    GoForward,
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserInspectorTab {
    Copilot,
    Recorder,
    Session,
    Store,
    Design,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveBrowserInput {
    None,
    AddressBar,
    ProfileName,
    CookieImport,
    CopilotPrompt,
    DistillName,
    DistillVersion,
    DistillCategory,
    DistillDescription,
    ScriptParam(String),
    StoreSearch,
    StoreChangelog,
    DesignNote,
    DesignSelectorQuery,
}

impl ActiveBrowserInput {
    pub fn owner_inspector_tab(&self) -> Option<BrowserInspectorTab> {
        match self {
            Self::None | Self::AddressBar => None,
            Self::CopilotPrompt => Some(BrowserInspectorTab::Copilot),
            Self::DistillName
            | Self::DistillVersion
            | Self::DistillCategory
            | Self::DistillDescription
            | Self::ScriptParam(_) => Some(BrowserInspectorTab::Recorder),
            Self::ProfileName | Self::CookieImport => Some(BrowserInspectorTab::Session),
            Self::StoreSearch | Self::StoreChangelog => Some(BrowserInspectorTab::Store),
            Self::DesignNote | Self::DesignSelectorQuery => Some(BrowserInspectorTab::Design),
        }
    }
}

pub struct BrowserWorkspaceConfig {
    pub data_dir: PathBuf,
    pub client: Option<Arc<TelegramClient>>,
    pub session_registry: Arc<BrowserSessionRegistry>,
    pub orchestrator: Option<Arc<BotBrowserOrchestrator>>,
    pub local_fabushi: Option<Arc<FabushiBotHost>>,
    pub unlocked_vault_key: Option<Arc<UnlockedUserVaultKey>>,
    pub tokio_handle: Option<tokio::runtime::Handle>,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

pub struct BrowserWorkspace {
    pub active_engine: BrowserEngineKind,
    pub active_session_id: String,
    pub active_profile_id: String,
    pub data_dir: PathBuf,
    pub client: Option<Arc<TelegramClient>>,
    pub session_registry: Arc<BrowserSessionRegistry>,
    pub orchestrator: Option<Arc<BotBrowserOrchestrator>>,
    pub local_fabushi: Option<Arc<FabushiBotHost>>,
    shared_local_fabushi: Option<Arc<SyncRwLock<Option<Arc<FabushiBotHost>>>>>,
    pub unlocked_vault_key: Option<Arc<UnlockedUserVaultKey>>,
    pub tokio_handle: Option<tokio::runtime::Handle>,
    #[cfg(target_os = "macos")]
    wkwebview: Option<EmbeddedWkWebView>,
    #[cfg(target_os = "macos")]
    wkwebview_listener_started: bool,
    #[cfg(target_os = "macos")]
    wkwebview_pending_url: Option<String>,
    #[cfg(target_os = "macos")]
    pending_profile_cookie_restore: bool,
    #[cfg(target_os = "macos")]
    pending_dom_fill: Option<RecordedDomEvent>,
    #[cfg(target_os = "macos")]
    last_recorded_nav_url: Option<String>,
    cdp_backend: Option<Arc<dyn BrowserSessionBackend>>,
    cdp_nav_in_flight: bool,
    pending_cdp_nav: Option<PendingCdpNav>,
    pending_cdp_refresh: bool,
    cdp_surface: Option<Entity<CdpViewportSurface>>,
    _cdp_surface_subscription: Option<gpui::Subscription>,
    _cdp_focus_subscription: Option<(EntityId, WindowId, gpui::Subscription)>,
    active_backend: Option<Arc<dyn BrowserSessionBackend>>,
    automation_session: Option<Arc<BrowserAutomationSession>>,
    event_listener_generation: u64,
    pub url_input: String,
    pub current_url: String,
    pub page_title: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub is_loading: bool,
    pub control_mode: BrowserControlMode,
    pub last_error: Option<String>,
    pub status_banner: Option<String>,
    pub inspector_open: bool,
    pub inspector_tab: BrowserInspectorTab,
    pub recorder: RecorderPanelState,
    pub store: StorePanelState,
    pub design_mode: DesignModePanelState,
    on_design_handoff: Option<
        Arc<dyn Fn(DesignHandoffTarget, bot_browser::DesignModeChatHandoff) + Send + Sync>,
    >,
    external_occluded: bool,
    workspace_visible: bool,
    pub close_requested: bool,
    pub focus_handle: FocusHandle,
    pub active_input: ActiveBrowserInput,
    active_input_generation: u64,
    last_set_active_input_had_window: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    nav_request_generation: u64,
}

impl BrowserWorkspace {
    pub fn is_headless_or_offline_env() -> bool {
        #[cfg(target_os = "macos")]
        {
            EmbeddedWkWebView::is_headless_or_offline_test_env()
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::env::var("GPUI_OFFLINE_TEST").as_deref() == Ok("1")
                || std::env::var("GPUI_MINIAPP_HEADLESS_CHILD").is_ok()
        }
    }

    pub fn new(config: BrowserWorkspaceConfig, cx: &mut Context<Self>) -> Self {
        let initial_engine = if cfg!(target_os = "macos") && !Self::is_headless_or_offline_env() {
            BrowserEngineKind::WkWebView
        } else {
            BrowserEngineKind::RustCdp
        };

        let mut store = StorePanelState::default();
        if let Some(ref client) = config.client {
            store.installed_apps = client.list_installed_automation_apps_cached();
        }

        let tokio_handle = config
            .tokio_handle
            .or_else(|| config.client.as_ref().map(|c| c.runtime().handle().clone()))
            .or_else(|| tokio::runtime::Handle::try_current().ok());

        cx.on_release(|this: &mut Self, cx: &mut gpui::App| {
            if let Some(prev) = this.design_mode.preview_image.take() {
                cx.drop_image(prev, None);
            }
            #[cfg(target_os = "macos")]
            if let Some(mut wv) = this.wkwebview.take() {
                wv.destroy();
            }
            this.session_registry.unregister_backend(&this.active_session_id);
        })
        .detach();

        Self {
            active_engine: initial_engine,
            active_session_id: DEFAULT_BROWSER_SESSION_ID.to_string(),
            active_profile_id: "default".to_string(),
            data_dir: config.data_dir,
            client: config.client,
            session_registry: config.session_registry,
            orchestrator: config.orchestrator,
            local_fabushi: config.local_fabushi,
            shared_local_fabushi: None,
            unlocked_vault_key: config.unlocked_vault_key,
            tokio_handle,
            #[cfg(target_os = "macos")]
            wkwebview: None,
            #[cfg(target_os = "macos")]
            wkwebview_listener_started: false,
            #[cfg(target_os = "macos")]
            wkwebview_pending_url: None,
            #[cfg(target_os = "macos")]
            pending_profile_cookie_restore: false,
            #[cfg(target_os = "macos")]
            pending_dom_fill: None,
            #[cfg(target_os = "macos")]
            last_recorded_nav_url: None,
            cdp_backend: None,
            cdp_nav_in_flight: false,
            pending_cdp_nav: None,
            pending_cdp_refresh: false,
            cdp_surface: None,
            _cdp_surface_subscription: None,
            _cdp_focus_subscription: None,
            active_backend: None,
            automation_session: None,
            event_listener_generation: 0,
            url_input: "https://x.com".to_string(),
            current_url: String::new(),
            page_title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            is_loading: false,
            control_mode: BrowserControlMode::Idle,
            last_error: None,
            status_banner: None,
            inspector_open: true,
            inspector_tab: BrowserInspectorTab::Copilot,
            recorder: RecorderPanelState::default(),
            store,
            design_mode: DesignModePanelState::default(),
            on_design_handoff: None,
            external_occluded: false,
            workspace_visible: true,
            close_requested: false,
            focus_handle: cx.focus_handle(),
            active_input: ActiveBrowserInput::None,
            active_input_generation: 0,
            last_set_active_input_had_window: false,
            palette: config.palette,
            language_id: config.language_id,
            nav_request_generation: 0,
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang_id: impl Into<String>) {
        self.language_id = lang_id.into();
    }

    pub fn sync_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        lang_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let lang = lang_id.into();
        self.palette = palette;
        self.language_id = lang.clone();
        if let Some(ref surface) = self.cdp_surface {
            surface.update(cx, |s, scx| s.set_palette(palette, lang, scx));
        }
        cx.notify();
    }

    pub fn set_orchestrator(&mut self, orchestrator: Arc<BotBrowserOrchestrator>) {
        self.session_registry
            .set_orchestrator(Arc::clone(&orchestrator));
        self.orchestrator = Some(orchestrator);
    }

    pub fn set_local_fabushi(&mut self, host: Arc<FabushiBotHost>) {
        self.local_fabushi = Some(Arc::clone(&host));
        if let Some(ref shared) = self.shared_local_fabushi {
            *shared.write() = Some(host);
        }
    }

    pub(crate) fn set_shared_local_fabushi(
        &mut self,
        shared: Arc<SyncRwLock<Option<Arc<FabushiBotHost>>>>,
    ) {
        if let Some(host) = shared.read().clone() {
            self.local_fabushi = Some(host);
        } else if let Some(ref host) = self.local_fabushi {
            *shared.write() = Some(Arc::clone(host));
        }
        self.shared_local_fabushi = Some(shared);
    }

    pub fn effective_local_fabushi(&self) -> Option<Arc<FabushiBotHost>> {
        if let Some(ref shared) = self.shared_local_fabushi {
            if let Some(host) = shared.read().clone() {
                return Some(host);
            }
        }
        self.local_fabushi.clone()
    }

    pub fn set_unlocked_vault_key(&mut self, key: Option<Arc<UnlockedUserVaultKey>>) {
        self.unlocked_vault_key = key;
    }

    pub fn update_client(&mut self, client: Arc<TelegramClient>, cx: &mut Context<Self>) {
        self.tokio_handle = Some(client.runtime().handle().clone());
        self.store.installed_apps = client.list_installed_automation_apps_cached();
        self.client = Some(client);
        cx.notify();
    }

    pub fn cdp_surface(&self) -> Option<&Entity<CdpViewportSurface>> {
        self.cdp_surface.as_ref()
    }

    pub fn active_backend(&self) -> Option<&Arc<dyn BrowserSessionBackend>> {
        self.active_backend.as_ref()
    }

    pub fn automation_session(&self) -> Option<&Arc<BrowserAutomationSession>> {
        self.automation_session.as_ref()
    }

    pub fn control_mode(&self) -> BrowserControlMode {
        self.automation_session
            .as_ref()
            .map(|s| s.control_mode())
            .unwrap_or_else(|| self.control_mode.clone())
    }

    pub fn is_externally_occluded(&self) -> bool {
        self.external_occluded
    }

    pub fn is_workspace_visible(&self) -> bool {
        self.workspace_visible
    }

    pub fn is_workspace_occluded(&self) -> bool {
        self.external_occluded || !self.workspace_visible
    }

    pub fn is_occluded(&self) -> bool {
        self.external_occluded
            || !self.workspace_visible
            || self.active_engine != BrowserEngineKind::WkWebView
    }

    pub fn is_native_webview_visible(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.wkwebview.as_ref().is_some_and(|wv| !wv.is_hidden())
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn has_pending_webview_focus_claim(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            !self.is_workspace_occluded()
                && self.active_engine == BrowserEngineKind::WkWebView
                && self
                    .wkwebview
                    .as_ref()
                    .is_some_and(|wv| !wv.is_hidden() && wv.has_pending_became_first_responder())
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub fn consume_webview_focus_claim(&mut self, cx: &mut Context<Self>) -> bool {
        #[cfg(target_os = "macos")]
        {
            let claimed = self
                .wkwebview
                .as_ref()
                .is_some_and(|wv| wv.take_became_first_responder());
            if claimed
                && !self.is_workspace_occluded()
                && self.active_engine == BrowserEngineKind::WkWebView
                && self.active_input != ActiveBrowserInput::None
            {
                self.assign_active_input(ActiveBrowserInput::None);
                cx.notify();
                return true;
            }
            false
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cx;
            false
        }
    }

    pub fn set_native_webview_hidden(&mut self, hidden: bool) {
        #[cfg(target_os = "macos")]
        if let Some(ref mut wv) = self.wkwebview {
            wv.set_hidden(hidden);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = hidden;
    }

    fn sync_cdp_surface_interactivity(&self, cx: &mut Context<Self>) {
        let interactive = !self.is_workspace_occluded()
            && !matches!(self.control_mode(), BrowserControlMode::AiControlling { .. })
            && !(self.design_mode.enabled && self.design_mode.picker_active);
        if let Some(ref surface) = self.cdp_surface {
            surface.update(cx, |s, _| s.set_interactive_enabled(interactive));
        }
    }

    pub fn set_occluded(&mut self, occluded: bool, cx: &mut Context<Self>) {
        let was_workspace_occluded = self.is_workspace_occluded();
        self.external_occluded = occluded;
        if occluded {
            self.set_native_webview_hidden(true);
        }
        if was_workspace_occluded != self.is_workspace_occluded() {
            self.sync_cdp_surface_interactivity(cx);
        }
        cx.notify();
    }

    pub fn set_workspace_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let was_workspace_occluded = self.is_workspace_occluded();
        self.workspace_visible = visible;
        if !visible {
            self.set_native_webview_hidden(true);
        }
        if was_workspace_occluded != self.is_workspace_occluded() {
            self.sync_cdp_surface_interactivity(cx);
        }
        cx.notify();
    }

    pub fn has_focused_text_input(&self) -> bool {
        self.active_input != ActiveBrowserInput::None
    }

    pub fn is_cdp_viewport_focused(&self, window: &Window, cx: &App) -> bool {
        !self.is_workspace_occluded()
            && self.active_engine == BrowserEngineKind::RustCdp
            && self.active_input == ActiveBrowserInput::None
            && self
                .cdp_surface
                .as_ref()
                .is_some_and(|s| s.read(cx).focus_handle(cx).is_focused(window))
    }

    pub fn active_input_generation(&self) -> u64 {
        self.active_input_generation
    }

    pub fn last_set_active_input_had_window(&self) -> bool {
        self.last_set_active_input_had_window
    }

    pub(crate) fn clear_last_set_active_input_had_window(&mut self) {
        self.last_set_active_input_had_window = false;
    }

    fn assign_active_input(&mut self, target: ActiveBrowserInput) {
        if target == ActiveBrowserInput::None {
            self.last_set_active_input_had_window = false;
        }
        if self.active_input != target {
            self.active_input = target;
            self.active_input_generation = self.active_input_generation.wrapping_add(1);
        }
    }

    pub fn set_active_input(
        &mut self,
        target: ActiveBrowserInput,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let is_non_none = target != ActiveBrowserInput::None;
        self.assign_active_input(target);
        if is_non_none {
            #[cfg(target_os = "macos")]
            if let Some(ref wv) = self.wkwebview {
                wv.clear_became_first_responder();
                wv.blur_active_dom_element();
                wv.restore_parent_first_responder();
            }
        }
        if is_non_none && window.is_some() {
            self.last_set_active_input_had_window = true;
        }
        if let Some(win) = window {
            self.focus_handle.focus(win);
        }
        cx.notify();
    }

    pub fn set_inspector_tab(&mut self, tab: BrowserInspectorTab, cx: &mut Context<Self>) {
        self.inspector_tab = tab;
        if self
            .active_input
            .owner_inspector_tab()
            .is_some_and(|owner| owner != tab)
        {
            self.assign_active_input(ActiveBrowserInput::None);
        }
        cx.notify();
    }

    fn active_input_mut(&mut self) -> Option<(&mut String, bool)> {
        match &self.active_input {
            ActiveBrowserInput::None => None,
            ActiveBrowserInput::AddressBar => Some((&mut self.url_input, false)),
            ActiveBrowserInput::ProfileName => Some((&mut self.recorder.profile_draft, false)),
            ActiveBrowserInput::CookieImport => Some((&mut self.recorder.cookie_import_draft, true)),
            ActiveBrowserInput::CopilotPrompt => {
                Some((&mut self.recorder.copilot_prompt_draft, true))
            }
            ActiveBrowserInput::DistillName => Some((&mut self.recorder.distill_name_draft, false)),
            ActiveBrowserInput::DistillVersion => {
                Some((&mut self.recorder.distill_version_draft, false))
            }
            ActiveBrowserInput::DistillCategory => {
                Some((&mut self.recorder.distill_category_draft, false))
            }
            ActiveBrowserInput::DistillDescription => {
                Some((&mut self.recorder.distill_description_draft, false))
            }
            ActiveBrowserInput::ScriptParam(name) => {
                let entry = self.recorder.param_inputs.entry(name.clone()).or_default();
                Some((entry, false))
            }
            ActiveBrowserInput::StoreSearch => Some((&mut self.store.search_query_draft, false)),
            ActiveBrowserInput::StoreChangelog => Some((&mut self.store.changelog_draft, false)),
            ActiveBrowserInput::DesignNote => Some((&mut self.design_mode.note_draft, true)),
            ActiveBrowserInput::DesignSelectorQuery => {
                Some((&mut self.design_mode.selector_query_draft, false))
            }
        }
    }

    pub fn active_input_text(&self) -> String {
        match &self.active_input {
            ActiveBrowserInput::None => String::new(),
            ActiveBrowserInput::AddressBar => self.url_input.clone(),
            ActiveBrowserInput::ProfileName => self.recorder.profile_draft.clone(),
            ActiveBrowserInput::CookieImport => self.recorder.cookie_import_draft.clone(),
            ActiveBrowserInput::CopilotPrompt => self.recorder.copilot_prompt_draft.clone(),
            ActiveBrowserInput::DistillName => self.recorder.distill_name_draft.clone(),
            ActiveBrowserInput::DistillVersion => self.recorder.distill_version_draft.clone(),
            ActiveBrowserInput::DistillCategory => self.recorder.distill_category_draft.clone(),
            ActiveBrowserInput::DistillDescription => {
                self.recorder.distill_description_draft.clone()
            }
            ActiveBrowserInput::ScriptParam(name) => self
                .recorder
                .param_inputs
                .get(name)
                .cloned()
                .unwrap_or_default(),
            ActiveBrowserInput::StoreSearch => self.store.search_query_draft.clone(),
            ActiveBrowserInput::StoreChangelog => self.store.changelog_draft.clone(),
            ActiveBrowserInput::DesignNote => self.design_mode.note_draft.clone(),
            ActiveBrowserInput::DesignSelectorQuery => {
                self.design_mode.selector_query_draft.clone()
            }
        }
    }

    pub fn clear_active_input(&mut self, cx: &mut Context<Self>) {
        if let Some((buf, _)) = self.active_input_mut() {
            buf.clear();
            cx.notify();
        }
    }

    pub fn insert_text_to_active_input(
        &mut self,
        raw_text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((buf, allow_newlines)) = self.active_input_mut() else {
            return false;
        };
        let clean = sanitize_paste_text(raw_text, allow_newlines);
        if clean.is_empty() {
            return false;
        }
        buf.push_str(&clean);
        cx.notify();
        true
    }

    pub fn handle_non_char_key_input(
        &mut self,
        key: &str,
        is_alt: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if key == "escape" {
            if self.active_input != ActiveBrowserInput::None {
                self.assign_active_input(ActiveBrowserInput::None);
                cx.notify();
                return true;
            }
            if self.design_mode.enabled && self.design_mode.picker_active {
                self.set_design_picker_active(false, cx);
                return true;
            }
        }
        let target = self.active_input.clone();
        let Some((buf, is_multiline)) = self.active_input_mut() else {
            return false;
        };
        match key {
            "backspace" => {
                if is_alt {
                    crate::components::ui::delete_prev_word(buf);
                } else {
                    pop_last_char(buf);
                }
                cx.notify();
                true
            }
            "space" => {
                buf.push(' ');
                cx.notify();
                true
            }
            "enter" => {
                if is_multiline {
                    match target {
                        ActiveBrowserInput::CopilotPrompt => {
                            self.trigger_copilot_prompt(cx);
                        }
                        _ => {
                            if let Some((b, _)) = self.active_input_mut() {
                                b.push('\n');
                            }
                            cx.notify();
                        }
                    }
                    return true;
                }
                match target {
                    ActiveBrowserInput::AddressBar => {
                        let url = self.url_input.clone();
                        self.navigate(&url, cx);
                    }
                    ActiveBrowserInput::ProfileName => {
                        let name = self.recorder.profile_draft.trim().to_string();
                        if !name.is_empty() {
                            self.switch_profile(name, cx);
                        }
                    }
                    ActiveBrowserInput::StoreSearch => {
                        self.search_store_apps(cx);
                    }
                    ActiveBrowserInput::DesignSelectorQuery => {
                        let selector = self.design_mode.selector_query_draft.clone();
                        self.capture_design_element_by_selector(selector, cx);
                    }
                    _ => {
                        self.assign_active_input(ActiveBrowserInput::None);
                        cx.notify();
                    }
                }
                true
            }
            "tab" => {
                match target {
                    ActiveBrowserInput::DistillName => {
                        self.assign_active_input(ActiveBrowserInput::DistillVersion);
                    }
                    ActiveBrowserInput::DistillVersion => {
                        self.assign_active_input(ActiveBrowserInput::DistillCategory);
                    }
                    ActiveBrowserInput::DistillCategory => {
                        self.assign_active_input(ActiveBrowserInput::DistillDescription);
                    }
                    ActiveBrowserInput::DistillDescription => {
                        self.assign_active_input(ActiveBrowserInput::DistillName);
                    }
                    _ => {}
                }
                cx.notify();
                true
            }
            _ => false,
        }
    }

    fn register_current_backend_with_registry(
        &mut self,
        backend: Arc<dyn BrowserSessionBackend>,
        cx: &mut Context<Self>,
    ) {
        self.active_backend = Some(Arc::clone(&backend));
        let session = self
            .session_registry
            .register_active_backend(&self.active_session_id, backend);
        let should_attach_listener = self
            .automation_session
            .as_ref()
            .is_none_or(|existing| !Arc::ptr_eq(existing, &session));
        if should_attach_listener {
            self.control_mode = session.control_mode();
            self.automation_session = Some(Arc::clone(&session));
            self.start_automation_event_listener(session, cx);
        }
    }

    fn start_automation_event_listener(
        &mut self,
        session: Arc<BrowserAutomationSession>,
        cx: &mut Context<Self>,
    ) {
        self.event_listener_generation = self.event_listener_generation.wrapping_add(1);
        let generation = self.event_listener_generation;
        let mut rx = session.subscribe();
        let (mpsc_tx, mut mpsc_rx) = mpsc::unbounded::<BrowserAutomationEvent>();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        handle.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        if mpsc_tx.unbounded_send(ev).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            while let Some(ev) = mpsc_rx.next().await {
                let alive = weak
                    .update(app, |this, cx| {
                        if this.event_listener_generation != generation {
                            return false;
                        }
                        this.apply_automation_event(ev, cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_automation_event(
        &mut self,
        ev: BrowserAutomationEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            BrowserAutomationEvent::StepStarted {
                step_index,
                operation,
                locator,
                url,
                ..
            } => {
                let detail = locator
                    .as_ref()
                    .map(format_locator_summary)
                    .or(url)
                    .unwrap_or_default();
                self.recorder.push_log(
                    BrowserLogLevel::Info,
                    format!("Step #{} started: {operation}", step_index + 1),
                    detail,
                    None,
                );
            }
            BrowserAutomationEvent::StepCompleted {
                step_index,
                operation,
                duration_ms,
                locator,
                url,
                ..
            } => {
                let detail = locator
                    .as_ref()
                    .map(format_locator_summary)
                    .or(url.clone())
                    .unwrap_or_default();
                self.recorder.push_log(
                    BrowserLogLevel::Success,
                    format!("Step #{} done: {operation}", step_index + 1),
                    detail,
                    Some(duration_ms),
                );
                if let Some(u) = url {
                    if !self.cdp_nav_in_flight && self.pending_cdp_nav.is_none() {
                        self.current_url = u.clone();
                        if self.active_input != ActiveBrowserInput::AddressBar {
                            self.url_input = u;
                        }
                    }
                }
                if self.active_engine == BrowserEngineKind::RustCdp {
                    if let Some(ref surface) = self.cdp_surface {
                        if self.cdp_nav_in_flight {
                            self.pending_cdp_refresh = true;
                        }
                        surface.update(cx, |s, scx| s.refresh_frame(scx));
                    }
                }
            }
            BrowserAutomationEvent::StepFailed {
                step_index,
                operation,
                duration_ms,
                error,
                ..
            } => {
                self.recorder.push_log(
                    BrowserLogLevel::Error,
                    format!("Step #{} failed: {operation}", step_index + 1),
                    error,
                    Some(duration_ms),
                );
            }
            BrowserAutomationEvent::TakeoverRequested {
                reason,
                paused_at_step,
                ..
            } => {
                self.control_mode = BrowserControlMode::WaitingForTakeover {
                    reason: reason.clone(),
                };
                let interactive = !self.is_workspace_occluded();
                if let Some(ref surface) = self.cdp_surface {
                    if self.active_engine == BrowserEngineKind::RustCdp && self.cdp_nav_in_flight {
                        self.pending_cdp_refresh = true;
                    }
                    surface.update(cx, |s, scx| {
                        s.set_interactive_enabled(interactive);
                        s.refresh_frame(scx);
                    });
                }
                self.recorder.push_log(
                    BrowserLogLevel::Warning,
                    format!("Takeover requested at step #{}", paused_at_step + 1),
                    reason,
                    None,
                );
            }
            BrowserAutomationEvent::ControlModeChanged { mode, .. } => {
                self.control_mode = mode.clone();
                let interactive =
                    !matches!(mode, BrowserControlMode::AiControlling { .. }) && !self.is_workspace_occluded();
                if let Some(ref surface) = self.cdp_surface {
                    if !matches!(mode, BrowserControlMode::AiControlling { .. })
                        && self.cdp_nav_in_flight
                    {
                        self.pending_cdp_refresh = true;
                    }
                    surface.update(cx, |s, scx| {
                        s.set_interactive_enabled(interactive);
                        if !matches!(mode, BrowserControlMode::AiControlling { .. }) {
                            s.refresh_frame(scx);
                        }
                    });
                }
                self.sync_url_title_from_active_backend(cx);
            }
            BrowserAutomationEvent::TraceCompleted { trace, .. } => {
                self.recorder.push_log(
                    BrowserLogLevel::Info,
                    "Trace completed",
                    format!("{} actions recorded", trace.actions.len()),
                    None,
                );
            }
            BrowserAutomationEvent::ScriptDistilled { app, .. } => {
                self.recorder.push_log(
                    BrowserLogLevel::Success,
                    format!("Distilled: {} v{}", app.name, app.version),
                    format!("{} steps", app.steps.len()),
                    None,
                );
                self.store.selected_draft_app = Some(app.clone());
                self.recorder.upsert_script(app);
            }
        }
    }

    fn sync_from_cdp_surface(
        &mut self,
        surface: &Entity<CdpViewportSurface>,
        cx: &mut Context<Self>,
    ) {
        if self.active_engine != BrowserEngineKind::RustCdp {
            return;
        }
        let (surf_url, surf_title, surf_refreshing, surf_navigating, surf_err) = {
            let sr = surface.read(cx);
            (
                sr.current_url().to_string(),
                sr.page_title().to_string(),
                sr.is_refreshing(),
                sr.is_navigating(),
                sr.last_error().map(str::to_string),
            )
        };

        if !surf_refreshing && !surf_navigating {
            if !surf_url.is_empty() {
                self.current_url = surf_url.clone();
                if self.active_input != ActiveBrowserInput::AddressBar
                    && self.pending_cdp_nav.is_none()
                {
                    self.url_input = surf_url;
                }
            }
            if !surf_title.is_empty() {
                self.page_title = surf_title;
            }
        }
        if !surf_refreshing && !surf_navigating {
            if let Some(next) = self.pending_cdp_nav.take() {
                self.cdp_nav_in_flight = true;
                self.pending_cdp_refresh = false;
                self.is_loading = true;
                self.last_error = None;
                match next {
                    PendingCdpNav::Navigate(url) => {
                        if self.active_input != ActiveBrowserInput::AddressBar {
                            self.url_input = url.clone();
                        }
                        surface.update(cx, |s, scx| s.navigate_and_refresh(url, scx));
                    }
                    PendingCdpNav::GoBack => {
                        surface.update(cx, |s, scx| s.go_back_and_refresh(scx));
                    }
                    PendingCdpNav::GoForward => {
                        surface.update(cx, |s, scx| s.go_forward_and_refresh(scx));
                    }
                    PendingCdpNav::Reload => {
                        surface.update(cx, |s, scx| s.reload_and_refresh(scx));
                    }
                }
                cx.notify();
                return;
            }
            if self.pending_cdp_refresh {
                self.pending_cdp_refresh = false;
                self.cdp_nav_in_flight = false;
                self.is_loading = true;
                self.last_error = surf_err;
                surface.update(cx, |s, scx| s.refresh_frame(scx));
                cx.notify();
                return;
            }
            self.cdp_nav_in_flight = false;
            self.is_loading = false;
            self.last_error = surf_err;
        } else {
            self.is_loading = true;
            self.last_error = surf_err;
        }
        cx.notify();
    }

    /// Injects a custom CDP/Mock `BrowserSessionBackend` (used for CDP mode and headless GPUI tests)
    /// and registers it as the sole active backend for `active_session_id`.
    pub fn set_cdp_backend(
        &mut self,
        backend: Arc<dyn BrowserSessionBackend>,
        cx: &mut Context<Self>,
    ) {
        self.set_native_webview_hidden(true);
        self.active_engine = BrowserEngineKind::RustCdp;
        self.cdp_nav_in_flight = false;
        self.pending_cdp_nav = None;
        self.pending_cdp_refresh = false;
        let sf = Arc::new(SingleFlightNavBackend::new(backend));
        let wrapped: Arc<dyn BrowserSessionBackend> = sf as Arc<dyn BrowserSessionBackend>;
        self.cdp_backend = Some(Arc::clone(&wrapped));
        let handle = self.tokio_handle.clone();
        let profile = Some(self.active_profile_id.clone());
        let pal = self.palette;
        let lang = self.language_id.clone();
        let surface_backend = Arc::clone(&wrapped);
        let initial_interactive = !self.is_workspace_occluded()
            && !matches!(self.control_mode(), BrowserControlMode::AiControlling { .. });
        let surface = cx.new(|cx| {
            let mut s = CdpViewportSurface::with_options(
                surface_backend,
                profile,
                (1280, 800),
                handle,
                cx,
            );
            s.set_interactive_enabled(initial_interactive);
            s.set_palette(pal, lang, cx);
            s.refresh_frame(cx);
            s
        });
        let sub = cx.observe(&surface, |this, surf, cx| {
            this.sync_from_cdp_surface(&surf, cx);
        });
        self._cdp_surface_subscription = Some(sub);
        self._cdp_focus_subscription = None;
        self.cdp_surface = Some(surface);
        self.register_current_backend_with_registry(wrapped, cx);
        cx.notify();
    }

    pub fn set_engine_kind(&mut self, engine: BrowserEngineKind, cx: &mut Context<Self>) {
        if engine == BrowserEngineKind::WkWebView
            && (cfg!(not(target_os = "macos")) || Self::is_headless_or_offline_env())
        {
            self.active_engine = BrowserEngineKind::RustCdp;
            self.status_banner = Some(
                tr_str(
                    &self.language_id,
                    "当前环境不支持原生 WKWebView，已保持 Rust CDP 模式。",
                    "Native WKWebView unavailable in headless/non-macOS env; staying on Rust CDP.",
                )
                .to_string(),
            );
            cx.notify();
            return;
        }
        if self.active_engine == engine {
            return;
        }
        self.active_engine = engine;
        match engine {
            BrowserEngineKind::RustCdp => {
                self.set_native_webview_hidden(true);
                if let Some(ref backend) = self.cdp_backend {
                    let b = Arc::clone(backend);
                    self.register_current_backend_with_registry(b, cx);
                } else {
                    self.ensure_cdp_backend_async(cx);
                }
            }
            BrowserEngineKind::WkWebView => {
                #[cfg(target_os = "macos")]
                {
                    let hidden = self.is_occluded();
                    let cur_url = self.current_url.clone();
                    if let Some(ref mut wv) = self.wkwebview {
                        let backend: Arc<dyn BrowserSessionBackend> = wv.backend();
                        wv.set_hidden(hidden);
                        if !cur_url.is_empty() {
                            wv.load_url_if_changed(&cur_url);
                        }
                        self.register_current_backend_with_registry(backend, cx);
                    } else if !cur_url.is_empty() {
                        self.wkwebview_pending_url = Some(cur_url);
                    }
                }
            }
        }
        cx.notify();
    }

    fn ensure_cdp_backend_async(&mut self, cx: &mut Context<Self>) {
        if let Some(ref backend) = self.cdp_backend {
            let b = Arc::clone(backend);
            self.register_current_backend_with_registry(b, cx);
            return;
        }
        let Some(orch) = self.orchestrator.clone() else {
            let msg = tr_str(
                &self.language_id,
                "BotBrowserOrchestrator 尚未初始化，无法创建 Rust CDP 会话。",
                "BotBrowserOrchestrator is not attached yet; cannot initialize Rust CDP session.",
            )
            .to_string();
            self.status_banner = Some(msg.clone());
            self.last_error = Some(msg);
            cx.notify();
            return;
        };
        let sid = self.active_session_id.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                orch.ensure_session_backend(&sid, None)
                    .await
                    .map(|b| b as Arc<dyn BrowserSessionBackend>)
                    .map_err(|e| e.to_string())
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(backend)) => {
                    this.set_cdp_backend(backend, cx);
                }
                Ok(Err(err)) | Err(err) => {
                    this.last_error = Some(err);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn navigate(&mut self, raw_url: &str, cx: &mut Context<Self>) {
        let trimmed = raw_url.trim();
        if trimmed.is_empty() {
            return;
        }
        let target = if trimmed.contains("://") {
            trimmed.to_string()
        } else {
            format!("https://{trimmed}")
        };
        self.url_input = target.clone();
        self.last_error = None;
        self.is_loading = true;
        self.nav_request_generation = self.nav_request_generation.wrapping_add(1);
        let nav_gen = self.nav_request_generation;

        #[cfg(target_os = "macos")]
        if self.active_engine == BrowserEngineKind::WkWebView && !Self::is_headless_or_offline_env()
        {
            if let Some(ref mut wv) = self.wkwebview {
                wv.load_url(&target);
                self.current_url = target;
                cx.notify();
                return;
            }
            self.wkwebview_pending_url = Some(target.clone());
            self.current_url = target;
            cx.notify();
            return;
        }

        if let Some(ref surface) = self.cdp_surface {
            if self.cdp_nav_in_flight {
                self.pending_cdp_nav = Some(PendingCdpNav::Navigate(target));
            } else {
                self.cdp_nav_in_flight = true;
                self.pending_cdp_nav = None;
                surface.update(cx, |s, scx| s.navigate_and_refresh(target.clone(), scx));
            }
            cx.notify();
            return;
        }

        if let Some(backend) = self.active_backend.clone().or_else(|| self.cdp_backend.clone()) {
            let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
            let target_clone = target.clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = run_on_tokio(&handle, async move {
                    backend.navigate(&target_clone).await.map_err(|e| e.to_string())
                })
                .await;
                let _ = weak.update(app, |this, cx| {
                    if this.nav_request_generation != nav_gen {
                        return;
                    }
                    this.is_loading = false;
                    match res {
                        Ok(Ok(nav)) => {
                            this.current_url = nav.url.clone();
                            if this.active_input != ActiveBrowserInput::AddressBar {
                                this.url_input = nav.url;
                            }
                            this.page_title = nav.title;
                            this.last_error = None;
                        }
                        Ok(Err(err)) | Err(err) => {
                            this.last_error = Some(err);
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        } else if let Some(orch) = self.orchestrator.clone() {
            let sid = self.active_session_id.clone();
            let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = run_on_tokio(&handle, async move {
                    let backend: Arc<dyn BrowserSessionBackend> = orch
                        .ensure_session_backend(&sid, None)
                        .await
                        .map_err(|e| e.to_string())?;
                    let nav = backend.navigate(&target).await.map_err(|e| e.to_string())?;
                    Ok::<_, String>((backend, nav))
                })
                .await;
                let _ = weak.update(app, |this, cx| {
                    if this.nav_request_generation != nav_gen {
                        return;
                    }
                    this.is_loading = false;
                    match res {
                        Ok(Ok((backend, nav))) => {
                            this.set_cdp_backend(backend, cx);
                            this.current_url = nav.url.clone();
                            if this.active_input != ActiveBrowserInput::AddressBar {
                                this.url_input = nav.url;
                            }
                            this.page_title = nav.title;
                            this.last_error = None;
                        }
                        Ok(Err(err)) | Err(err) => {
                            this.last_error = Some(err);
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            self.is_loading = false;
            self.last_error = Some("No active browser backend configured".to_string());
            cx.notify();
        }
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if self.active_engine == BrowserEngineKind::WkWebView && !Self::is_headless_or_offline_env()
        {
            if let Some(ref mut wv) = self.wkwebview {
                let _ = wv.go_back();
            }
            cx.notify();
            return;
        }
        if let Some(ref s) = self.cdp_surface {
            self.is_loading = true;
            self.last_error = None;
            if self.cdp_nav_in_flight {
                self.pending_cdp_nav = Some(PendingCdpNav::GoBack);
            } else {
                self.cdp_nav_in_flight = true;
                self.pending_cdp_nav = None;
                s.update(cx, |surface, scx| surface.go_back_and_refresh(scx));
            }
        }
        cx.notify();
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if self.active_engine == BrowserEngineKind::WkWebView && !Self::is_headless_or_offline_env()
        {
            if let Some(ref mut wv) = self.wkwebview {
                let _ = wv.go_forward();
            }
            cx.notify();
            return;
        }
        if let Some(ref s) = self.cdp_surface {
            self.is_loading = true;
            self.last_error = None;
            if self.cdp_nav_in_flight {
                self.pending_cdp_nav = Some(PendingCdpNav::GoForward);
            } else {
                self.cdp_nav_in_flight = true;
                self.pending_cdp_nav = None;
                s.update(cx, |surface, scx| surface.go_forward_and_refresh(scx));
            }
        }
        cx.notify();
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if self.active_engine == BrowserEngineKind::WkWebView && !Self::is_headless_or_offline_env()
        {
            if let Some(ref mut wv) = self.wkwebview {
                wv.reload();
            }
            cx.notify();
            return;
        }
        if let Some(ref s) = self.cdp_surface {
            self.is_loading = true;
            self.last_error = None;
            if self.cdp_nav_in_flight {
                self.pending_cdp_nav = Some(PendingCdpNav::Reload);
            } else {
                self.cdp_nav_in_flight = true;
                self.pending_cdp_nav = None;
                s.update(cx, |surface, scx| surface.reload_and_refresh(scx));
            }
        }
        cx.notify();
    }

    pub fn user_take_over(&mut self, cx: &mut Context<Self>) {
        if let Some(ref session) = self.automation_session {
            session.user_take_over();
            self.control_mode = session.control_mode();
        } else {
            self.control_mode = BrowserControlMode::UserTakeover { paused_at_step: 0 };
        }
        let interactive = !self.is_workspace_occluded();
        if let Some(ref surface) = self.cdp_surface {
            if self.active_engine == BrowserEngineKind::RustCdp && self.cdp_nav_in_flight {
                self.pending_cdp_refresh = true;
            }
            surface.update(cx, |s, scx| {
                s.set_interactive_enabled(interactive);
                s.refresh_frame(scx);
            });
        }
        self.recorder.push_log(
            BrowserLogLevel::Warning,
            "User took over browser control",
            "AI paused; manual interaction enabled",
            None,
        );
        cx.notify();
    }

    pub fn user_resume_ai(&mut self, cx: &mut Context<Self>) {
        if let Some(ref session) = self.automation_session {
            session.user_resume_ai();
            self.control_mode = session.control_mode();
        } else {
            self.control_mode = BrowserControlMode::Idle;
        }
        self.recorder.push_log(
            BrowserLogLevel::Info,
            "Control returned to AI",
            "Resuming from live browser state",
            None,
        );
        cx.notify();
    }

    pub fn user_cancel_ai(&mut self, cx: &mut Context<Self>) {
        if let Some(ref session) = self.automation_session {
            session.user_cancel_ai();
            self.control_mode = session.control_mode();
        } else {
            self.control_mode = BrowserControlMode::Idle;
        }
        let interactive = !self.is_workspace_occluded();
        if let Some(ref surface) = self.cdp_surface {
            surface.update(cx, |s, _| s.set_interactive_enabled(interactive));
        }
        self.recorder.push_log(
            BrowserLogLevel::Warning,
            "AI automation cancelled",
            String::new(),
            None,
        );
        cx.notify();
    }

    pub fn toggle_dom_recording(&mut self, cx: &mut Context<Self>) {
        if self.recorder.is_dom_recording {
            self.flush_pending_dom_fill();
        }
        self.recorder.is_dom_recording = !self.recorder.is_dom_recording;
        #[cfg(target_os = "macos")]
        if let Some(ref wv) = self.wkwebview {
            wv.set_dom_recording(self.recorder.is_dom_recording);
        }
        cx.notify();
    }

    pub fn trigger_copilot_prompt(&mut self, cx: &mut Context<Self>) {
        let prompt = self.recorder.copilot_prompt_draft.trim().to_string();
        if prompt.is_empty() || self.recorder.copilot_running {
            return;
        }
        let Some(host) = self.effective_local_fabushi() else {
            self.recorder.push_log(
                BrowserLogLevel::Warning,
                "Copilot unavailable",
                "Local Fabushi agent host is not attached yet.",
                None,
            );
            cx.notify();
            return;
        };
        self.recorder.copilot_running = true;
        self.recorder.copilot_last_reply = None;
        let sid = self.active_session_id.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let mut req = HermesTurnRequest::new(prompt);
                req.session_id = Some(sid);
                req.adaptive_toolsets = Some(vec![ToolsetId::Browser]);
                host.core_engine()
                    .run_turn(req)
                    .await
                    .map(|r| r.response_text)
                    .map_err(|e| e.to_string())
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                this.recorder.copilot_running = false;
                match res {
                    Ok(Ok(reply)) => {
                        this.recorder.copilot_last_reply = Some(reply);
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.push_log(
                            BrowserLogLevel::Error,
                            "Copilot run failed",
                            err.clone(),
                            None,
                        );
                        this.last_error = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    fn flush_pending_dom_fill(&mut self) {
        if let Some(ev) = self.pending_dom_fill.take() {
            if let Some(ref session) = self.automation_session {
                let val = ev.value.unwrap_or_default();
                session.record_fill(
                    Some(ev.page_url),
                    ev.locator,
                    &val,
                    ev.is_sensitive,
                    None,
                    None,
                    0,
                );
                self.recorder.recorded_dom_count = self.recorder.recorded_dom_count.saturating_add(1);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn flush_pending_dom_fill(&mut self) {}

    pub fn distill_from_form(&mut self, cx: &mut Context<Self>) -> Result<AutomationScriptApp, String> {
        let name = self.recorder.distill_name_draft.clone();
        let desc = self.recorder.distill_description_draft.clone();
        let cat = self.recorder.distill_category_draft.clone();
        self.distill_last_trace(&name, &desc, &cat, cx)
    }

    pub fn distill_last_trace(
        &mut self,
        name: &str,
        description: &str,
        category: &str,
        cx: &mut Context<Self>,
    ) -> Result<AutomationScriptApp, String> {
        self.flush_pending_dom_fill();
        let version = if self.recorder.distill_version_draft.trim().is_empty() {
            "1.0.0".to_string()
        } else {
            self.recorder.distill_version_draft.trim().to_string()
        };
        let meta = ScriptAppMetadata {
            name: if name.trim().is_empty() {
                "Recorded Browser Script".to_string()
            } else {
                name.trim().to_string()
            },
            version,
            description: description.trim().to_string(),
            category: if category.trim().is_empty() {
                "general".to_string()
            } else {
                category.trim().to_string()
            },
            ..Default::default()
        };
        let res = self
            .session_registry
            .distill_script(&self.active_session_id, meta)
            .map_err(|e| e.to_string());
        match res {
            Ok(app) => {
                self.recorder.upsert_script(app.clone());
                self.store.selected_draft_app = Some(app.clone());
                cx.notify();
                Ok(app)
            }
            Err(err) => {
                self.recorder.distill_error = Some(err.clone());
                cx.notify();
                Err(err)
            }
        }
    }

    pub fn collect_replay_params_for(&self, app: &AutomationScriptApp) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        for p in &app.parameters {
            if !p.is_secret {
                if let Some(v) = self.recorder.param_inputs.get(&p.name) {
                    if !v.is_empty() {
                        params.insert(p.name.clone(), v.clone());
                        continue;
                    }
                }
                if let Some(ref def) = p.default_value {
                    params.insert(p.name.clone(), def.clone());
                }
            }
        }
        params
    }

    fn resolve_vault_resolver(&self) -> Option<Arc<dyn VaultSecretResolver>> {
        if let Some(ref orch) = self.orchestrator {
            return Some(Arc::new(CredentialVaultSecretResolver::new(Arc::clone(
                orch.vault(),
            ))));
        }
        if let Some(ref key) = self.unlocked_vault_key {
            let db_path = self.data_dir.join("vault.db");
            if let Ok(vault) = CredentialVault::open(db_path, key.as_ref()) {
                return Some(Arc::new(CredentialVaultSecretResolver::new(Arc::new(
                    vault,
                ))));
            }
        }
        None
    }

    fn ensure_automation_session_listener(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.session_registry.get_session(&self.active_session_id) {
            let needs_attach = self
                .automation_session
                .as_ref()
                .is_none_or(|existing| !Arc::ptr_eq(existing, &session));
            if needs_attach {
                self.control_mode = session.control_mode();
                self.automation_session = Some(Arc::clone(&session));
                self.start_automation_event_listener(session, cx);
            }
        }
    }

    pub fn replay_script_app(
        &mut self,
        app: AutomationScriptApp,
        params: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        self.recorder.is_replaying = true;
        self.recorder.replay_error = None;
        self.ensure_automation_session_listener(cx);
        let registry = Arc::clone(&self.session_registry);
        let sid = self.active_session_id.clone();
        let resolver = self.resolve_vault_resolver();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());

        cx.spawn(async move |weak: WeakEntity<Self>, app_cx: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                registry
                    .replay_script(&sid, &app, &params, resolver.as_deref())
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;

            let _ = weak.update(app_cx, |this, cx| {
                this.recorder.is_replaying = false;
                this.ensure_automation_session_listener(cx);
                match res {
                    Ok(Ok(report)) => {
                        if let Some(ref u) = report.final_url {
                            if !this.cdp_nav_in_flight && this.pending_cdp_nav.is_none() {
                                this.current_url = u.clone();
                                if this.active_input != ActiveBrowserInput::AddressBar {
                                    this.url_input = u.clone();
                                }
                            }
                        }
                        if report.success {
                            this.recorder.replay_error = None;
                        } else {
                            this.recorder.replay_error = Some(
                                report
                                    .error
                                    .clone()
                                    .unwrap_or_else(|| "Script replay failed".to_string()),
                            );
                        }
                        this.recorder.push_log(
                            if report.success {
                                BrowserLogLevel::Success
                            } else {
                                BrowserLogLevel::Error
                            },
                            format!("Replay {} (used_llm={})", report.app_id, report.used_llm),
                            format!(
                                "{} steps · {} ms",
                                report.steps_executed, report.total_duration_ms
                            ),
                            Some(report.total_duration_ms),
                        );
                        this.recorder.last_replay_report = Some(report);
                        if this.active_engine == BrowserEngineKind::RustCdp {
                            if let Some(ref surface) = this.cdp_surface {
                                if this.cdp_nav_in_flight {
                                    this.pending_cdp_refresh = true;
                                }
                                surface.update(cx, |s, scx| s.refresh_frame(scx));
                            }
                        }
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.replay_error = Some(err.clone());
                        this.recorder.push_log(
                            BrowserLogLevel::Error,
                            "Script replay error",
                            err,
                            None,
                        );
                        if this.active_engine == BrowserEngineKind::RustCdp {
                            if let Some(ref surface) = this.cdp_surface {
                                if this.cdp_nav_in_flight {
                                    this.pending_cdp_refresh = true;
                                }
                                surface.update(cx, |s, scx| s.refresh_frame(scx));
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn switch_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        let trimmed = profile_id.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        self.active_profile_id = trimmed.clone();
        self.recorder.profile_draft = trimmed.clone();
        if !self.recorder.available_profiles.contains(&trimmed) {
            self.recorder.available_profiles.push(trimmed.clone());
        }
        #[cfg(target_os = "macos")]
        if self.active_engine == BrowserEngineKind::WkWebView && !Self::is_headless_or_offline_env()
        {
            if let Some(mut old_wv) = self.wkwebview.take() {
                old_wv.destroy();
            }
            self.wkwebview_listener_started = false;
            self.active_backend = None;
            if !self.current_url.is_empty() {
                self.wkwebview_pending_url = Some(self.current_url.clone());
            }
            self.pending_profile_cookie_restore = true;
            cx.notify();
            return;
        }
        self.restore_active_profile_cookies(cx);
        cx.notify();
    }

    pub fn save_active_profile_cookies(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.active_backend.clone() else {
            self.recorder.cookie_status_message = None;
            self.recorder.cookie_error_message = Some("No active browser backend".to_string());
            cx.notify();
            return;
        };
        let profile = self.active_profile_id.clone();
        let data_dir = self.data_dir.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let cookies = backend.export_cookies(None).await.map_err(|e| e.to_string())?;
                let count = cookies.len();
                let store = BrowserProfileStore::new(&data_dir);
                store
                    .save_cookies(&profile, None, &cookies)
                    .map_err(|e| e.to_string())?;
                Ok::<usize, String>(count)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(count)) => {
                        this.recorder.last_persisted_cookie_count = count;
                        this.recorder.cookie_error_message = None;
                        this.recorder.cookie_status_message = Some(format!(
                            "Saved {count} cookies to profile '{}'",
                            this.active_profile_id
                        ));
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.cookie_status_message = None;
                        this.recorder.cookie_error_message = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn restore_active_profile_cookies(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.active_backend.clone() else {
            return;
        };
        let profile = self.active_profile_id.clone();
        let data_dir = self.data_dir.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let store = BrowserProfileStore::new(&data_dir);
                let cookies = store
                    .load_active_cookies(&profile, None, Utc::now().timestamp())
                    .map_err(|e| e.to_string())?;
                if cookies.is_empty() {
                    return Ok::<usize, String>(0);
                }
                backend
                    .import_cookies(&cookies)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(count)) => {
                        this.recorder.last_imported_cookie_count = count;
                        this.recorder.cookie_error_message = None;
                        if count > 0 {
                            this.recorder.cookie_status_message = Some(format!(
                                "Restored {count} cookies for profile '{}'",
                                this.active_profile_id
                            ));
                        }
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.cookie_status_message = None;
                        this.recorder.cookie_error_message = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn backup_profile_to_vault(&mut self, cx: &mut Context<Self>) {
        let profile = self.active_profile_id.clone();
        let data_dir = self.data_dir.clone();
        let orch = self.orchestrator.clone();
        let unlocked_key = self.unlocked_vault_key.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                tokio::task::spawn_blocking(move || {
                    let store = BrowserProfileStore::new(&data_dir);
                    if let Some(orch) = orch {
                        store
                            .backup_to_credential_vault(
                                orch.vault(),
                                &profile,
                                None,
                                MAX_VAULT_COOKIE_BUNDLE_BYTES,
                            )
                            .map_err(|e| e.to_string())?;
                        return Ok::<String, String>(format!(
                            "Backed up profile '{profile}' into CredentialVault"
                        ));
                    }
                    if let Some(key) = unlocked_key {
                        let vault = CredentialVault::open(data_dir.join("vault.db"), key.as_ref())
                            .map_err(|e| e.to_string())?;
                        store
                            .backup_to_credential_vault(
                                &vault,
                                &profile,
                                None,
                                MAX_VAULT_COOKIE_BUNDLE_BYTES,
                            )
                            .map_err(|e| e.to_string())?;
                        return Ok(format!(
                            "Backed up profile '{profile}' into CredentialVault"
                        ));
                    }
                    Err("CredentialVault is locked or unavailable".to_string())
                })
                .await
                .map_err(|e| e.to_string())?
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(msg)) => {
                        this.recorder.cookie_error_message = None;
                        this.recorder.vault_sync_status = Some(msg);
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.cookie_status_message = None;
                        this.recorder.cookie_error_message = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn import_cookies_from_text_async(&mut self, raw_text: String, cx: &mut Context<Self>) {
        let Some(backend) = self.active_backend.clone() else {
            self.recorder.cookie_status_message = None;
            self.recorder.cookie_error_message = Some("No active browser backend".to_string());
            cx.notify();
            return;
        };
        let profile = self.active_profile_id.clone();
        let data_dir = self.data_dir.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let cookies = import_cookies_from_text(&raw_text).map_err(|e| e.to_string())?;
                let imported = backend
                    .import_cookies(&cookies)
                    .await
                    .map_err(|e| e.to_string())?;
                let store = BrowserProfileStore::new(&data_dir);
                let total = store
                    .merge_cookies(&profile, None, &cookies)
                    .map_err(|e| e.to_string())?;
                Ok::<(usize, usize), String>((imported, total))
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok((imported, total))) => {
                        this.recorder.last_imported_cookie_count = imported;
                        this.recorder.last_persisted_cookie_count = total;
                        this.recorder.cookie_error_message = None;
                        this.recorder.cookie_status_message = Some(format!(
                            "Imported {imported} cookies ({total} total in profile '{}')",
                            this.active_profile_id
                        ));
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.cookie_status_message = None;
                        this.recorder.cookie_error_message = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn apply_chrome_probe_result(&mut self, probe: ChromeImportProbe, cx: &mut Context<Self>) {
        self.recorder.chrome_probe = ChromeProbeUiState::from(probe);
        cx.notify();
    }

    pub fn probe_local_chrome(&mut self, cx: &mut Context<Self>) {
        self.recorder.chrome_probe = ChromeProbeUiState::Probing;
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                tokio::task::spawn_blocking(probe_local_chrome_cookies)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(probe)) => this.apply_chrome_probe_result(probe, cx),
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.cookie_error_message = Some(err);
                        this.recorder.chrome_probe = ChromeProbeUiState::NotFound;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn import_cookies_from_chrome_path(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(backend) = self.active_backend.clone() else {
            self.recorder.cookie_status_message = None;
            self.recorder.cookie_error_message = Some("No active browser backend".to_string());
            cx.notify();
            return;
        };
        let profile = self.active_profile_id.clone();
        let data_dir = self.data_dir.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let db_path = PathBuf::from(&path);
                let report = tokio::task::spawn_blocking(move || {
                    import_cookies_from_chrome_sqlite(
                        &db_path,
                        &SystemChromeKeyProvider,
                        &ChromeImportOptions::default(),
                    )
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                let imported = backend
                    .import_cookies(&report.cookies)
                    .await
                    .map_err(|e| e.to_string())?;
                let store = BrowserProfileStore::new(&data_dir);
                let total = store
                    .merge_cookies(&profile, None, &report.cookies)
                    .map_err(|e| e.to_string())?;
                Ok::<(usize, usize), String>((imported, total))
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok((imported, total))) => {
                        this.recorder.last_imported_cookie_count = imported;
                        this.recorder.last_persisted_cookie_count = total;
                        this.recorder.cookie_error_message = None;
                        this.recorder.cookie_status_message = Some(format!(
                            "Imported {imported} Chrome cookies ({total} total in profile)"
                        ));
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.cookie_status_message = None;
                        this.recorder.cookie_error_message = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn publish_script_app(&mut self, app: AutomationScriptApp, cx: &mut Context<Self>) {
        let changelog = Some(self.store.changelog_draft.clone());
        self.publish_script_app_with_changelog(app, changelog, cx);
    }

    pub fn publish_script_app_with_changelog(
        &mut self,
        app: AutomationScriptApp,
        changelog: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            self.store.publish_error = Some("TelegramClient is not attached".to_string());
            cx.notify();
            return;
        };
        let req = match StorePanelState::build_publish_request(&app, changelog) {
            Ok(r) => r,
            Err(err) => {
                self.store.publish_error = Some(err);
                self.store.publish_secret_rejected = false;
                self.store.publish_success = None;
                cx.notify();
                return;
            }
        };
        self.store.is_publishing = true;
        self.store.publish_error = None;
        self.store.publish_secret_rejected = false;
        self.store.publish_success = None;
        let http = client.http_client.clone();
        let tok_opt = client.token().filter(|t| !t.trim().is_empty());
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app_cx: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let tok = tok_opt.ok_or_else(|| {
                    HttpClientError::Unauthorized("No active session token".to_string())
                })?;
                http.publish_automation_app(&tok, &req).await
            })
            .await;
            if let Ok(Ok(ref detail)) = res {
                client
                    .local_store
                    .upsert_cached_automation_app(detail.clone());
            }
            let _ = weak.update(app_cx, |this, cx| {
                this.store.is_publishing = false;
                match res {
                    Ok(Ok(detail)) => {
                        this.store.publish_error = None;
                        this.store.publish_secret_rejected = false;
                        this.store.publish_success = Some(format!(
                            "Published {} v{} (id={})",
                            detail.name, detail.latest_version, detail.app_id
                        ));
                        this.search_store_apps(cx);
                    }
                    Ok(Err(http_err)) => {
                        let (is_secret, msg) = StorePanelState::format_publish_error(&http_err);
                        this.store.publish_secret_rejected = is_secret;
                        this.store.publish_error = Some(msg);
                    }
                    Err(task_err) => {
                        this.store.publish_error = Some(task_err);
                    }
                }
                cx.notify();
            });
            drop(client);
        })
        .detach();
        cx.notify();
    }

    pub fn search_store_apps(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.store.is_loading_catalog = true;
        self.store.store_error = None;
        let q = self.store.search_query_draft.trim().to_string();
        let cat = self.store.category_filter.clone();
        let http = client.http_client.clone();
        let tok = client.token();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let q_opt = (!q.is_empty()).then_some(q.as_str());
                let c_opt = cat.as_deref();
                http.search_automation_apps(tok.as_deref(), q_opt, c_opt, None, Some(50), Some(0))
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                this.store.is_loading_catalog = false;
                match res {
                    Ok(Ok(resp)) => {
                        this.store.catalog_apps = resp.apps;
                        this.store.store_error = None;
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.store.store_error = Some(err);
                    }
                }
                cx.notify();
            });
            drop(client);
        })
        .detach();
        cx.notify();
    }

    pub fn refresh_installed_store_apps(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let http = client.http_client.clone();
        let tok_opt = client.token().filter(|t| !t.trim().is_empty());
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let tok = tok_opt.ok_or_else(|| "No active session token".to_string())?;
                http.list_installed_automation_apps(&tok)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Ok(ref resp)) = res {
                client
                    .local_store
                    .replace_installed_automation_apps(resp.apps.clone());
            }
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.store.installed_apps = resp.apps;
                    }
                    Ok(Err(_)) | Err(_) => {
                        if let Some(ref c) = this.client {
                            this.store.installed_apps = c.list_installed_automation_apps_cached();
                        }
                    }
                }
                cx.notify();
            });
            drop(client);
        })
        .detach();
    }

    pub fn install_store_app(&mut self, app_id: &str, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            self.store.store_error = Some("TelegramClient is not attached".to_string());
            cx.notify();
            return;
        };
        let target_id = app_id.to_string();
        self.store.store_error = None;
        let http = client.http_client.clone();
        let tok_opt = client.token().filter(|t| !t.trim().is_empty());
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let tok = tok_opt.ok_or_else(|| "No active session token".to_string())?;
                http.install_automation_app(&tok, &target_id)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Ok(ref resp)) = res {
                if !resp.app.app_id.trim().is_empty() {
                    client
                        .local_store
                        .upsert_installed_automation_app(resp.app.clone());
                }
            }
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.store.store_error = None;
                        this.store.last_installed_app_id = Some(resp.app.app_id.clone());
                        if let Some(ref c) = this.client {
                            this.store.installed_apps = c.list_installed_automation_apps_cached();
                        }
                        if let Some(summary) = this
                            .store
                            .catalog_apps
                            .iter_mut()
                            .find(|s| s.app_id == resp.app.app_id)
                        {
                            summary.is_installed = true;
                            summary.install_count = resp.install_count;
                        }
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.store.store_error = Some(err);
                    }
                }
                cx.notify();
            });
            drop(client);
        })
        .detach();
    }

    pub fn uninstall_store_app(&mut self, app_id: &str, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let target_id = app_id.to_string();
        self.store.store_error = None;
        let http = client.http_client.clone();
        let tok_opt = client.token().filter(|t| !t.trim().is_empty());
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let target_for_req = target_id.clone();
            let res = run_on_tokio(&handle, async move {
                let tok = tok_opt.ok_or_else(|| "No active session token".to_string())?;
                http.uninstall_automation_app(&tok, &target_for_req)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Ok(ref resp)) = res {
                let key = if resp.app_id.trim().is_empty() {
                    target_id.trim()
                } else {
                    resp.app_id.trim()
                };
                client.local_store.remove_installed_automation_app(key);
            }
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.store.store_error = None;
                        this.store.installed_apps.retain(|a| a.app_id != resp.app_id);
                        if let Some(summary) = this
                            .store
                            .catalog_apps
                            .iter_mut()
                            .find(|s| s.app_id == resp.app_id)
                        {
                            summary.is_installed = false;
                        }
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.store.store_error = Some(err);
                    }
                }
                cx.notify();
            });
            drop(client);
        })
        .detach();
    }

    pub fn run_installed_store_app(
        &mut self,
        installed: InstalledAutomationApp,
        cx: &mut Context<Self>,
    ) {
        let parsed: AutomationScriptApp = match serde_json::from_value(installed.manifest.clone()) {
            Ok(a) => a,
            Err(e) => {
                self.store.store_error = Some(format!("Invalid installed app manifest: {e}"));
                cx.notify();
                return;
            }
        };
        let params = self.collect_replay_params_for(&parsed);
        self.recorder.is_replaying = true;
        self.recorder.replay_error = None;
        self.store.store_error = None;
        self.ensure_automation_session_listener(cx);
        let registry = Arc::clone(&self.session_registry);
        let sid = self.active_session_id.clone();
        let resolver = self.resolve_vault_resolver();
        let client = self.client.clone();
        let http_and_tok = client.as_ref().and_then(|c| {
            c.token()
                .filter(|t| !t.trim().is_empty())
                .map(|t| (c.http_client.clone(), t))
        });
        let app_id = installed.app_id.clone();
        let installed_ver = installed.installed_version.clone();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());

        cx.spawn(async move |weak: WeakEntity<Self>, app_cx: &mut AsyncApp| {
            let app_id_req = app_id.clone();
            let res = run_on_tokio(&handle, async move {
                let report = registry
                    .replay_script(&sid, &parsed, &params, resolver.as_deref())
                    .await
                    .map_err(|e| e.to_string())?;
                let mut recorded_run_at: Option<i64> = None;
                if let Some((http, tok)) = http_and_tok {
                    if let Ok(run_resp) = http
                        .record_automation_app_run(
                            &tok,
                            &app_id_req,
                            &RecordAutomationRunRequest {
                                version: Some(installed_ver),
                                success: report.success,
                                steps_executed: report.steps_executed,
                                total_duration_ms: report.total_duration_ms,
                                used_llm: report.used_llm,
                                error: report.error.clone(),
                            },
                        )
                        .await
                    {
                        recorded_run_at = Some(run_resp.last_run_at_ms);
                    }
                }
                Ok::<(ScriptReplayReport, Option<i64>), String>((report, recorded_run_at))
            })
            .await;

            if let Ok(Ok((_, Some(ts)))) = res {
                if let Some(ref c) = client {
                    c.local_store.mark_automation_app_run(&app_id, ts);
                }
            }

            let _ = weak.update(app_cx, |this, cx| {
                this.recorder.is_replaying = false;
                this.ensure_automation_session_listener(cx);
                match res {
                    Ok(Ok((report, _))) => {
                        if report.success {
                            this.store.store_error = None;
                            this.recorder.replay_error = None;
                        } else {
                            let err_msg = report
                                .error
                                .clone()
                                .unwrap_or_else(|| "Script replay failed".to_string());
                            this.store.store_error = Some(err_msg.clone());
                            this.recorder.replay_error = Some(err_msg);
                        }
                        if let Some(ref u) = report.final_url {
                            if !this.cdp_nav_in_flight && this.pending_cdp_nav.is_none() {
                                this.current_url = u.clone();
                                if this.active_input != ActiveBrowserInput::AddressBar {
                                    this.url_input = u.clone();
                                }
                            }
                        }
                        this.recorder.last_replay_report = Some(report);
                        if let Some(ref c) = this.client {
                            this.store.installed_apps = c.list_installed_automation_apps_cached();
                        }
                        if this.active_engine == BrowserEngineKind::RustCdp {
                            if let Some(ref surface) = this.cdp_surface {
                                if this.cdp_nav_in_flight {
                                    this.pending_cdp_refresh = true;
                                }
                                surface.update(cx, |s, scx| s.refresh_frame(scx));
                            }
                        }
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.recorder.replay_error = Some(err.clone());
                        this.store.store_error = Some(err);
                        if this.active_engine == BrowserEngineKind::RustCdp {
                            if let Some(ref surface) = this.cdp_surface {
                                if this.cdp_nav_in_flight {
                                    this.pending_cdp_refresh = true;
                                }
                                surface.update(cx, |s, scx| s.refresh_frame(scx));
                            }
                        }
                    }
                }
                cx.notify();
            });
            drop(client);
        })
        .detach();
        cx.notify();
    }

    fn sync_url_title_from_active_backend(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.active_backend.clone() else {
            return;
        };
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                let url = backend.current_url().await.unwrap_or_default();
                let title = backend.page_title().await.unwrap_or_default();
                (url, title)
            })
            .await;
            if let Ok((url, title)) = res {
                let _ = weak.update(app, |this, cx| {
                    if this.cdp_nav_in_flight || this.pending_cdp_nav.is_some() {
                        return;
                    }
                    if !url.is_empty() {
                        this.current_url = url.clone();
                        if this.active_input != ActiveBrowserInput::AddressBar {
                            this.url_input = url;
                        }
                    }
                    if !title.is_empty() {
                        this.page_title = title;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    #[cfg(target_os = "macos")]
    pub fn process_recorded_dom_event(&mut self, dom_ev: RecordedDomEvent) {
        let is_ai_controlling = matches!(
            self.control_mode(),
            BrowserControlMode::AiControlling { .. }
        );
        if !self.recorder.is_dom_recording || is_ai_controlling {
            return;
        }
        self.apply_recorded_dom_action(dom_ev);
    }

    #[cfg(target_os = "macos")]
    fn apply_recorded_dom_action(&mut self, dom_ev: RecordedDomEvent) {
        match dom_ev.event_kind.as_str() {
            "input" => {
                if let Some(prev) = self.pending_dom_fill.as_mut() {
                    if same_editable_element_locator(&prev.locator, &dom_ev.locator) {
                        *prev = dom_ev;
                    } else {
                        self.flush_pending_dom_fill();
                        self.pending_dom_fill = Some(dom_ev);
                    }
                } else {
                    self.pending_dom_fill = Some(dom_ev);
                }
            }
            "change" => {
                if let Some(prev) = self.pending_dom_fill.take() {
                    if same_editable_element_locator(&prev.locator, &dom_ev.locator) {
                        self.pending_dom_fill = Some(dom_ev);
                    } else {
                        self.pending_dom_fill = Some(prev);
                        self.flush_pending_dom_fill();
                        self.pending_dom_fill = Some(dom_ev);
                    }
                } else {
                    self.pending_dom_fill = Some(dom_ev);
                }
                self.flush_pending_dom_fill();
            }
            "click" => {
                self.flush_pending_dom_fill();
                if let Some(ref session) = self.automation_session {
                    session.record_click(Some(dom_ev.page_url), dom_ev.locator, 0);
                    self.recorder.recorded_dom_count =
                        self.recorder.recorded_dom_count.saturating_add(1);
                }
            }
            "submit" => {
                self.flush_pending_dom_fill();
                if let Some(ref session) = self.automation_session {
                    session.record_press_key(Some(dom_ev.page_url), "Enter".to_string(), 0);
                    self.recorder.recorded_dom_count =
                        self.recorder.recorded_dom_count.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn drain_wkwebview_events(&mut self, cx: &mut Context<Self>) {
        let events = match self.wkwebview.as_ref() {
            Some(wv) => wv.drain_webview_events(),
            None => return,
        };
        if events.is_empty() {
            return;
        }
        let is_ai_controlling = matches!(
            self.control_mode(),
            BrowserControlMode::AiControlling { .. }
        );
        for ev in events {
            match ev {
                WkWebViewEvent::NavigationChanged {
                    url,
                    can_go_back,
                    can_go_forward,
                } => {
                    self.current_url = url.clone();
                    if self.active_input != ActiveBrowserInput::AddressBar {
                        self.url_input = url.clone();
                    }
                    self.can_go_back = can_go_back;
                    self.can_go_forward = can_go_forward;
                    if self.recorder.is_dom_recording && !is_ai_controlling {
                        if self.last_recorded_nav_url.as_deref() != Some(url.as_str()) {
                            self.flush_pending_dom_fill();
                            self.last_recorded_nav_url = Some(url.clone());
                            if let Some(ref session) = self.automation_session {
                                session.record_navigate(&url, 0);
                            }
                        }
                    }
                }
                WkWebViewEvent::TitleChanged { title } => {
                    self.page_title = title;
                }
                WkWebViewEvent::LoadingChanged { is_loading } => {
                    self.is_loading = is_loading;
                }
                WkWebViewEvent::NavigationFailed { url, error } => {
                    self.is_loading = false;
                    self.last_error = Some(format!("Navigation failed ({url}): {error}"));
                }
                WkWebViewEvent::PopupRequested { url } => {
                    self.status_banner = Some(format!("Opened popup in tab: {url}"));
                }
                WkWebViewEvent::RecordedDomAction(dom_ev) => {
                    if !self.recorder.is_dom_recording || is_ai_controlling {
                        continue;
                    }
                    self.apply_recorded_dom_action(dom_ev);
                }
                WkWebViewEvent::BecameFirstResponder => {
                    if let Some(ref wv) = self.wkwebview {
                        wv.clear_became_first_responder();
                    }
                    if !self.is_workspace_occluded()
                        && self.active_input != ActiveBrowserInput::None
                    {
                        self.assign_active_input(ActiveBrowserInput::None);
                    }
                }
                WkWebViewEvent::BridgeMessage { .. } => {}
            }
        }
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    fn sync_native_wkwebview(
        &mut self,
        window: &mut Window,
        bounds: Bounds<Pixels>,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        if Self::is_headless_or_offline_env() {
            return;
        }
        if let Some(ref wv) = self.wkwebview {
            if !wv.matches_window(window) {
                if let Some(mut old) = self.wkwebview.take() {
                    old.destroy();
                }
                self.wkwebview_listener_started = false;
                self.active_backend = None;
                if !self.current_url.is_empty() {
                    self.wkwebview_pending_url = Some(self.current_url.clone());
                }
            }
        }
        if self.wkwebview.is_none() {
            let cfg = WkWebViewConfig::browser_profile(&self.active_profile_id);
            if let Some((wv, mut wakeup_rx)) = EmbeddedWkWebView::attach_with_config(window, cfg) {
                let backend: Arc<dyn BrowserSessionBackend> = wv.backend();
                wv.set_dom_recording(self.recorder.is_dom_recording);
                self.wkwebview = Some(wv);
                self.register_current_backend_with_registry(backend, cx);
                if self.pending_profile_cookie_restore {
                    self.pending_profile_cookie_restore = false;
                    self.restore_active_profile_cookies(cx);
                }
                if !self.wkwebview_listener_started {
                    self.wkwebview_listener_started = true;
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        while wakeup_rx.recv().await.is_some() {
                            let alive = weak
                                .update(app, |this, cx| {
                                    this.drain_wkwebview_events(cx);
                                })
                                .is_ok();
                            if !alive {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                if let Some(pending) = self.wkwebview_pending_url.take() {
                    if let Some(ref mut wv) = self.wkwebview {
                        wv.load_url(&pending);
                    }
                }
            }
        }
        self.drain_wkwebview_events(cx);
        if let Some(ref mut wv) = self.wkwebview {
            wv.set_allow_first_responder(visible);
            wv.sync_bounds(bounds, visible);
        }
    }
    /// Returns `(viewport_bounds, frame_pixel_size, css_viewport_size)` for Design Mode overlay projection.
    pub(crate) fn cdp_surface_geometry(
        &self,
        cx: &App,
    ) -> Option<(Bounds<Pixels>, (u32, u32), (u32, u32))> {
        let surface = self.cdp_surface.as_ref()?;
        let s = surface.read(cx);
        let bounds = s.viewport_bounds()?;
        let vp_sz = s.viewport_size();
        let img_sz = s.frame_pixel_size().unwrap_or(vp_sz);
        Some((bounds, img_sz, vp_sz))
    }

    pub fn set_on_design_handoff(
        &mut self,
        cb: Arc<dyn Fn(DesignHandoffTarget, bot_browser::DesignModeChatHandoff) + Send + Sync>,
    ) {
        self.on_design_handoff = Some(cb);
    }

    pub fn toggle_design_mode(&mut self, cx: &mut Context<Self>) {
        let next = !self.design_mode.enabled;
        self.set_design_mode_enabled(next, cx);
    }

    pub fn set_design_mode_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.design_mode.enabled == enabled {
            if enabled {
                self.inspector_open = true;
                self.set_inspector_tab(BrowserInspectorTab::Design, cx);
            }
            return;
        }
        self.design_mode.enabled = enabled;
        if enabled {
            self.design_mode.picker_active = true;
            self.design_mode.last_error = None;
            self.inspector_open = true;
            self.set_inspector_tab(BrowserInspectorTab::Design, cx);
        } else {
            self.design_mode.picker_active = false;
            self.design_mode.hover_target = None;
            self.design_mode.last_hover_css_point = None;
        }
        self.sync_cdp_surface_interactivity(cx);
        cx.notify();
    }

    pub fn set_design_picker_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.design_mode.picker_active = active;
        if !active {
            self.design_mode.hover_target = None;
            self.design_mode.last_hover_css_point = None;
        }
        self.sync_cdp_surface_interactivity(cx);
        cx.notify();
    }

    pub fn clear_design_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.design_mode.clear_selection() {
            cx.drop_image(prev, None);
        }
        cx.notify();
    }

    fn resolve_design_backend(&self) -> Option<Arc<dyn BrowserSessionBackend>> {
        self.cdp_backend
            .clone()
            .or_else(|| self.active_backend.clone())
    }

    fn apply_design_capture_result(
        &mut self,
        res: Result<bot_browser::DesignModeCapture, String>,
        cx: &mut Context<Self>,
    ) {
        self.design_mode.is_capturing = false;
        match res {
            Ok(capture) => {
                let decoded_preview = capture
                    .cropped_png_bytes
                    .as_deref()
                    .and_then(|bytes| decode_png_to_render_image(bytes).ok());
                if let Some(prev) = self.design_mode.preview_image.take() {
                    cx.drop_image(prev, None);
                }
                let selector = capture.selector.clone();
                self.design_mode.apply_capture(capture);
                if let Some((img, dims)) = decoded_preview {
                    self.design_mode.preview_image = Some(img);
                    self.design_mode.preview_dimensions = Some(dims);
                }
                self.design_mode.last_status = Some(format!("Captured element: {selector}"));
                self.inspector_open = true;
                self.inspector_tab = BrowserInspectorTab::Design;
            }
            Err(err) => {
                self.design_mode.last_error = Some(err);
            }
        }
        cx.notify();
    }

    pub fn probe_design_hover_at_window_point(
        &mut self,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.design_mode.enabled || !self.design_mode.picker_active {
            return;
        }
        let Some((bounds, img_sz, vp_sz)) = self.cdp_surface_geometry(cx) else {
            return;
        };
        let (css_x, css_y) = map_point_contain(pos, bounds, img_sz, vp_sz);
        if let Some((prev_x, prev_y)) = self.design_mode.last_hover_css_point {
            if (css_x - prev_x).abs() < 3.0 && (css_y - prev_y).abs() < 3.0 {
                return;
            }
        }
        let Some(backend) = self.resolve_design_backend() else {
            return;
        };
        self.design_mode.last_hover_css_point = Some((css_x, css_y));
        self.design_mode.hover_probe_generation =
            self.design_mode.hover_probe_generation.wrapping_add(1);
        let generation = self.design_mode.hover_probe_generation;
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let result = run_on_tokio(&handle, async move {
                bot_browser::design_mode::probe_element_at_point(backend.as_ref(), css_x, css_y)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.design_mode.hover_probe_generation != generation
                    || !this.design_mode.enabled
                    || !this.design_mode.picker_active
                {
                    return;
                }
                if let Ok(Ok(target)) = result {
                    this.design_mode.hover_target = target;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn capture_design_element_at_window_point(
        &mut self,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((bounds, img_sz, vp_sz)) = self.cdp_surface_geometry(cx) else {
            self.design_mode.last_error =
                Some("Browser viewport bounds not ready for Design Mode capture".to_string());
            cx.notify();
            return;
        };
        let Some(backend) = self.resolve_design_backend() else {
            self.design_mode.last_error =
                Some("Navigate to a page first to initialize the Rust CDP backend".to_string());
            cx.notify();
            return;
        };
        let (css_x, css_y) = map_point_contain(pos, bounds, img_sz, vp_sz);
        self.design_mode.capture_generation = self.design_mode.capture_generation.wrapping_add(1);
        let generation = self.design_mode.capture_generation;
        self.design_mode.is_capturing = true;
        self.design_mode.last_error = None;
        let opts = bot_browser::DesignModeCaptureOptions::default();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let result = run_on_tokio(&handle, async move {
                bot_browser::design_mode::capture_element_at_point(
                    backend.as_ref(),
                    css_x,
                    css_y,
                    &opts,
                )
                .await
                .map_err(|e| e.to_string())
            })
            .await;
            let flat = match result {
                Ok(inner) => inner,
                Err(e) => Err(e),
            };
            let _ = weak.update(app, |this, cx| {
                if this.design_mode.capture_generation != generation {
                    return;
                }
                this.apply_design_capture_result(flat, cx);
            });
        })
        .detach();
    }

    pub fn capture_design_element_by_selector(
        &mut self,
        selector: String,
        cx: &mut Context<Self>,
    ) {
        let trimmed = selector.trim().to_string();
        if trimmed.is_empty() {
            self.design_mode.last_error = Some("Enter a CSS selector to capture".to_string());
            cx.notify();
            return;
        }
        let Some(backend) = self.resolve_design_backend() else {
            self.design_mode.last_error =
                Some("Navigate to a page first to initialize the Rust CDP backend".to_string());
            cx.notify();
            return;
        };
        self.design_mode.capture_generation = self.design_mode.capture_generation.wrapping_add(1);
        let generation = self.design_mode.capture_generation;
        self.design_mode.is_capturing = true;
        self.design_mode.last_error = None;
        let opts = bot_browser::DesignModeCaptureOptions::default();
        let handle = resolve_browser_tokio_handle(self.tokio_handle.as_ref());
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let result = run_on_tokio(&handle, async move {
                bot_browser::design_mode::capture_element_by_selector(
                    backend.as_ref(),
                    &trimmed,
                    &opts,
                )
                .await
                .map_err(|e| e.to_string())
            })
            .await;
            let flat = match result {
                Ok(inner) => inner,
                Err(e) => Err(e),
            };
            let _ = weak.update(app, |this, cx| {
                if this.design_mode.capture_generation != generation {
                    return;
                }
                this.apply_design_capture_result(flat, cx);
            });
        })
        .detach();
    }

    pub fn send_design_capture_to_target(
        &mut self,
        target: DesignHandoffTarget,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let handoff = self.design_mode.build_handoff_payload()?;
        self.design_mode.last_handoff = Some(handoff.clone());
        self.design_mode.last_error = None;

        match &target {
            DesignHandoffTarget::BrowserCopilot => {
                self.recorder.copilot_prompt_draft = handoff.prompt_markdown.clone();
                self.inspector_open = true;
                self.set_inspector_tab(BrowserInspectorTab::Copilot, cx);
                self.design_mode.last_status =
                    Some("Design capture loaded into Browser Copilot draft".to_string());
            }
            DesignHandoffTarget::OrcaBot { .. } => {
                self.design_mode.last_status =
                    Some("Design capture dispatched to Orca Bot".to_string());
            }
            DesignHandoffTarget::ActiveChat => {
                self.design_mode.last_status =
                    Some("Design capture dispatched to Active Chat".to_string());
            }
        }

        if let Some(ref cb) = self.on_design_handoff {
            cb(target.clone(), handoff.clone());
        }
        cx.emit(BrowserWorkspaceEvent::SendDesignCaptureToChat { target, handoff });
        cx.notify();
        Ok(())
    }
}

impl EventEmitter<BrowserWorkspaceEvent> for BrowserWorkspace {}

impl Focusable for BrowserWorkspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BrowserWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(ref surface) = self.cdp_surface {
            let surf_id = surface.entity_id();
            let win_id = window.window_handle().window_id();
            let needs_focus_sub = self
                ._cdp_focus_subscription
                .as_ref()
                .is_none_or(|(eid, wid, _)| *eid != surf_id || *wid != win_id);
            if needs_focus_sub {
                let surf_focus = surface.read(cx).focus_handle(cx);
                let sub = cx.on_focus_in(&surf_focus, window, |this, _window, cx| {
                    if !this.is_workspace_occluded() && this.active_input != ActiveBrowserInput::None {
                        this.assign_active_input(ActiveBrowserInput::None);
                        cx.notify();
                    }
                });
                self._cdp_focus_subscription = Some((surf_id, win_id, sub));
            }
        } else {
            self._cdp_focus_subscription = None;
        }
        let pal = self.palette;
        let lang = self.language_id.clone();
        let occluded = self.is_occluded();

        let (takeover_label, takeover_col) = match &self.control_mode {
            BrowserControlMode::Idle => (tr_str(&lang, "空闲", "Idle").to_string(), pal.text_secondary),
            BrowserControlMode::AiControlling { status_text, .. } => (
                format!("AI: {status_text}"),
                pal.accent,
            ),
            BrowserControlMode::WaitingForTakeover { reason } => (
                format!("{}: {reason}", tr_str(&lang, "等待人工接管", "Waiting for Takeover")),
                0xE67E22,
            ),
            BrowserControlMode::UserTakeover { paused_at_step } => (
                format!(
                    "{} (#{})",
                    tr_str(&lang, "已接管", "User Takeover"),
                    paused_at_step + 1
                ),
                pal.success,
            ),
        };

        let is_ai_or_paused = !matches!(self.control_mode, BrowserControlMode::Idle);
        let can_resume = matches!(
            self.control_mode,
            BrowserControlMode::WaitingForTakeover { .. } | BrowserControlMode::UserTakeover { .. }
        );

        let inspector_tabs: &[(BrowserInspectorTab, &str)] = &[
            (BrowserInspectorTab::Copilot, "Copilot"),
            (BrowserInspectorTab::Recorder, "Script Apps"),
            (BrowserInspectorTab::Session, "Session"),
            (BrowserInspectorTab::Store, "Store"),
            (BrowserInspectorTab::Design, "Design"),
        ];

        let inspector_body: AnyElement = match self.inspector_tab {
            BrowserInspectorTab::Copilot => render_copilot_tab(self, pal, &lang, cx),
            BrowserInspectorTab::Recorder => render_recorder_tab(self, pal, &lang, cx),
            BrowserInspectorTab::Session => render_session_tab(self, pal, &lang, cx),
            BrowserInspectorTab::Store => render_store_tab(self, pal, &lang, cx),
            BrowserInspectorTab::Design => render_design_mode_tab(self, pal, &lang, cx),
        };

        let addr_active = self.active_input == ActiveBrowserInput::AddressBar;

        let viewport_element: AnyElement = if self.active_engine == BrowserEngineKind::WkWebView
            && cfg!(target_os = "macos")
            && !Self::is_headless_or_offline_env()
        {
            #[cfg(target_os = "macos")]
            {
                let entity = cx.entity().clone();
                div()
                    .id("browser-wkwebview-host")
                    .relative()
                    .flex_1()
                    .size_full()
                    .bg(rgb(pal.history_bg))
                    .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                        if this.active_input != ActiveBrowserInput::None {
                            this.assign_active_input(ActiveBrowserInput::None);
                            cx.notify();
                        }
                    }))
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |bounds, _, window, cx| {
                                entity.update(cx, |this, wcx| {
                                    this.sync_native_wkwebview(window, bounds, !occluded, wcx);
                                });
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .into_any_element()
            }
            #[cfg(not(target_os = "macos"))]
            {
                div().flex_1().into_any_element()
            }
        } else if let Some(ref surface) = self.cdp_surface {
            div()
                .id("browser-cdp-viewport-host")
                .relative()
                .flex_1()
                .size_full()
                .min_h(px(0.0))
                .overflow_hidden()
                .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                    if this.active_input != ActiveBrowserInput::None {
                        this.assign_active_input(ActiveBrowserInput::None);
                        cx.notify();
                    }
                }))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if !this.is_workspace_occluded()
                        && this.is_cdp_viewport_focused(window, cx)
                        && !ev.keystroke.modifiers.platform
                        && !ev.keystroke.modifiers.control
                    {
                        cx.stop_propagation();
                    }
                }))
                .child(surface.clone())
                .when(self.design_mode.enabled, |host| {
                    host.child(render_design_mode_overlay(self, pal, &lang, cx))
                })
                .into_any_element()
        } else {
            div()
                .id("browser-empty-viewport")
                .flex_1()
                .size_full()
                .bg(rgb(pal.history_bg))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .child(tg_icon(IconName::ScreenShare, 32.0, pal.text_secondary))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            &lang,
                            "输入网址并按 Enter 开始浏览或驱动 Rust CDP 会话",
                            "Enter a URL above to browse with Rust CDP",
                        )),
                )
                .into_any_element()
        };

        div()
            .id("browser-workspace-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if this.is_workspace_occluded() {
                    return;
                }
                let key = ev.keystroke.key.as_str();
                if ev.keystroke.modifiers.platform && key.eq_ignore_ascii_case("v") {
                    if let Some(clip) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        this.insert_text_to_active_input(&clip, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                if this.handle_non_char_key_input(key, ev.keystroke.modifiers.alt, cx) {
                    cx.stop_propagation();
                    return;
                }
                if let Some(ref ch) = ev.keystroke.key_char {
                    if this.insert_text_to_active_input(ch, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .h_full()
            .overflow_hidden()
            .bg(rgb(pal.window_bg))
            // Top Toolbar
            .child(
                div()
                    .flex_shrink_0()
                    .w_full()
                    .px(px(12.0))
                    .py(px(8.0))
                    .bg(rgb(pal.top_bar_bg))
                    .border_b_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("browser-nav-back-btn")
                            .px(px(7.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.go_back(cx)),
                            )
                            .child(tg_icon(IconName::BackArrow, 14.0, pal.text_primary)),
                    )
                    .child(
                        div()
                            .id("browser-nav-forward-btn")
                            .px(px(7.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.go_forward(cx)),
                            )
                            .child(tg_icon(IconName::Forward, 14.0, pal.text_primary)),
                    )
                    .child(
                        div()
                            .id("browser-nav-reload-btn")
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.reload(cx)),
                            )
                            .child(if self.is_loading { "Stop" } else { "Reload" }),
                    )
                    // Editable URL / Address Bar
                    .child(
                        div()
                            .id("browser-address-bar-input")
                            .flex_1()
                            .min_w(px(140.0))
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(rgb(if addr_active {
                                pal.accent
                            } else {
                                pal.border_fg
                            }))
                            .cursor_text()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.set_active_input(
                                        ActiveBrowserInput::AddressBar,
                                        Some(window),
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            )
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .child(if addr_active {
                                format!("{}|", self.url_input)
                            } else if self.url_input.is_empty() {
                                "https://".to_string()
                            } else {
                                self.url_input.clone()
                            }),
                    )
                    .child(
                        div()
                            .id("browser-nav-go-btn")
                            .px(px(10.0))
                            .py(px(5.5))
                            .rounded(px(7.0))
                            .bg(rgb(pal.accent))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.active_button_fg))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    let url = this.url_input.clone();
                                    this.navigate(&url, cx);
                                }),
                            )
                            .child("Go"),
                    )
                    // Profile Badge
                    .child(
                        div()
                            .id("browser-profile-pill")
                            .px(px(8.0))
                            .py(px(4.5))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.inspector_open = true;
                                    this.set_inspector_tab(BrowserInspectorTab::Session, cx);
                                }),
                            )
                            .child(format!("Profile: {}", self.active_profile_id)),
                    )
                    // Engine Toggle Badge (`WKWebView` / `Rust CDP`)
                    .child(
                        div()
                            .id("browser-engine-toggle-btn")
                            .px(px(8.0))
                            .py(px(4.5))
                            .rounded(px(6.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.accent))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    let next = match this.active_engine {
                                        BrowserEngineKind::WkWebView => BrowserEngineKind::RustCdp,
                                        BrowserEngineKind::RustCdp => BrowserEngineKind::WkWebView,
                                    };
                                    this.set_engine_kind(next, cx);
                                }),
                            )
                            .child(self.active_engine.display_name()),
                    )
                    // Design Mode Toggle Pill (`Cmd+Shift+I`)
                    .child(
                        div()
                            .id("browser-design-mode-toggle-btn")
                            .px(px(8.0))
                            .py(px(4.5))
                            .rounded(px(6.0))
                            .bg(if self.design_mode.enabled {
                                rgb(pal.accent)
                            } else {
                                rgb(pal.input_bg)
                            })
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if self.design_mode.enabled {
                                pal.active_button_fg
                            } else {
                                pal.text_primary
                            }))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_design_mode(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(tg_icon(
                                IconName::Camera,
                                12.0,
                                if self.design_mode.enabled {
                                    pal.active_button_fg
                                } else {
                                    pal.accent
                                },
                            ))
                            .child(if self.design_mode.enabled {
                                tr_str(&lang, "设计模式: 开", "Design: On")
                            } else {
                                tr_str(&lang, "设计模式", "Design")
                            }),
                    )
                    // Inspector Collapse Toggle
                    .child(
                        div()
                            .id("browser-inspector-toggle-btn")
                            .px(px(8.0))
                            .py(px(4.5))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.inspector_open = !this.inspector_open;
                                    if !this.inspector_open
                                        && this.active_input.owner_inspector_tab().is_some()
                                    {
                                        this.assign_active_input(ActiveBrowserInput::None);
                                    }
                                    cx.notify();
                                }),
                            )
                            .child(if self.inspector_open { "Hide Panel" } else { "Show Panel" }),
                    ),
            )
            // Control / Takeover Status Banner
            .child(
                div()
                    .w_full()
                    .px(px(12.0))
                    .py(px(6.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(takeover_col, 0.12))
                    .border_b_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(takeover_col))
                                    .child(takeover_label),
                            )
                            .when(!self.page_title.is_empty(), |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("· {}", self.page_title)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("browser-takeover-btn")
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(0xE67E22))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.user_take_over(cx)),
                                    )
                                    .child(tr_str(&lang, "接管控制", "Take Over")),
                            )
                            .when(can_resume, |row| {
                                row.child(
                                    div()
                                        .id("browser-resume-ai-btn")
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(pal.success))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| this.user_resume_ai(cx)),
                                        )
                                        .child(tr_str(&lang, "交还 AI", "Resume AI")),
                                )
                            })
                            .when(is_ai_or_paused, |row| {
                                row.child(
                                    div()
                                        .id("browser-cancel-ai-btn")
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(pal.danger))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| this.user_cancel_ai(cx)),
                                        )
                                        .child(tr_str(&lang, "取消 AI", "Cancel AI")),
                                )
                            }),
                    ),
            )
            // Optional Status / Error Strip
            .when_some(self.last_error.clone(), |col, err| {
                col.child(
                    div()
                        .w_full()
                        .px(px(12.0))
                        .py(px(5.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                        .text_xs()
                        .text_color(rgb(pal.danger))
                        .child(err),
                )
            })
            // Split-Pane Body: Viewport (Left/Center) + Collapsible Inspector (Right)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(viewport_element),
                    )
                    .when(self.inspector_open, |row| {
                        row.child(
                            div()
                                .w(px(360.0))
                                .h_full()
                                .flex_shrink_0()
                                .bg(rgb(pal.dialogs_bg))
                                .border_l_1()
                                .border_color(pal.separator_rgba())
                                .p(px(10.0))
                                .flex()
                                .flex_col()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .children(inspector_tabs.iter().map(|(tab, title)| {
                                            let is_sel = self.inspector_tab == *tab;
                                            let t = *tab;
                                            div()
                                                .flex_1()
                                                .px(px(6.0))
                                                .py(px(5.0))
                                                .rounded(px(6.0))
                                                .bg(rgb(if is_sel {
                                                    pal.accent
                                                } else {
                                                    pal.input_bg
                                                }))
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(if is_sel {
                                                    pal.active_button_fg
                                                } else {
                                                    pal.text_secondary
                                                }))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.set_inspector_tab(t, cx);
                                                    }),
                                                )
                                                .child(*title)
                                        })),
                                )
                                .child(inspector_body),
                        )
                    }),
            )
            .into_any_element()
    }
}
