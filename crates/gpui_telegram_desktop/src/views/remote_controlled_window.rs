//! 1:1 Controlled Status Window ("本机被控中") modeled after NetEase UU Remote's
//! active controlled-state window, with human (#1E6BFF) vs AI MCP (teal/indigo)
//! banners, live HH:MM:SS timer, collapse-to-floating-pill toggle, grouped cards,
//! per-session disconnect, and instant local + server kill-switch.

use std::sync::Arc;
use std::time::Duration;

use fabushi_remote_engine::control_lease::RemoteControlHub;
use gpui::*;
use gpui_telegram_proto::remote_control_contracts::{
    RemoteControlMode, RemoteControllerKind, RemoteSessionDto,
};
use gpui_telegram_proto::tailnet_contracts::TailnetRelayMode;

use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};

pub const UU_HUMAN_BANNER_HEX: u32 = 0x1E6BFF;
pub const AI_MCP_TEAL_BANNER_HEX: u32 = 0x0F766E;
pub const MIXED_INDIGO_BANNER_HEX: u32 = 0x4338CA;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlledWindowAction {
    DisconnectAll {
        disconnected_local_count: usize,
        session_ids: Vec<String>,
    },
    DisconnectSession {
        session_id: String,
        disconnected_local: bool,
    },
    ToggleCollapse(bool),
    CloseRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlledBannerVariant {
    /// Human remote control (UU vibrant blue `#1E6BFF`).
    HumanBlue,
    /// Pure AI MCP control (teal `#0F766E`).
    AiTeal,
    /// Mixed human + AI MCP control (indigo `#4338CA`).
    MixedIndigo,
    /// No active sessions (`Idle` subtle blue-slate).
    Idle,
}

impl ControlledBannerVariant {
    pub fn bg_hex(self) -> u32 {
        match self {
            Self::HumanBlue => UU_HUMAN_BANNER_HEX,
            Self::AiTeal => AI_MCP_TEAL_BANNER_HEX,
            Self::MixedIndigo => MIXED_INDIGO_BANNER_HEX,
            Self::Idle => 0x1E293B,
        }
    }

    pub fn border_hex(self) -> u32 {
        match self {
            Self::HumanBlue => 0x3B82F6,
            Self::AiTeal => 0x14B8A6,
            Self::MixedIndigo => 0x6366F1,
            Self::Idle => 0x334155,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlledSessionGroup {
    pub kind: RemoteControllerKind,
    pub title_zh: &'static str,
    pub title_en: &'static str,
    pub sessions: Vec<RemoteSessionDto>,
}

/// Formats `elapsed_ms` (clamped at 0) as `HH:MM:SS`.
pub fn format_elapsed_hms(elapsed_ms: i64) -> String {
    let total_secs = (elapsed_ms.max(0) / 1000) as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Computes `banner_variant` from active sessions.
pub fn resolve_banner_variant(sessions: &[RemoteSessionDto]) -> ControlledBannerVariant {
    if sessions.is_empty() {
        return ControlledBannerVariant::Idle;
    }
    let has_ai = sessions
        .iter()
        .any(|s| matches!(s.controller_kind, RemoteControllerKind::AiMcp));
    let has_human = sessions.iter().any(|s| {
        matches!(
            s.controller_kind,
            RemoteControllerKind::OwnDevice | RemoteControllerKind::AssistGuest
        )
    });
    match (has_ai, has_human) {
        (true, true) => ControlledBannerVariant::MixedIndigo,
        (true, false) => ControlledBannerVariant::AiTeal,
        (false, _) => ControlledBannerVariant::HumanBlue,
    }
}

/// Groups active sessions into NetEase UU-style categorized sections:
/// **"本账号设备"** / **"远程协助设备"** / **"AI Agent (MCP)"** (+ **"本地 CLI"** if present).
pub fn group_controlled_sessions(sessions: &[RemoteSessionDto]) -> Vec<ControlledSessionGroup> {
    let buckets = [
        (
            RemoteControllerKind::OwnDevice,
            "本账号设备",
            "Own Account Devices",
        ),
        (
            RemoteControllerKind::AssistGuest,
            "远程协助设备",
            "Remote Assist Devices",
        ),
        (
            RemoteControllerKind::AiMcp,
            "AI Agent (MCP)",
            "AI Agent (MCP)",
        ),
        (RemoteControllerKind::Cli, "本地 CLI", "Local CLI"),
    ];

    let mut groups = Vec::new();
    for (kind, title_zh, title_en) in buckets {
        let mut matched: Vec<RemoteSessionDto> = sessions
            .iter()
            .filter(|s| s.controller_kind == kind)
            .cloned()
            .collect();
        if !matched.is_empty() {
            matched.sort_by_key(|s| s.started_at_ms);
            groups.push(ControlledSessionGroup {
                kind,
                title_zh,
                title_en,
                sessions: matched,
            });
        }
    }
    groups
}

/// Dedicated GPUI view representing "本机被控中" (Controlled Status Window).
pub struct RemoteControlledWindow {
    pub hub: Arc<RemoteControlHub>,
    pub sessions: Vec<RemoteSessionDto>,
    pub collapsed_to_pill: bool,
    pub language_id: String,
    pub theme_mode: TdesktopThemeMode,
    pub should_close: bool,
    /// Optional deterministic timestamp override for unit tests (`chrono::Utc::now()` when `None`).
    pub now_ms_override: Option<i64>,
    pub pending_actions: Vec<RemoteControlledWindowAction>,
    focus_handle: Option<FocusHandle>,
    _timer_started: bool,
}

impl RemoteControlledWindow {
    /// Creates an offline/headless instance suitable for unit tests without a GPUI window context.
    pub fn new_offline(hub: Arc<RemoteControlHub>, language_id: impl Into<String>) -> Self {
        let sessions = hub.list_sessions();
        Self {
            hub,
            sessions,
            collapsed_to_pill: false,
            language_id: language_id.into(),
            theme_mode: TdesktopThemeMode::Night,
            should_close: false,
            now_ms_override: None,
            pending_actions: Vec::new(),
            focus_handle: None,
            _timer_started: false,
        }
    }

    pub fn new(
        hub: Arc<RemoteControlHub>,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new_offline(hub, language_id);
        this.focus_handle = Some(cx.focus_handle());
        this.spawn_live_tick_loop(cx);
        this
    }

    fn spawn_live_tick_loop(&mut self, cx: &mut Context<Self>) {
        if self._timer_started {
            return;
        }
        self._timer_started = true;
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| loop {
            app.background_executor().timer(Duration::from_secs(1)).await;
            let keep = weak
                .update(app, |this, cx| {
                    this.refresh_from_hub();
                    cx.notify();
                    !this.should_close
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        })
        .detach();
    }

    pub fn current_now_ms(&self) -> i64 {
        self.now_ms_override
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
    }

    /// Merges sessions from local `RemoteControlHub` while preserving any external/server sessions
    /// passed in via `sync_server_sessions`.
    pub fn refresh_from_hub(&mut self) {
        let local = self.hub.list_sessions();
        if !local.is_empty() {
            let mut merged = local;
            for existing in &self.sessions {
                if !merged.iter().any(|s| s.session_id == existing.session_id)
                    && self.hub.get_session(&existing.session_id).is_some()
                {
                    merged.push(existing.clone());
                }
            }
            merged.sort_by_key(|s| s.started_at_ms);
            self.sessions = merged;
        }
    }

    /// Replaces the displayed sessions list with an authoritative combined list (local hub + server).
    pub fn set_sessions(&mut self, server_sessions: Vec<RemoteSessionDto>) {
        let mut merged = self.hub.list_sessions();
        for sess in server_sessions {
            if !merged.iter().any(|s| s.session_id == sess.session_id) {
                merged.push(sess);
            }
        }
        merged.sort_by_key(|s| s.started_at_ms);
        self.sessions = merged;
    }

    pub fn set_language_id(&mut self, lang: impl Into<String>) {
        self.language_id = lang.into();
    }

    pub fn set_theme_mode(&mut self, mode: TdesktopThemeMode) {
        self.theme_mode = mode;
    }

    pub fn take_actions(&mut self) -> Vec<RemoteControlledWindowAction> {
        std::mem::take(&mut self.pending_actions)
    }

    pub fn banner_variant(&self) -> ControlledBannerVariant {
        resolve_banner_variant(&self.sessions)
    }

    pub fn earliest_elapsed_hms(&self) -> String {
        let now_ms = self.current_now_ms();
        let earliest = self
            .sessions
            .iter()
            .map(|s| s.started_at_ms)
            .filter(|&ts| ts > 0)
            .min()
            .unwrap_or(now_ms);
        format_elapsed_hms(now_ms.saturating_sub(earliest))
    }

    pub fn session_elapsed_hms(&self, session: &RemoteSessionDto) -> String {
        let now_ms = self.current_now_ms();
        let started = if session.started_at_ms > 0 {
            session.started_at_ms
        } else {
            now_ms
        };
        format_elapsed_hms(now_ms.saturating_sub(started))
    }

    pub fn toggle_collapse(&mut self) {
        self.collapsed_to_pill = !self.collapsed_to_pill;
        self.pending_actions
            .push(RemoteControlledWindowAction::ToggleCollapse(
                self.collapsed_to_pill,
            ));
    }

    /// Instant local kill-switch (`RemoteControlHub::disconnect_all()`) + emits
    /// `RemoteControlledWindowAction::DisconnectAll` so `AppShell` invokes server
    /// `/api/v1/remote/sessions/disconnect-all`.
    pub fn disconnect_all(&mut self) -> usize {
        let session_ids: Vec<String> = self
            .sessions
            .iter()
            .map(|s| s.session_id.clone())
            .collect();
        let disconnected_local_count = self.hub.disconnect_all();
        self.sessions.clear();
        self.pending_actions
            .push(RemoteControlledWindowAction::DisconnectAll {
                disconnected_local_count,
                session_ids,
            });
        disconnected_local_count
    }

    pub fn disconnect_session(&mut self, session_id: &str) -> bool {
        let disconnected_local = self.hub.disconnect_session(session_id);
        self.sessions.retain(|s| s.session_id != session_id);
        self.pending_actions
            .push(RemoteControlledWindowAction::DisconnectSession {
                session_id: session_id.to_string(),
                disconnected_local,
            });
        disconnected_local
    }

    fn platform_icon_for(session: &RemoteSessionDto) -> IconName {
        match session.controller_kind {
            RemoteControllerKind::AiMcp => IconName::BotRobot,
            RemoteControllerKind::Cli => IconName::TerminalMcp,
            _ => match session.mode {
                RemoteControlMode::Terminal => IconName::TerminalMcp,
                RemoteControlMode::FileTransfer => IconName::FileDoc,
                RemoteControlMode::AiControl => IconName::BotRobot,
                RemoteControlMode::Desktop => IconName::ComputerDesktop,
            },
        }
    }

    fn controller_subtitle(&self, session: &RemoteSessionDto) -> String {
        let mode_label = if self.language_id.starts_with("zh") {
            session.mode.label_zh()
        } else {
            session.mode.label_en()
        };
        let mut parts = vec![mode_label.to_string()];
        if let Some(tool) = session.active_tool.as_deref().filter(|t| !t.trim().is_empty()) {
            parts.push(format!("tool: {tool}"));
        }
        let relay = if self.language_id.starts_with("zh") {
            session.relay_mode.label_zh()
        } else {
            session.relay_mode.label_en()
        };
        if session.relay_mode != TailnetRelayMode::Offline {
            parts.push(relay.to_string());
        }
        if let Some(rtt) = session.rtt_ms {
            parts.push(format!("{rtt}ms"));
        }
        if let Some(fps) = session.fps {
            parts.push(format!("{fps}fps"));
        }
        parts.join(" · ")
    }

    fn render_collapsed_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = &self.language_id;
        let variant = self.banner_variant();
        let elapsed = self.earliest_elapsed_hms();
        let count = self.sessions.len();
        let banner_bg = rgb(variant.bg_hex());
        let border_col = rgb(variant.border_hex());

        div()
            .id("controlled-pill-bar")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3p5()
            .py_2()
            .rounded(px(999.0))
            .bg(banner_bg)
            .border_1()
            .border_color(border_col)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded(px(999.0))
                            .bg(rgb(0x4ADE80)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child(format!(
                                "{} {} ({})",
                                tr_str(lang, "本机被控中", "Controlled"),
                                elapsed,
                                count
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .id("pill-expand-btn")
                            .cursor_pointer()
                            .px_2p5()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(0xFFFFFF26))
                            .hover(|s| s.bg(rgba(0xFFFFFF40)))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xFFFFFF))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_collapse();
                                cx.notify();
                            }))
                            .child(tr_str(lang, "展开", "Expand")),
                    )
                    .child(
                        div()
                            .id("pill-disconnect-all-btn")
                            .cursor_pointer()
                            .px_2p5()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0xEF4444))
                            .hover(|s| s.bg(rgb(0xDC2626)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xFFFFFF))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.disconnect_all();
                                cx.notify();
                            }))
                            .child(tr_str(lang, "断开所有远控", "Disconnect All")),
                    ),
            )
    }
}

