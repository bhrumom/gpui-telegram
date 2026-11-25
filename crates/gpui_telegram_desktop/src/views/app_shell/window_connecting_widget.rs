//! WindowConnectingWidget: Top status bar showing connection progress, updates, or proxy state
//! Upstream: Telegram/SourceFiles/window/window_connecting_widget.h, window_connecting_widget.cpp

use std::time::{Duration, Instant};

use gpui::*;
use serde::{Deserialize, Serialize};

/// Telegram Desktop `kIgnoreStartTimeout`: avoid flashing a "Connecting..." indicator during
/// normal fast (<1.2s) startup or brief WebSocket handshakes.
pub const CONNECTING_BANNER_GRACE_MS: u64 = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    Updating,
    WaitingForNetwork,
    ProxyConnecting,
}

#[derive(Debug, Clone)]
pub struct WindowConnectingWidget {
    pub status: ConnectionStatus,
    pub is_visible: bool,
    pub proxy_host: Option<String>,
    pub status_changed_at: Instant,
}

impl WindowConnectingWidget {
    pub fn new() -> Self {
        Self {
            status: ConnectionStatus::Connected,
            is_visible: false,
            proxy_host: None,
            status_changed_at: Instant::now(),
        }
    }

    pub fn set_status(&mut self, status: ConnectionStatus) {
        if self.status != status {
            self.status = status;
            self.status_changed_at = Instant::now();
        }
        self.is_visible = status != ConnectionStatus::Connected;
    }

    pub fn set_proxy_host(&mut self, host: Option<String>) {
        self.proxy_host = host;
    }

    /// Returns `true` when the connecting indicator should actually be painted on screen.
    /// `WaitingForNetwork` appears immediately; transient `Connecting`/`Updating` waits for the
    /// Telegram-style grace delay so fast handshakes never cause visual flicker.
    pub fn should_show_banner(&self) -> bool {
        if !self.is_visible {
            return false;
        }
        match self.status {
            ConnectionStatus::Connected => false,
            ConnectionStatus::WaitingForNetwork => true,
            ConnectionStatus::Connecting
            | ConnectionStatus::Updating
            | ConnectionStatus::ProxyConnecting => {
                self.status_changed_at.elapsed() >= Duration::from_millis(CONNECTING_BANNER_GRACE_MS)
            }
        }
    }

    pub fn status_text(&self) -> &'static str {
        match self.status {
            ConnectionStatus::Connected => "Connected",
            ConnectionStatus::Connecting => "Connecting...",
            ConnectionStatus::Updating => "Updating...",
            ConnectionStatus::WaitingForNetwork => "Waiting for network...",
            ConnectionStatus::ProxyConnecting => "Connecting through proxy...",
        }
    }

    pub fn localized_status_text(&self, lang_id: &str) -> &'static str {
        if lang_id.starts_with("zh") {
            match self.status {
                ConnectionStatus::Connected => "已连接",
                ConnectionStatus::Connecting => "连接中…",
                ConnectionStatus::Updating => "正在同步…",
                ConnectionStatus::WaitingForNetwork => "等待网络…",
                ConnectionStatus::ProxyConnecting => "正在通过代理连接…",
            }
        } else {
            self.status_text()
        }
    }
}

impl Default for WindowConnectingWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for WindowConnectingWidget {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(28.0))
            .w_full()
            .bg(rgb(0x2B5278))
            .text_color(rgb(0xFFFFFF))
            .text_xs()
            .child(self.status_text())
            .into_any_element()
    }
}
