//! Built-in Interactive Browser views (`BrowserWorkspace`, `CdpViewportSurface`,
//! `recorder_panel`, `store_panel`, and engine metadata).

pub mod cdp_surface;
pub mod design_mode;
pub mod openbot_browser_chrome;
pub mod openbot_full_view;
pub mod openbot_session;
pub mod recorder_panel;
pub mod store_panel;
pub mod workspace;

pub use cdp_surface::{decode_png_to_render_image, map_point_contain, CdpViewportSurface};
pub use design_mode::{
    contain_draw_rect, project_css_rect_to_overlay_px, project_norm_bp_to_overlay_px,
    BrowserWorkspaceEvent, DesignHandoffTarget, DesignModePanelState, OverlayBoxPx,
};
pub use openbot_browser_chrome::{
    browser_profile_hue_color, openbot_browser_icon, render_browser_owner_avatar,
    render_browser_preview_thumbnail_content, OpenBotBrowserIcon,
};
pub use openbot_session::{
    browser_action_label, browser_address_url, browser_completion, browser_display_url,
    browser_search_query, browser_search_url, browser_shortcut_glyph, browser_suggestions,
    format_teach_message_for_chat, is_recordable_browser_url, resolve_browser_view_command,
    BrowserDockUiState, BrowserHistoryEntry, BrowserIslandView, BrowserPopupFailure,
    BrowserShortcutName, BrowserShortcutPlatform, BrowserSuggestion, BrowserSuggestionKind,
    BrowserTabEntry, BrowserTabIndexTarget, BrowserTabOwner, BrowserTabPreview,
    BrowserTeachUiState, BrowserViewCommand, ClosedBrowserTab, OpenBotBrowserSession, TabPlacement,
    BROWSER_ADDRESS_ROW_HEIGHT_PX, BROWSER_BUSY_CAPTURE_INTERVAL_MS, BROWSER_CLOSED_LIMIT,
    BROWSER_DOCK_MAX_WIDTH_PX, BROWSER_DOCK_REPLY_CLOSE_MS, BROWSER_FULL_TOP_HEIGHT_PX,
    BROWSER_HISTORY_LIMIT, BROWSER_IDLE_CAPTURE_INTERVAL_MS, BROWSER_MAC_TRAFFIC_LIGHTS_INSET_PX,
    BROWSER_MAX_SUGGESTIONS, BROWSER_MAX_TABS, BROWSER_NEW_TAB_URL, BROWSER_PREVIEW_ASPECT_RATIO,
    BROWSER_SEARCH_URL, BROWSER_TAB_MAX_WIDTH_PX, BROWSER_TAB_MIN_WIDTH_PX,
};
pub use recorder_panel::{BrowserLogEntry, BrowserLogLevel, ChromeProbeUiState, RecorderPanelState};
pub use store_panel::StorePanelState;
pub use workspace::{
    ActiveBrowserInput, BrowserInspectorTab, BrowserWorkspace, BrowserWorkspaceConfig,
};

/// Active rendering engine backing a built-in browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserEngineKind {
    /// Native macOS out-of-process WebKit (`EmbeddedWkWebView`) for 120Hz interactive parity.
    WkWebView,
    /// Cross-platform / headless Rust CDP offscreen viewport (`CdpViewportSurface`).
    RustCdp,
}

impl BrowserEngineKind {
    pub fn default_for_platform() -> Self {
        if cfg!(target_os = "macos") {
            Self::WkWebView
        } else {
            Self::RustCdp
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::WkWebView => "WKWebView",
            Self::RustCdp => "Rust CDP",
        }
    }

    pub fn is_native_wkwebview(self) -> bool {
        matches!(self, Self::WkWebView)
    }

    pub fn supports_cdp_surface(self) -> bool {
        matches!(self, Self::RustCdp)
    }
}

/// Inspectable viewport telemetry summary shared across browser surface implementations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserViewportStats {
    pub engine: BrowserEngineKind,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub frame_revision: u64,
    pub has_frame: bool,
}