impl Render for RemoteControlledWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.collapsed_to_pill {
            return div()
                .id("remote-controlled-window-root-collapsed")
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .p_2()
                .child(self.render_collapsed_pill(cx))
                .into_any_element();
        }

        let palette = TdesktopThemePalette::build(self.theme_mode, 0x1E6BFF, 14, 12);
        let lang = &self.language_id;
        let variant = self.banner_variant();
        let elapsed = self.earliest_elapsed_hms();
        let count = self.sessions.len();
        let groups = group_controlled_sessions(&self.sessions);

        let header_title = if matches!(variant, ControlledBannerVariant::AiTeal) {
            format!("{} {}", tr_str(lang, "AI MCP 操控中", "AI MCP Active"), elapsed)
        } else if matches!(variant, ControlledBannerVariant::MixedIndigo) {
            format!(
                "{} {}",
                tr_str(lang, "本机被控中 (含 AI MCP)", "Controlled (+ AI MCP)"),
                elapsed
            )
        } else {
            format!("{} {}", tr_str(lang, "本机被控中", "Being Controlled"), elapsed)
        };

        let subtitle = if count == 0 {
            tr_str(
                lang,
                "当前暂无活动远控或 AI Agent 会话",
                "No active remote control or AI Agent sessions",
            )
            .to_string()
        } else if lang.starts_with("zh") {
            format!("当前有 {} 台设备正在控制此电脑", count)
        } else {
            format!("{} device(s) currently controlling this computer", count)
        };

        let mut root = div()
            .id("remote-controlled-window-root")
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(360.0))
            .max_w(px(480.0))
            .rounded_xl()
            .overflow_hidden()
            .bg(rgb(palette.card_bg))
            .border_1()
            .border_color(rgb(palette.border_fg))
            .shadow_xl();

        if let Some(handle) = &self.focus_handle {
            root = root.track_focus(handle);
        }

        // Top UU-style vibrancy header
        let header = div()
            .flex()
            .flex_col()
            .gap_3()
            .px_4()
            .py_4()
            .bg(rgb(variant.bg_hex()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(tg_icon(
                                        if matches!(variant, ControlledBannerVariant::AiTeal) {
                                            IconName::BotRobot
                                        } else {
                                            IconName::ScreenShare
                                        },
                                        18.0,
                                        0xFFFFFF,
                                    ))
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xFFFFFF))
                                            .child(header_title),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(0xFFFFFFDD))
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .id("controlled-collapse-btn")
                                    .cursor_pointer()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgba(0xFFFFFF24))
                                    .hover(|s| s.bg(rgba(0xFFFFFF3D)))
                                    .text_xs()
                                    .text_color(rgb(0xFFFFFF))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_collapse();
                                        cx.notify();
                                    }))
                                    .child(tr_str(lang, "收起为悬浮条", "Pill")),
                            )
                            .child(
                                div()
                                    .id("controlled-close-btn")
                                    .cursor_pointer()
                                    .p_1p5()
                                    .rounded_md()
                                    .bg(rgba(0xFFFFFF18))
                                    .hover(|s| s.bg(rgba(0xFFFFFF35)))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.should_close = true;
                                        this.pending_actions
                                            .push(RemoteControlledWindowAction::CloseRequested);
                                        window.remove_window();
                                        cx.notify();
                                    }))
                                    .child(tg_icon(IconName::Close, 14.0, 0xFFFFFF)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(0xFFFFFFCC))
                            .child(tr_str(
                                lang,
                                "本地用户随时可一键接管并立即中断远控与 MCP 操控",
                                "Local user can preempt or disconnect all remote/MCP control instantly",
                            )),
                    )
                    .child(
                        div()
                            .id("disconnect-all-remote-btn")
                            .cursor_pointer()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1p5()
                            .px_3()
                            .py_1p5()
                            .rounded_lg()
                            .bg(rgb(0xFFFFFF))
                            .hover(|s| s.bg(rgb(0xFEE2E2)))
                            .shadow_sm()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.disconnect_all();
                                cx.notify();
                            }))
                            .child(tg_icon(IconName::Hangup, 14.0, 0xDC2626))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xDC2626))
                                    .child(tr_str(lang, "断开所有远控", "Disconnect All")),
                            ),
                    ),
            );

        let body = if groups.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .py_8()
                .px_4()
                .child(tg_icon(
                    IconName::Lock,
                    28.0,
                    palette.text_secondary,
                ))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(palette.text_primary))
                        .child(tr_str(
                            lang,
                            "当前没有正在进行的远程控制",
                            "No active remote control sessions",
                        )),
                )
                .into_any_element()
        } else {
            let mut list = div()
                .id("controlled-session-groups-scroll")
                .flex()
                .flex_col()
                .gap_3()
                .p_3p5()
                .max_h(px(360.0))
                .overflow_y_scroll();

            for group in groups {
                let group_title = if lang.starts_with("zh") {
                    group.title_zh
                } else {
                    group.title_en
                };
                let mut section = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette.text_secondary))
                                    .child(format!("{} ({})", group_title, group.sessions.len())),
                            ),
                    );

                for sess in group.sessions {
                    let sess_id = sess.session_id.clone();
                    let elapsed_str = self.session_elapsed_hms(&sess);
                    let subtitle = self.controller_subtitle(&sess);
                    let icon = Self::platform_icon_for(&sess);
                    let accent_hex = match sess.controller_kind {
                        RemoteControllerKind::AiMcp => 0x14B8A6,
                        RemoteControllerKind::AssistGuest => 0xF59E0B,
                        RemoteControllerKind::OwnDevice => UU_HUMAN_BANNER_HEX,
                        RemoteControllerKind::Cli => 0x8B5CF6,
                    };
                    let platform_label = if sess.controller_platform.trim().is_empty() {
                        sess.controller_kind.as_str().to_string()
                    } else {
                        sess.controller_platform.clone()
                    };
                    let controller_title = if sess.controller_name.trim().is_empty() {
                        sess.controller_id.clone()
                    } else {
                        sess.controller_name.clone()
                    };

                    let card = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .px_3()
                        .py_2p5()
                        .rounded_lg()
                        .bg(rgb(palette.window_bg))
                        .border_1()
                        .border_color(rgb(palette.border_fg))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2p5()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .w(px(34.0))
                                        .h(px(34.0))
                                        .rounded_lg()
                                        .bg(rgba((accent_hex << 8) | 0x24))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(tg_icon(icon, 18.0, accent_hex)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap_1p5()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(rgb(palette.text_primary))
                                                        .truncate()
                                                        .child(controller_title),
                                                )
                                                .child(
                                                    div()
                                                        .px_1p5()
                                                        .py_0p5()
                                                        .rounded(px(4.0))
                                                        .bg(rgb(palette.hover_bg))
                                                        .text_xs()
                                                        .text_color(rgb(palette.text_secondary))
                                                        .child(platform_label),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(accent_hex))
                                                        .child(elapsed_str),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette.text_secondary))
                                                .truncate()
                                                .child(subtitle),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("disconnect-session-{sess_id}")))
                                .cursor_pointer()
                                .px_2p5()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0xEF444422))
                                .hover(|s| s.bg(rgba(0xEF44443D)))
                                .border_1()
                                .border_color(rgba(0xEF444455))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xF87171))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.disconnect_session(&sess_id);
                                    cx.notify();
                                }))
                                .child(tr_str(lang, "断开远控", "Disconnect")),
                        );

                    section = section.child(card);
                }

                list = list.child(section);
            }
            list.into_any_element()
        };

        root.child(header).child(body).into_any_element()
    }
}

