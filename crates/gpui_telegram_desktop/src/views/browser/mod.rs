//! Built-in Interactive Browser views (`BrowserWorkspace`, `CdpViewportSurface`,
//! `recorder_panel`, `store_panel`, and engine metadata).

pub mod cdp_surface;
pub mod design_mode;
pub mod recorder_panel;
pub mod store_panel;
pub mod workspace;

pub use cdp_surface::{decode_png_to_render_image, map_point_contain, CdpViewportSurface};
pub use design_mode::{
    contain_draw_rect, project_css_rect_to_overlay_px, project_norm_bp_to_overlay_px,
    BrowserWorkspaceEvent, DesignHandoffTarget, DesignModePanelState, OverlayBoxPx,
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