/// Opens a detached floating GPUI window for the 1:1 Controlled Status Window ("本机被控中").
/// Strictly guarded in tests and headless environments (`cfg!(test)` / `GPUI_OFFLINE_TEST`).
pub fn open_controlled_status_window(
    hub: Arc<RemoteControlHub>,
    sessions: Vec<RemoteSessionDto>,
    language_id: impl Into<String>,
    theme_mode: TdesktopThemeMode,
    cx: &mut App,
) -> std::result::Result<Option<WindowHandle<RemoteControlledWindow>>, String> {
    if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
        return Ok(None);
    }
    let language_id = language_id.into();
    let bounds = Bounds::centered(None, size(px(430.0), px(480.0)), cx);
    let title = tr_str(&language_id, "本机被控中 - Fabushi", "Being Controlled - Fabushi")
        .to_string();
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from(title)),
                    ..Default::default()
                }),
                focus: false,
                show: true,
                kind: WindowKind::Floating,
                window_min_size: Some(size(px(360.0), px(120.0))),
                ..Default::default()
            },
            move |window, cx| {
                let _ = window;
                cx.new(|cx| {
                    let mut view = RemoteControlledWindow::new(hub.clone(), language_id.clone(), cx);
                    view.set_theme_mode(theme_mode);
                    view.set_sessions(sessions.clone());
                    view
                })
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(Some(handle))
}
