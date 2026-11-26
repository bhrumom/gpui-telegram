//! OpenBot 7 Desktop OS Surfaces (`OB-F19`):
//! `Main`, `ChatWindow`, `DynamicIsland`, `AgentDock`, `BrowserDock`, `CUA`, and `Recovery`.
//!
//! 1:1 GPUI `0.2.2` state machines, validators, physics/timing evaluators, and visual
//! renderers faithful to upstream `nightly-labs/openbot` (`main-window.ts`,
//! `DynamicIslandSurface.tsx`, `AgentDockSurface.tsx`, `BrowserDock.tsx`,
//! `ComputerUseHighlightSurface.tsx`, `ComputerUseAgentCursor.tsx`, `start-recovery-window.ts`).

use std::fmt;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{AvatarHue, AvatarMood};
use openbot_contracts::dynamic_island::{
    DEFAULT_NOTCH_WIDTH, DYNAMIC_ISLAND_DEFAULT_COMPACT_HEIGHT, DYNAMIC_ISLAND_WORKING_LIMIT,
    DynamicIslandMode, DynamicIslandPreference, DynamicIslandPresentation, DynamicIslandViewState,
    HEIGHT_PERCENT_MAX, HEIGHT_PERCENT_MIN, SIZE_PERCENT_STEP, WIDTH_PERCENT_MAX,
    WIDTH_PERCENT_MIN, dynamic_island_compact_height, interaction_locks_presentation,
};
use openbot_contracts::limits::utf16_len;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::components::bloub_avatar::{
    BundledPlushAssets, recovery_plush_svg, render_plush_seed_avatar,
};
use crate::theme::openbot_palette::{OpenBotColorScheme, OpenBotPalette};
use crate::theme::{IconName, tg_icon_rgba};

/// Canonical ordered names of all 7 Desktop OS Surfaces (`OB-F19`).
pub const SURFACE_NAMES: [&str; 7] = [
    "Main",
    "ChatWindow",
    "DynamicIsland",
    "AgentDock",
    "BrowserDock",
    "CUA",
    "Recovery",
];

/// Enumeration of all 7 OpenBot Desktop OS Surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DesktopOsSurfaceKind {
    Main,
    ChatWindow,
    DynamicIsland,
    AgentDock,
    BrowserDock,
    Cua,
    Recovery,
}

impl DesktopOsSurfaceKind {
    pub const ALL: [Self; 7] = [
        Self::Main,
        Self::ChatWindow,
        Self::DynamicIsland,
        Self::AgentDock,
        Self::BrowserDock,
        Self::Cua,
        Self::Recovery,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "Main",
            Self::ChatWindow => "ChatWindow",
            Self::DynamicIsland => "DynamicIsland",
            Self::AgentDock => "AgentDock",
            Self::BrowserDock => "BrowserDock",
            Self::Cua => "CUA",
            Self::Recovery => "Recovery",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "main" => Some(Self::Main),
            "chatwindow" | "chat-window" | "chat_window" => Some(Self::ChatWindow),
            "dynamicisland" | "dynamic-island" | "dynamic_island" => Some(Self::DynamicIsland),
            "agentdock" | "agent-dock" | "agent_dock" => Some(Self::AgentDock),
            "browserdock" | "browser-dock" | "browser_dock" => Some(Self::BrowserDock),
            "cua" | "computer-use" | "computer_use" => Some(Self::Cua),
            "recovery" => Some(Self::Recovery),
            _ => None,
        }
    }
}

/// Domain validation error returned by OS Surface evaluators & validators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsSurfaceValidationError {
    pub field: String,
    pub message: String,
}

impl OsSurfaceValidationError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for OsSurfaceValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for OsSurfaceValidationError {}

fn pal_rgba(palette: &OpenBotPalette, token: &str, fallback_hex: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback_hex))
}

// ============================================================================
// Surface 1 — MainWindowSurface (`src/main/main-window.ts`)
// ============================================================================

pub const MAIN_MIN_WIDTH_PX: u32 = 960;
pub const MAIN_MIN_HEIGHT_PX: u32 = 640;
pub const MAIN_BG_HEX: &str = "#0b0d0e";
pub const MAIN_AUTO_ZOOM_THRESHOLD_PX: u32 = 721;
pub const CLOSE_GUARD_BUTTONS: [&str; 3] = ["Keep Running", "Quit", "Cancel"];
pub const CMD_Q_HOLD_MS: u64 = 650;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CloseGuardChoice {
    KeepRunning,
    Quit,
    Cancel,
}

impl CloseGuardChoice {
    pub const fn label(self) -> &'static str {
        match self {
            Self::KeepRunning => "Keep Running",
            Self::Quit => "Quit",
            Self::Cancel => "Cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MainWindowState {
    pub width_px: u32,
    pub height_px: u32,
    pub working_agents_count: usize,
    pub attempt_close: bool,
    pub glass_enabled: bool,
    pub frameless_titlebar: bool,
    pub cmd_q_hold_ms: u64,
    pub cmd_q_hold_progress: f32,
    pub close_guard_selected: Option<CloseGuardChoice>,
}

impl Default for MainWindowState {
    fn default() -> Self {
        Self {
            width_px: 1280,
            height_px: 800,
            working_agents_count: 0,
            attempt_close: false,
            glass_enabled: false,
            frameless_titlebar: true,
            cmd_q_hold_ms: 0,
            cmd_q_hold_progress: 0.0,
            close_guard_selected: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MainWindowReport {
    pub min_width_px: u32,
    pub min_height_px: u32,
    pub bg_hex: String,
    pub width_px: u32,
    pub height_px: u32,
    pub auto_zoomed: bool,
    pub zoom_factor: f32,
    pub working_agents_count: usize,
    pub close_guard_prompted: bool,
    pub close_guard_buttons: Vec<String>,
    pub cmd_q_hold_progress: f32,
}

impl MainWindowState {
    pub fn new(width_px: u32, height_px: u32) -> Self {
        Self {
            width_px,
            height_px,
            ..Self::default()
        }
    }

    pub const fn auto_zoomed(&self) -> bool {
        self.width_px < MAIN_AUTO_ZOOM_THRESHOLD_PX
    }

    pub fn effective_zoom_factor(&self) -> f32 {
        if self.auto_zoomed() {
            (self.width_px.max(320) as f32 / MAIN_MIN_WIDTH_PX as f32).clamp(0.55, 1.0)
        } else {
            1.0
        }
    }

    pub const fn clamped_size(&self) -> (u32, u32) {
        let w = if self.width_px < MAIN_MIN_WIDTH_PX {
            MAIN_MIN_WIDTH_PX
        } else {
            self.width_px
        };
        let h = if self.height_px < MAIN_MIN_HEIGHT_PX {
            MAIN_MIN_HEIGHT_PX
        } else {
            self.height_px
        };
        (w, h)
    }

    pub const fn close_guard_prompted(&self) -> bool {
        self.attempt_close && self.working_agents_count > 0
    }

    pub fn update_cmd_q_hold(&mut self, elapsed_ms: u64) -> f32 {
        self.cmd_q_hold_ms = elapsed_ms;
        self.cmd_q_hold_progress = (elapsed_ms as f32 / CMD_Q_HOLD_MS as f32).clamp(0.0, 1.0);
        self.cmd_q_hold_progress
    }

    pub fn should_quit_from_hold(&self) -> bool {
        self.cmd_q_hold_progress >= 1.0
    }

    pub fn resolve_close_guard(&mut self, choice: CloseGuardChoice) {
        self.close_guard_selected = Some(choice);
        if matches!(choice, CloseGuardChoice::Cancel | CloseGuardChoice::KeepRunning) {
            self.attempt_close = false;
        }
    }

    pub fn report(&self) -> MainWindowReport {
        MainWindowReport {
            min_width_px: MAIN_MIN_WIDTH_PX,
            min_height_px: MAIN_MIN_HEIGHT_PX,
            bg_hex: MAIN_BG_HEX.to_string(),
            width_px: self.width_px,
            height_px: self.height_px,
            auto_zoomed: self.auto_zoomed(),
            zoom_factor: self.effective_zoom_factor(),
            working_agents_count: self.working_agents_count,
            close_guard_prompted: self.close_guard_prompted(),
            close_guard_buttons: CLOSE_GUARD_BUTTONS.iter().map(|s| (*s).to_string()).collect(),
            cmd_q_hold_progress: self.cmd_q_hold_progress,
        }
    }
}

/// Renders Surface 1's Close-While-Working Guard dialog (`Keep Running` / `Quit` / `Cancel`)
/// plus optional `Cmd+Q` hold progress bar.
pub fn render_close_guard_dialog(state: &MainWindowState, palette: &OpenBotPalette) -> Div {
    let sheet_bg = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let border = pal_rgba(palette, "--openbot-dialog-ring", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x44181c);
    let warning = pal_rgba(palette, "--openbot-warning", 0xff9412);
    let warning_soft = pal_rgba(palette, "--openbot-warning-soft", 0x3b2a14);
    let control_bg = pal_rgba(palette, "--openbot-bg-control", 0x2a2a2a);
    let control_hover = pal_rgba(palette, "--openbot-bg-control-hover", 0x343434);

    let active_count = state.working_agents_count;
    let noun = if active_count == 1 { "agent is" } else { "agents are" };
    let hold_pct = (state.cmd_q_hold_progress.clamp(0.0, 1.0) * 100.0).round() as u32;

    let mut buttons_row = div().flex().flex_row().items_center().justify_end().gap(px(8.0));
    for (idx, label) in CLOSE_GUARD_BUTTONS.iter().enumerate() {
        let (btn_bg, btn_fg, btn_border) = match *label {
            "Keep Running" => (accent, text_on_accent, accent),
            "Quit" => (danger_soft, danger, danger),
            _ => (control_bg, text_primary, border),
        };
        buttons_row = buttons_row.child(
            div()
                .id(("close-guard-btn", idx))
                .px(px(14.0))
                .py(px(7.0))
                .rounded(px(10.0))
                .bg(btn_bg)
                .border_1()
                .border_color(btn_border)
                .text_color(btn_fg)
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(move |s| s.bg(control_hover))
                .child(*label),
        );
    }

    div()
        .w(px(420.0))
        .p(px(20.0))
        .rounded(px(20.0))
        .bg(sheet_bg)
        .border_1()
        .border_color(border)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(34.0))
                        .h(px(34.0))
                        .rounded(px(10.0))
                        .bg(warning_soft)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon_rgba(IconName::BotRobot, 18.0, warning)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child(format!("{active_count} {noun} still working")),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_muted)
                                .child("Keep OpenBot running in the background or stop active turns."),
                        ),
                ),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(text_secondary)
                .child(if state.auto_zoomed() {
                    format!(
                        "Compact tiling viewport active ({}px < {}px auto-zoom threshold).",
                        state.width_px, MAIN_AUTO_ZOOM_THRESHOLD_PX
                    )
                } else {
                    "Quitting now interrupts active tasks and unsaved browser sessions.".to_string()
                }),
        )
        .when(state.cmd_q_hold_progress > 0.0, |el| {
            let fill_w = 378.0 * state.cmd_q_hold_progress.clamp(0.0, 1.0);
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted)
                            .child(format!("Hold Cmd+Q to quit ({hold_pct}%)")),
                    )
                    .child(
                        div()
                            .w(px(378.0))
                            .h(px(4.0))
                            .rounded_full()
                            .bg(control_bg)
                            .overflow_hidden()
                            .child(div().w(px(fill_w)).h(px(4.0)).rounded_full().bg(danger)),
                    ),
            )
        })
        .child(buttons_row)
}

// ============================================================================
// Surface 2 — ChatWindowSurface (`main-window.ts`, `WorkspaceShell.tsx`)
// ============================================================================

pub const CHAT_WINDOW_MIN_WIDTH_PX: u32 = 380;
pub const CHAT_WINDOW_MIN_HEIGHT_PX: u32 = 480;
pub const CHAT_WINDOW_HIDDEN_SETTINGS_TABS: [&str; 2] = ["dynamic-island", "agent-dock"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatWindowState {
    pub agent_id: String,
    pub agent_name: String,
    pub avatar_seed: String,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub width_px: u32,
    pub height_px: u32,
    pub rails_hidden: bool,
    pub sidebar_hidden: bool,
    pub side_panel_open: bool,
    pub target_agent_deleted: bool,
    pub auto_closed: bool,
    pub mood: AvatarMood,
}

impl Default for ChatWindowState {
    fn default() -> Self {
        Self::new("agent-1")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatWindowReport {
    pub agent_id: String,
    pub min_width_px: u32,
    pub min_height_px: u32,
    pub effective_width_px: u32,
    pub effective_height_px: u32,
    pub rails_hidden: bool,
    pub sidebar_hidden: bool,
    pub hidden_settings_tabs: Vec<String>,
    pub auto_closed: bool,
}

impl ChatWindowState {
    pub fn new(agent_id: impl Into<String>) -> Self {
        let id = agent_id.into();
        let seed = if id.is_empty() {
            "agent-1".to_string()
        } else {
            id.clone()
        };
        Self {
            agent_id: id.clone(),
            agent_name: id,
            avatar_seed: seed,
            avatar_hue: None,
            avatar_color: None,
            width_px: 520,
            height_px: 720,
            rails_hidden: true,
            sidebar_hidden: true,
            side_panel_open: false,
            target_agent_deleted: false,
            auto_closed: false,
            mood: AvatarMood::Idle,
        }
    }

    pub const fn effective_width_px(&self) -> u32 {
        if self.width_px < CHAT_WINDOW_MIN_WIDTH_PX {
            CHAT_WINDOW_MIN_WIDTH_PX
        } else {
            self.width_px
        }
    }

    pub const fn effective_height_px(&self) -> u32 {
        if self.height_px < CHAT_WINDOW_MIN_HEIGHT_PX {
            CHAT_WINDOW_MIN_HEIGHT_PX
        } else {
            self.height_px
        }
    }

    pub fn on_agent_deleted(&mut self, deleted_agent_id: &str) -> bool {
        if deleted_agent_id == self.agent_id {
            self.target_agent_deleted = true;
            self.auto_closed = true;
            true
        } else {
            false
        }
    }

    pub fn set_target_deleted(&mut self, deleted: bool) {
        self.target_agent_deleted = deleted;
        self.auto_closed = deleted;
    }

    pub fn is_settings_tab_visible(&self, tab_id: &str) -> bool {
        !CHAT_WINDOW_HIDDEN_SETTINGS_TABS.contains(&tab_id)
    }

    pub const fn hidden_settings_tabs(&self) -> &'static [&'static str] {
        &CHAT_WINDOW_HIDDEN_SETTINGS_TABS
    }

    pub fn report(&self) -> ChatWindowReport {
        ChatWindowReport {
            agent_id: self.agent_id.clone(),
            min_width_px: CHAT_WINDOW_MIN_WIDTH_PX,
            min_height_px: CHAT_WINDOW_MIN_HEIGHT_PX,
            effective_width_px: self.effective_width_px(),
            effective_height_px: self.effective_height_px(),
            rails_hidden: self.rails_hidden,
            sidebar_hidden: self.sidebar_hidden,
            hidden_settings_tabs: CHAT_WINDOW_HIDDEN_SETTINGS_TABS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            auto_closed: self.auto_closed,
        }
    }
}

/// Renders Surface 2's detached 1-on-1 `ChatWindow` chrome (`min 380x480`, rails & sidebar hidden).
pub fn render_detached_chat_window_chrome(
    state: &ChatWindowState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x44181c);
    let control_bg = pal_rgba(palette, "--openbot-bg-control", 0x212121);

    let title = if state.agent_name.trim().is_empty() {
        state.agent_id.clone()
    } else {
        state.agent_name.clone()
    };

    div()
        .min_w(px(CHAT_WINDOW_MIN_WIDTH_PX as f32))
        .min_h(px(CHAT_WINDOW_MIN_HEIGHT_PX as f32))
        .w(px(state.effective_width_px() as f32))
        .h(px(state.effective_height_px() as f32))
        .bg(bg_canvas)
        .border_1()
        .border_color(border)
        .rounded(px(14.0))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(46.0))
                .px(px(14.0))
                .bg(bg_surface)
                .border_b_1()
                .border_color(border)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(render_plush_seed_avatar(
                            &state.avatar_seed,
                            state.avatar_hue,
                            state.avatar_color.as_deref(),
                            state.mood,
                            now_sec,
                            false,
                            22.0,
                        ))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .truncate()
                                .child(title),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(6.0))
                                .bg(control_bg)
                                .text_size(px(10.0))
                                .text_color(text_muted)
                                .child("1:1 Window"),
                        ),
                )
                .child(
                    div()
                        .id("chat-window-side-panel-toggle")
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |s| s.bg(control_bg))
                        .child(tg_icon_rgba(IconName::SidebarPanel, 15.0, text_muted)),
                ),
        )
        .when(state.auto_closed, |el| {
            el.child(
                div()
                    .mx(px(14.0))
                    .mt(px(12.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(danger_soft)
                    .border_1()
                    .border_color(danger)
                    .text_size(px(12.0))
                    .text_color(danger)
                    .child("Agent deleted — detached chat window auto-closed."),
            )
        })
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .p(px(14.0))
                .flex()
                .flex_col()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_muted)
                        .child("Detached conversation surface (PageRail, ServerRail & Sidebar hidden)."),
                ),
        )
}

// ============================================================================
// Surface 3 — DynamicIslandSurface (`DynamicIslandSurface.tsx`, `spring-motion.ts`)
// ============================================================================

pub const NOTCH_WIDTH_PX: u32 = DEFAULT_NOTCH_WIDTH;
pub const NOTCH_HEIGHT_PX: u32 = DYNAMIC_ISLAND_DEFAULT_COMPACT_HEIGHT;
pub const AVATAR_SIZE_PX: f32 = 20.0;
pub const AVATAR_OVERLAP_PX: f32 = -6.0;
pub const STATUS_COMPACT_NOTCH_WIDTH_PX: u32 = 412;
pub const STATUS_COMPACT_ISLAND_WIDTH_PX: u32 = 280;
pub const STATUS_COMPACT_MIN_NOTCH_WIDTH_PX: u32 = 360;
pub const STATUS_COMPACT_MIN_ISLAND_WIDTH_PX: u32 = 212;
pub const IDLE_COMPACT_MIN_ISLAND_WIDTH_PX: u32 = 74;
pub const SPRING_OPEN: [f64; 2] = [0.5_f64, 0.8_f64];
pub const SPRING_CLOSE: [f64; 2] = [0.45_f64, 1.0_f64];
pub const SPRING_CONTENT: [f64; 2] = [0.34_f64, 0.88_f64];
pub const IDLE_GREETING_INTERVAL_MS: u64 = 8_000;
pub const IDLE_GREETING_FRONT_EMOJIS: [&str; 4] = ["👋", "😊", "🙌", "✨"];
pub const IDLE_GREETING_BACK_EMOJIS: [&str; 4] = ["🫶", "😄", "👏", "✨"];
pub const INLINE_REPLY_MAX_CHARS: usize = 20_000;
pub const HOVER_LEADING_SCALE: f32 = 1.22;
pub const HOVER_TRAILING_SCALE: f32 = 1.08;
pub const HOVER_OUTWARD_TRANSLATE_X: f32 = 10.0;
pub const HOVER_TRANSLATE_Y: f32 = 4.0;
pub const MOTION_ROOM_PX: u32 = 2;

/// Strict validator for `DynamicIslandPreference` dimension ranges and step-5 increments.
#[derive(Debug, Clone, Copy, Default)]
pub struct DynamicIslandPreferenceValidator;

impl DynamicIslandPreferenceValidator {
    pub fn validate_dimensions(width_percent: u32, height_percent: u32) -> Result<(), String> {
        if !(WIDTH_PERCENT_MIN..=WIDTH_PERCENT_MAX).contains(&width_percent) {
            return Err(format!(
                "widthPercent {width_percent} is out of range (must be between {WIDTH_PERCENT_MIN} and {WIDTH_PERCENT_MAX} in step {SIZE_PERCENT_STEP})"
            ));
        }
        if width_percent % SIZE_PERCENT_STEP != 0 {
            return Err(format!(
                "widthPercent {width_percent} has invalid step: must be a multiple of 5"
            ));
        }
        if !(HEIGHT_PERCENT_MIN..=HEIGHT_PERCENT_MAX).contains(&height_percent) {
            return Err(format!(
                "heightPercent {height_percent} is out of range (must be between {HEIGHT_PERCENT_MIN} and {HEIGHT_PERCENT_MAX} in step {SIZE_PERCENT_STEP})"
            ));
        }
        if height_percent % SIZE_PERCENT_STEP != 0 {
            return Err(format!(
                "heightPercent {height_percent} has invalid step: must be a multiple of 5"
            ));
        }
        Ok(())
    }

    pub fn validate(pref: &DynamicIslandPreference) -> Result<(), String> {
        Self::validate_dimensions(pref.width_percent, pref.height_percent)?;
        if !pref.is_valid() {
            return Err(
                "invalid DynamicIslandPreference: server visibility exceeds limits or contains invalid server identifier"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// Validates inline reply text submitted from the Dynamic Island composer.
pub fn validate_inline_reply(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("inline reply cannot be empty or blank".to_string());
    }
    let char_len = text.chars().count();
    let code_units = utf16_len(text);
    if char_len > INLINE_REPLY_MAX_CHARS || code_units > INLINE_REPLY_MAX_CHARS {
        return Err(format!(
            "inline reply length ({char_len}) exceeds 20000 character limit"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DynamicIslandDisplayMode {
    Notch,
    Island,
}

fn default_local_server_id() -> String {
    "local".to_string()
}

fn deserialize_opt_string_or_int<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<Value>::deserialize(deserializer)?;
    match val {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(Value::Number(n)) => Ok(Some(n.to_string())),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected string or integer request id, got {other}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicIslandPresentationItem {
    pub mode: DynamicIslandMode,
    #[serde(default = "default_local_server_id", alias = "server_id")]
    pub server_id: String,
    #[serde(
        default,
        alias = "request_id",
        deserialize_with = "deserialize_opt_string_or_int"
    )]
    pub request_id: Option<String>,
    #[serde(
        default,
        alias = "resolved_request_id",
        deserialize_with = "deserialize_opt_string_or_int"
    )]
    pub resolved_request_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, alias = "agent_ids")]
    pub agent_ids: Vec<String>,
}

impl Default for DynamicIslandPresentationItem {
    fn default() -> Self {
        Self {
            mode: DynamicIslandMode::Idle,
            server_id: default_local_server_id(),
            request_id: None,
            resolved_request_id: None,
            title: String::new(),
            body: String::new(),
            agent_ids: Vec::new(),
        }
    }
}

impl DynamicIslandPresentationItem {
    pub const fn is_critical(&self) -> bool {
        self.mode.is_critical()
    }

    pub fn identity(&self) -> String {
        let sid = if self.server_id.is_empty() {
            "local"
        } else {
            self.server_id.as_str()
        };
        let mode = self.mode.as_str();
        match self.request_id.as_deref() {
            Some(req) if !req.is_empty() => format!("{sid}:{mode}:{req}"),
            _ if !self.title.is_empty() => format!("{sid}:{mode}:{}", self.title),
            _ => format!("{sid}:{mode}"),
        }
    }
}

/// Exact contract priority (`0..=6` aligned with `openbot_contracts::dynamic_island`).
pub const fn contract_priority(mode: DynamicIslandMode) -> u8 {
    mode.priority()
}

/// Compact probe priority (`question`/`approval` = 0, `message` = 1, `working` = 2, `idle` = 3).
pub const fn compact_probe_priority(mode: DynamicIslandMode) -> u8 {
    match mode {
        DynamicIslandMode::Question
        | DynamicIslandMode::Approval
        | DynamicIslandMode::Takeover
        | DynamicIslandMode::Failed => 0,
        DynamicIslandMode::Message => 1,
        DynamicIslandMode::Working => 2,
        DynamicIslandMode::Idle => 3,
    }
}

/// Hover-exit collapses the expanded island unless keyboard focus is inside an active text field.
pub const fn should_collapse_on_hover_exit(
    keyboard_inside: bool,
    text_field_focused: bool,
) -> bool {
    !(keyboard_inside && text_field_focused)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicIslandTransitionResult {
    pub locked: bool,
    pub critical_lock: bool,
    pub active_mode: DynamicIslandMode,
    pub queued_mode: Option<DynamicIslandMode>,
    pub held_request_ended: bool,
}

/// Exact `interactionLocksPresentation` state transition evaluator.
pub fn evaluate_presentation_transition(
    current: &DynamicIslandPresentationItem,
    next: &DynamicIslandPresentationItem,
    interacting: bool,
    typing: bool,
) -> DynamicIslandTransitionResult {
    let resolved_matches_current = current
        .request_id
        .as_deref()
        .zip(next.resolved_request_id.as_deref())
        .is_some_and(|(cur_id, res_id)| !cur_id.is_empty() && cur_id == res_id);

    let different_identity = current.identity() != next.identity();
    let should_lock = interacting
        && (typing || current.is_critical())
        && !resolved_matches_current
        && different_identity;

    if should_lock {
        DynamicIslandTransitionResult {
            locked: true,
            critical_lock: true,
            active_mode: current.mode,
            queued_mode: Some(next.mode),
            held_request_ended: false,
        }
    } else {
        let ended = resolved_matches_current
            || (current.is_critical() && next.mode.priority() > current.mode.priority());
        DynamicIslandTransitionResult {
            locked: false,
            critical_lock: false,
            active_mode: next.mode,
            queued_mode: None,
            held_request_ended: ended,
        }
    }
}

/// Bridges full `openbot_contracts::dynamic_island::DynamicIslandPresentation` transitions
/// directly through `interaction_locks_presentation`.
pub fn evaluate_contract_presentation_transition(
    current: &DynamicIslandPresentation,
    next: &DynamicIslandPresentation,
    interacting: bool,
    typing: bool,
) -> DynamicIslandTransitionResult {
    let locked = interaction_locks_presentation(current, next, interacting, typing);
    if locked {
        DynamicIslandTransitionResult {
            locked: true,
            critical_lock: true,
            active_mode: current.mode(),
            queued_mode: Some(next.mode()),
            held_request_ended: false,
        }
    } else {
        DynamicIslandTransitionResult {
            locked: false,
            critical_lock: false,
            active_mode: next.mode(),
            queued_mode: None,
            held_request_ended: current.mode().is_critical()
                && next.mode().priority() > current.mode().priority(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicIslandState {
    pub display_mode: DynamicIslandDisplayMode,
    pub view_state: DynamicIslandViewState,
    pub preference: DynamicIslandPreference,
    pub notch_width_px: u32,
    pub notch_height_px: u32,
    pub current: DynamicIslandPresentationItem,
    pub queued: Option<DynamicIslandPresentationItem>,
    pub interacting: bool,
    pub typing: bool,
    pub critical_lock: bool,
    pub inline_reply_draft: String,
    pub reduced_motion: bool,
    pub idle_elapsed_ms: u64,
}

impl Default for DynamicIslandState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicIslandReport {
    pub width_px: u32,
    pub height_px: u32,
    pub compact_height_px: u32,
    pub avatar_size_px: f32,
    pub avatar_overlap_px: f32,
    pub priority: u8,
    pub contract_priority: u8,
    pub spring_open: [f64; 2],
    pub spring_close: [f64; 2],
    pub spring_content: [f64; 2],
    pub mode: String,
    pub queued_mode: Option<String>,
    pub locked: bool,
    pub critical_lock: bool,
}

impl DynamicIslandState {
    pub fn new() -> Self {
        Self {
            display_mode: DynamicIslandDisplayMode::Notch,
            view_state: DynamicIslandViewState::Compact,
            preference: DynamicIslandPreference::default(),
            notch_width_px: NOTCH_WIDTH_PX,
            notch_height_px: NOTCH_HEIGHT_PX,
            current: DynamicIslandPresentationItem::default(),
            queued: None,
            interacting: false,
            typing: false,
            critical_lock: false,
            inline_reply_draft: String::new(),
            reduced_motion: false,
            idle_elapsed_ms: 0,
        }
    }

    pub fn compact_height_px(&self) -> u32 {
        dynamic_island_compact_height(Some(self.notch_height_px), self.preference.height_percent)
    }

    pub fn compact_width_px(&self) -> u32 {
        let base = match (self.display_mode, self.current.mode) {
            (DynamicIslandDisplayMode::Notch, DynamicIslandMode::Idle) => self.notch_width_px,
            (DynamicIslandDisplayMode::Notch, _) => STATUS_COMPACT_NOTCH_WIDTH_PX,
            (DynamicIslandDisplayMode::Island, DynamicIslandMode::Idle) => {
                IDLE_COMPACT_MIN_ISLAND_WIDTH_PX
            }
            (DynamicIslandDisplayMode::Island, _) => STATUS_COMPACT_ISLAND_WIDTH_PX,
        };
        let scaled = ((base as f64 * self.preference.width_percent as f64) / 100.0).round() as u32;
        match (self.display_mode, self.current.mode) {
            (DynamicIslandDisplayMode::Notch, DynamicIslandMode::Idle) => {
                scaled.max(self.notch_width_px)
            }
            (DynamicIslandDisplayMode::Notch, _) => scaled.max(STATUS_COMPACT_MIN_NOTCH_WIDTH_PX),
            (DynamicIslandDisplayMode::Island, DynamicIslandMode::Idle) => {
                scaled.max(IDLE_COMPACT_MIN_ISLAND_WIDTH_PX)
            }
            (DynamicIslandDisplayMode::Island, _) => scaled.max(STATUS_COMPACT_MIN_ISLAND_WIDTH_PX),
        }
    }

    pub fn idle_greeting(&self) -> (&'static str, &'static str) {
        let idx = ((self.idle_elapsed_ms / IDLE_GREETING_INTERVAL_MS) as usize)
            % IDLE_GREETING_FRONT_EMOJIS.len();
        (IDLE_GREETING_FRONT_EMOJIS[idx], IDLE_GREETING_BACK_EMOJIS[idx])
    }

    pub fn apply_incoming(
        &mut self,
        next: DynamicIslandPresentationItem,
    ) -> DynamicIslandTransitionResult {
        let tr = evaluate_presentation_transition(&self.current, &next, self.interacting, self.typing);
        self.critical_lock = tr.critical_lock;
        if tr.locked {
            self.queued = Some(next);
            self.view_state = DynamicIslandViewState::CriticalPromptLock;
        } else {
            self.current = next;
            self.queued = None;
            if self.view_state == DynamicIslandViewState::CriticalPromptLock {
                self.view_state = DynamicIslandViewState::Expanded;
            }
        }
        tr
    }

    pub fn submit_inline_reply(&mut self, text: &str) -> Result<String, String> {
        validate_inline_reply(text)?;
        let trimmed = text.trim().to_string();
        self.inline_reply_draft.clear();
        Ok(trimmed)
    }

    pub fn set_preference(&mut self, pref: DynamicIslandPreference) -> Result<(), String> {
        DynamicIslandPreferenceValidator::validate(&pref)?;
        self.preference = pref;
        Ok(())
    }

    pub fn report(&self) -> DynamicIslandReport {
        DynamicIslandReport {
            width_px: self.notch_width_px,
            height_px: self.notch_height_px,
            compact_height_px: self.compact_height_px(),
            avatar_size_px: AVATAR_SIZE_PX,
            avatar_overlap_px: AVATAR_OVERLAP_PX,
            priority: compact_probe_priority(self.current.mode),
            contract_priority: contract_priority(self.current.mode),
            spring_open: SPRING_OPEN,
            spring_close: SPRING_CLOSE,
            spring_content: SPRING_CONTENT,
            mode: self.current.mode.as_str().to_string(),
            queued_mode: self.queued.as_ref().map(|q| q.mode.as_str().to_string()),
            locked: self.critical_lock,
            critical_lock: self.critical_lock,
        }
    }
}

fn island_mood_for_mode(mode: DynamicIslandMode) -> AvatarMood {
    match mode {
        DynamicIslandMode::Working => AvatarMood::Working,
        DynamicIslandMode::Question | DynamicIslandMode::Approval | DynamicIslandMode::Takeover => {
            AvatarMood::Waiting
        }
        DynamicIslandMode::Failed => AvatarMood::Failed,
        DynamicIslandMode::Message => AvatarMood::Responded,
        DynamicIslandMode::Idle => AvatarMood::Idle,
    }
}

/// Renders Surface 3's Dynamic Island / MacBook Notch (`192x32` compact notch <-> expanded morph card,
/// `20x20` `-6px` overlapping `render_plush_seed_avatar` stack, inline reply composer, approval/question
/// quick actions, and critical lock badge).
pub fn render_dynamic_island(
    state: &DynamicIslandState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let island_bg = rgb(0x0b0d0e);
    let border = pal_rgba(palette, "--openbot-dialog-ring", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let warning = pal_rgba(palette, "--openbot-warning", 0xff9412);
    let warning_soft = pal_rgba(palette, "--openbot-warning-soft", 0x3b2a14);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x44181c);
    let success = pal_rgba(palette, "--openbot-success", 0x31cf76);
    let control_bg = pal_rgba(palette, "--openbot-bg-control", 0x212121);

    let mood = island_mood_for_mode(state.current.mode);
    let seeds: Vec<&str> = if state.current.agent_ids.is_empty() {
        vec!["openbot-notch"]
    } else {
        state
            .current
            .agent_ids
            .iter()
            .take(DYNAMIC_ISLAND_WORKING_LIMIT)
            .map(String::as_str)
            .collect()
    };

    let mut avatar_stack = div().flex().flex_row().items_center();
    for (idx, seed) in seeds.iter().enumerate() {
        avatar_stack = avatar_stack.child(
            div()
                .when(idx > 0, |el| el.ml(px(AVATAR_OVERLAP_PX)))
                .child(render_plush_seed_avatar(
                    seed,
                    None,
                    None,
                    mood,
                    now_sec,
                    state.reduced_motion,
                    AVATAR_SIZE_PX,
                )),
        );
    }

    let expanded = state.view_state != DynamicIslandViewState::Compact;
    let status_accent = match state.current.mode {
        DynamicIslandMode::Approval | DynamicIslandMode::Question | DynamicIslandMode::Takeover => {
            warning
        }
        DynamicIslandMode::Failed => danger,
        DynamicIslandMode::Working => accent,
        DynamicIslandMode::Message => success,
        DynamicIslandMode::Idle => text_muted,
    };

    if !expanded {
        let compact_w = state.compact_width_px() as f32;
        let compact_h = state.compact_height_px() as f32;
        let (front_emoji, _) = state.idle_greeting();
        let compact_label = if !state.current.title.is_empty() {
            state.current.title.clone()
        } else if state.current.mode == DynamicIslandMode::Idle {
            format!("{front_emoji} OpenBot")
        } else {
            format!("OpenBot · {}", state.current.mode.as_str())
        };
        return div()
            .w(px(compact_w))
            .h(px(compact_h))
            .px(px(12.0))
            .rounded_full()
            .bg(island_bg)
            .border_1()
            .border_color(border)
            .shadow_md()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(avatar_stack)
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_primary)
                    .truncate()
                    .child(compact_label),
            )
            .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(status_accent));
    }

    let heading = if state.current.title.is_empty() {
        match state.current.mode {
            DynamicIslandMode::Question => "Agent has a question".to_string(),
            DynamicIslandMode::Approval => "Approval required".to_string(),
            DynamicIslandMode::Takeover => "Browser takeover needed".to_string(),
            DynamicIslandMode::Failed => "Agent turn failed".to_string(),
            DynamicIslandMode::Working => "Working on task".to_string(),
            DynamicIslandMode::Message => "New agent message".to_string(),
            DynamicIslandMode::Idle => "OpenBot Ready".to_string(),
        }
    } else {
        state.current.title.clone()
    };

    let body_text = if state.current.body.is_empty() {
        format!("Priority {} · {}", state.current.mode.priority(), state.current.mode.as_str())
    } else {
        state.current.body.clone()
    };

    let mut card = div()
        .w(px(384.0))
        .p(px(14.0))
        .rounded(px(22.0))
        .bg(island_bg)
        .border_1()
        .border_color(if state.critical_lock { warning } else { border })
        .shadow_lg()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(avatar_stack)
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .truncate()
                                .child(heading),
                        ),
                )
                .when(state.critical_lock, |el| {
                    let lock_label = match state.queued.as_ref() {
                        Some(q) => format!("Locked · queued {}", q.mode.as_str()),
                        None => "Locked".to_string(),
                    };
                    el.child(
                        div()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded_full()
                            .bg(warning_soft)
                            .border_1()
                            .border_color(warning)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(tg_icon_rgba(IconName::Lock, 11.0, warning))
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(warning)
                                    .child(lock_label),
                            ),
                    )
                }),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(text_secondary)
                .child(body_text),
        );

    if matches!(
        state.current.mode,
        DynamicIslandMode::Message | DynamicIslandMode::Question
    ) {
        let placeholder = if state.inline_reply_draft.is_empty() {
            "Reply inline...".to_string()
        } else {
            state.inline_reply_draft.clone()
        };
        card = card.child(
            div()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(12.0))
                .bg(control_bg)
                .border_1()
                .border_color(border)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(if state.inline_reply_draft.is_empty() {
                            text_muted
                        } else {
                            text_primary
                        })
                        .truncate()
                        .child(placeholder),
                )
                .child(
                    div()
                        .id("dynamic-island-send-reply")
                        .w(px(24.0))
                        .h(px(24.0))
                        .rounded_full()
                        .bg(accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(tg_icon_rgba(IconName::SendPlane, 12.0, text_on_accent)),
                ),
        );
    }

    if matches!(
        state.current.mode,
        DynamicIslandMode::Approval | DynamicIslandMode::Question | DynamicIslandMode::Takeover
    ) {
        let primary_label = match state.current.mode {
            DynamicIslandMode::Approval => "Approve",
            DynamicIslandMode::Question => "Answer",
            _ => "Take Over",
        };
        let secondary_label = match state.current.mode {
            DynamicIslandMode::Approval => "Decline",
            _ => "Open Chat",
        };
        card = card.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(
                    div()
                        .id("dynamic-island-secondary-action")
                        .px(px(12.0))
                        .py(px(5.0))
                        .rounded(px(9.0))
                        .bg(if state.current.mode == DynamicIslandMode::Approval {
                            danger_soft
                        } else {
                            control_bg
                        })
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if state.current.mode == DynamicIslandMode::Approval {
                            danger
                        } else {
                            text_primary
                        })
                        .cursor_pointer()
                        .child(secondary_label),
                )
                .child(
                    div()
                        .id("dynamic-island-primary-action")
                        .px(px(12.0))
                        .py(px(5.0))
                        .rounded(px(9.0))
                        .bg(accent)
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_on_accent)
                        .cursor_pointer()
                        .child(primary_label),
                ),
        );
    }

    card
}

// ============================================================================
// Surface 4 — AgentDockSurface (`AgentDockSurface.tsx`, `AgentDockEdgeStrip.tsx`)
// ============================================================================

pub const AGENT_DOCK_MAX_SEATS: usize = 24;
pub const AGENT_DOCK_TUCK_DELAY_MS: u64 = 900;
pub const AGENT_DOCK_STRIP_PX: u32 = 4;
pub const AGENT_DOCK_RENDERED_STRIP_PX: u32 = 6;
pub const AGENT_DOCK_STRIP_REST_RATIO: f32 = 0.35;
pub const AGENT_DOCK_STRIP_FEEL_PX: u32 = 160;
pub const AGENT_DOCK_WIDEN_WAIT_MS: u64 = 250;
pub const AGENT_DOCK_EXTENT_SETTLE_MS: u64 = 700;
pub const AGENT_DOCK_EXTENT_SLACK_PX: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentDockPlacement {
    Left,
    Right,
    InWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentDockSeatKind {
    Agent,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDockSeat {
    pub id: String,
    pub name: String,
    pub kind: AgentDockSeatKind,
    pub avatar_seed: String,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub mood: AvatarMood,
    pub unread_count: u32,
    pub pinned: bool,
}

impl AgentDockSeat {
    pub fn agent(id: impl Into<String>, name: impl Into<String>) -> Self {
        let id_str = id.into();
        let name_str = name.into();
        Self {
            avatar_seed: id_str.clone(),
            id: id_str,
            name: name_str,
            kind: AgentDockSeatKind::Agent,
            avatar_hue: None,
            avatar_color: None,
            mood: AvatarMood::Idle,
            unread_count: 0,
            pinned: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDockState {
    pub placement: AgentDockPlacement,
    pub seats: Vec<AgentDockSeat>,
    pub idle_ms: u64,
    pub pointer_in: bool,
    pub near: bool,
    pub popover_open: bool,
    pub dragging: bool,
    pub shelf_shown: bool,
    pub menu_open: bool,
    pub active_chat_seat_id: Option<String>,
    pub reduced_motion: bool,
}

impl Default for AgentDockState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDockReport {
    pub max_seats: usize,
    pub seats_count: usize,
    pub tuck_delay_ms: u64,
    pub strip_px: u32,
    pub rendered_strip_px: u32,
    pub strip_feel_px: u32,
    pub in_use: bool,
    pub tucked: bool,
}

impl AgentDockState {
    pub fn validate_seat_count(count: usize) -> Result<(), String> {
        if count > AGENT_DOCK_MAX_SEATS {
            return Err(format!(
                "agent dock seats count ({count}) exceeds maximum limit of {AGENT_DOCK_MAX_SEATS}"
            ));
        }
        Ok(())
    }

    pub fn new() -> Self {
        Self {
            placement: AgentDockPlacement::Right,
            seats: Vec::new(),
            idle_ms: 0,
            pointer_in: false,
            near: false,
            popover_open: false,
            dragging: false,
            shelf_shown: false,
            menu_open: false,
            active_chat_seat_id: None,
            reduced_motion: false,
        }
    }

    pub fn add_seat(&mut self, seat: AgentDockSeat) -> Result<(), String> {
        Self::validate_seat_count(self.seats.len() + 1)?;
        self.seats.push(seat);
        Ok(())
    }

    pub fn set_seats(&mut self, seats: Vec<AgentDockSeat>) -> Result<(), String> {
        Self::validate_seat_count(seats.len())?;
        self.seats = seats;
        Ok(())
    }

    pub fn reorder_seat(&mut self, from_idx: usize, to_idx: usize) -> bool {
        if from_idx >= self.seats.len() || to_idx >= self.seats.len() {
            return false;
        }
        let item = self.seats.remove(from_idx);
        self.seats.insert(to_idx, item);
        true
    }

    pub const fn in_use(&self) -> bool {
        self.pointer_in
            || self.near
            || self.popover_open
            || self.dragging
            || self.shelf_shown
            || self.menu_open
    }

    pub const fn is_tucked(&self) -> bool {
        self.idle_ms >= AGENT_DOCK_TUCK_DELAY_MS && !self.in_use()
    }

    pub const fn visible_strip_width_px(&self) -> u32 {
        if self.is_tucked() {
            AGENT_DOCK_RENDERED_STRIP_PX
        } else {
            56
        }
    }

    pub fn report(&self) -> AgentDockReport {
        AgentDockReport {
            max_seats: AGENT_DOCK_MAX_SEATS,
            seats_count: self.seats.len(),
            tuck_delay_ms: AGENT_DOCK_TUCK_DELAY_MS,
            strip_px: AGENT_DOCK_STRIP_PX,
            rendered_strip_px: AGENT_DOCK_RENDERED_STRIP_PX,
            strip_feel_px: AGENT_DOCK_STRIP_FEEL_PX,
            in_use: self.in_use(),
            tucked: self.is_tucked(),
        }
    }
}

/// Renders Surface 4's screen-edge `AgentDock` (`6px` tucked edge strip or expanded seat rail
/// with plush agent/channel avatars and quick-chat popover).
pub fn render_agent_dock(
    state: &AgentDockState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let glass_bg = pal_rgba(palette, "--openbot-bg-glass", 0x212121);
    let border = pal_rgba(palette, "--openbot-dialog-ring", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let control_bg = pal_rgba(palette, "--openbot-bg-control", 0x242424);

    if state.is_tucked() {
        let strip_h = (220.0 * AGENT_DOCK_STRIP_REST_RATIO).max(56.0);
        return div()
            .w(px(AGENT_DOCK_RENDERED_STRIP_PX as f32))
            .h(px(strip_h))
            .rounded_full()
            .bg(accent)
            .opacity(0.72)
            .border_1()
            .border_color(border)
            .shadow_md();
    }

    let mut seat_column = div()
        .w(px(56.0))
        .py(px(8.0))
        .px(px(6.0))
        .rounded(px(22.0))
        .bg(glass_bg)
        .border_1()
        .border_color(border)
        .shadow_lg()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0));

    for (idx, seat) in state.seats.iter().take(AGENT_DOCK_MAX_SEATS).enumerate() {
        let is_selected = state.active_chat_seat_id.as_deref() == Some(seat.id.as_str());
        seat_column = seat_column.child(
            div()
                .id(("agent-dock-seat", idx))
                .relative()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(px(14.0))
                .bg(if is_selected { control_bg } else { glass_bg })
                .border_1()
                .border_color(if is_selected { accent } else { border })
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(render_plush_seed_avatar(
                    &seat.avatar_seed,
                    seat.avatar_hue,
                    seat.avatar_color.as_deref(),
                    seat.mood,
                    now_sec,
                    state.reduced_motion,
                    30.0,
                ))
                .when(seat.kind == AgentDockSeatKind::Channel, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(px(-2.0))
                            .bottom(px(-2.0))
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded_full()
                            .bg(control_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(tg_icon_rgba(IconName::GroupUsers, 9.0, text_primary)),
                    )
                })
                .when(seat.unread_count > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .right(px(-3.0))
                            .top(px(-3.0))
                            .px(px(4.0))
                            .h(px(15.0))
                            .rounded_full()
                            .bg(accent)
                            .text_size(px(9.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_on_accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(seat.unread_count.to_string()),
                    )
                }),
        );
    }

    let active_seat = state
        .active_chat_seat_id
        .as_deref()
        .and_then(|id| state.seats.iter().find(|s| s.id == id))
        .or_else(|| state.seats.first());

    let mut container = div().flex().flex_row().items_start().gap(px(10.0));
    if state.popover_open {
        let header_name = active_seat
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Quick Chat".to_string());
        let popover = div()
            .w(px(300.0))
            .p(px(12.0))
            .rounded(px(18.0))
            .bg(glass_bg)
            .border_1()
            .border_color(border)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_primary)
                            .child(header_name),
                    )
                    .child(tg_icon_rgba(IconName::Mic, 14.0, text_muted)),
            )
            .child(
                div()
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(10.0))
                    .bg(control_bg)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_muted)
                            .child("Message agent from dock..."),
                    )
                    .child(tg_icon_rgba(IconName::SendPlane, 12.0, accent)),
            );
        if state.placement == AgentDockPlacement::Left {
            container = container.child(seat_column).child(popover);
        } else {
            container = container.child(popover).child(seat_column);
        }
    } else {
        container = container.child(seat_column);
    }
    container
}

// ============================================================================
// Surface 5 — BrowserDockSurface (`BrowserDock.tsx`)
// ============================================================================

pub const BROWSER_DOCK_REPLY_CLOSE_MS: u64 = 6_000;
pub const SEND_MORPH_WINDOW_MS: u64 = 1_500;
pub const HIDE_FADE_MS: u64 = 120;
pub const TAB_FADE_MS: u64 = 100;
pub const REVEAL_MS: u64 = 160;
pub const REVEAL_STAGGER_MS: u64 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserDockPhase {
    Open,
    Hiding,
    Hidden,
    Showing,
}

impl BrowserDockPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Hiding => "hiding",
            Self::Hidden => "hidden",
            Self::Showing => "showing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserDockIslandKind {
    Reply,
    Secret,
    Approval,
    Question,
    Prompt,
    Teach,
    Command,
}

impl BrowserDockIslandKind {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "secret" => Self::Secret,
            "approval" => Self::Approval,
            "question" => Self::Question,
            "prompt" => Self::Prompt,
            "teach" => Self::Teach,
            "command" => Self::Command,
            _ => Self::Reply,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Secret => "secret",
            Self::Approval => "approval",
            Self::Question => "question",
            Self::Prompt => "prompt",
            Self::Teach => "teach",
            Self::Command => "command",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserDockState {
    pub kind: BrowserDockIslandKind,
    pub has_question: bool,
    pub elapsed_ms: u64,
    pub send_elapsed_ms: Option<u64>,
    pub teach_recording: bool,
    pub tab_title: String,
    pub tab_url: String,
    pub message_preview: String,
    pub queued_count: usize,
}

impl Default for BrowserDockState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserDockReport {
    pub reply_close_ms: u64,
    pub send_morph_window_ms: u64,
    pub hide_fade_ms: u64,
    pub kind: String,
    pub has_question: bool,
    pub needs_answer: bool,
    pub open: bool,
    pub phase: String,
    pub teach_recording: bool,
}

impl BrowserDockState {
    pub fn new() -> Self {
        Self {
            kind: BrowserDockIslandKind::Reply,
            has_question: false,
            elapsed_ms: 0,
            send_elapsed_ms: None,
            teach_recording: false,
            tab_title: "Browser".to_string(),
            tab_url: "https://openbot.local".to_string(),
            message_preview: String::new(),
            queued_count: 0,
        }
    }

    pub const fn needs_answer(&self) -> bool {
        matches!(
            self.kind,
            BrowserDockIslandKind::Secret
                | BrowserDockIslandKind::Approval
                | BrowserDockIslandKind::Question
                | BrowserDockIslandKind::Prompt
        ) || self.has_question
    }

    pub const fn is_open(&self) -> bool {
        self.needs_answer() || self.elapsed_ms < BROWSER_DOCK_REPLY_CLOSE_MS
    }

    pub const fn phase(&self) -> BrowserDockPhase {
        if self.is_open() {
            BrowserDockPhase::Open
        } else if self.elapsed_ms < BROWSER_DOCK_REPLY_CLOSE_MS + HIDE_FADE_MS {
            BrowserDockPhase::Hiding
        } else {
            BrowserDockPhase::Hidden
        }
    }

    pub const fn in_send_morph_window(&self) -> bool {
        match self.send_elapsed_ms {
            Some(ms) => ms < SEND_MORPH_WINDOW_MS,
            None => false,
        }
    }

    pub fn report(&self) -> BrowserDockReport {
        BrowserDockReport {
            reply_close_ms: BROWSER_DOCK_REPLY_CLOSE_MS,
            send_morph_window_ms: SEND_MORPH_WINDOW_MS,
            hide_fade_ms: HIDE_FADE_MS,
            kind: self.kind.as_str().to_string(),
            has_question: self.has_question,
            needs_answer: self.needs_answer(),
            open: self.is_open(),
            phase: self.phase().as_str().to_string(),
            teach_recording: self.teach_recording,
        }
    }
}

/// Renders Surface 5's frosted glass `BrowserDock` island (tucked pill when closed, expanded
/// command/reply/secret/teach island when open).
pub fn render_browser_dock(state: &BrowserDockState, palette: &OpenBotPalette) -> Div {
    let glass_bg = pal_rgba(palette, "--openbot-bg-glass", 0x212121);
    let border = pal_rgba(palette, "--openbot-dialog-ring", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let warning = pal_rgba(palette, "--openbot-warning", 0xff9412);
    let warning_soft = pal_rgba(palette, "--openbot-warning-soft", 0x3b2a14);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x44181c);

    if !state.is_open() {
        return div()
            .px(px(12.0))
            .py(px(6.0))
            .rounded_full()
            .bg(glass_bg)
            .border_1()
            .border_color(border)
            .shadow_md()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(tg_icon_rgba(IconName::BotRobot, 14.0, accent))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_secondary)
                    .child(state.tab_title.clone()),
            );
    }

    let badge_text = if state.kind == BrowserDockIslandKind::Secret {
        "Secret Required"
    } else if state.teach_recording || state.kind == BrowserDockIslandKind::Teach {
        "Teaching Browser"
    } else if state.needs_answer() {
        "Needs Your Answer"
    } else {
        "Browser Dock"
    };

    let (badge_bg, badge_fg) = if state.teach_recording || state.kind == BrowserDockIslandKind::Teach
    {
        (danger_soft, danger)
    } else if state.needs_answer() {
        (warning_soft, warning)
    } else {
        (glass_bg, accent)
    };

    div()
        .w(px(360.0))
        .p(px(12.0))
        .rounded(px(18.0))
        .bg(glass_bg)
        .border_1()
        .border_color(if state.needs_answer() { warning } else { border })
        .shadow_lg()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded_full()
                        .bg(badge_bg)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .child(tg_icon_rgba(
                            if state.kind == BrowserDockIslandKind::Secret {
                                IconName::Lock
                            } else {
                                IconName::BotRobot
                            },
                            12.0,
                            badge_fg,
                        ))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(badge_fg)
                                .child(badge_text),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .truncate()
                        .child(state.tab_url.clone()),
                ),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(text_primary)
                .child(if state.message_preview.is_empty() {
                    format!("Active browser island ({})", state.kind.as_str())
                } else {
                    state.message_preview.clone()
                }),
        )
        .when(state.queued_count > 0, |el| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(format!("{} queued browser actions", state.queued_count)),
            )
        })
}

// ============================================================================
// Surface 6 — CuaOverlaySurface (`ComputerUseHighlightSurface.tsx`, `ComputerUseAgentCursor.tsx`)
// ============================================================================

pub const CUA_POLL_MS: u64 = 33;
pub const CUA_CORNER_RADIUS_PX: u32 = 12;
pub const CUA_IDLE_TIMEOUT_MS: u64 = 60_000;
pub const CUA_CURSOR_VIEWBOX: (f32, f32) = (45.0, 49.0);
pub const CUA_CURSOR_SVG_PATH: &str = "M0 0 L45 20 L27 31 L14 49 Z";
pub const CUA_CURSOR_POINTS: [(f32, f32); 4] = [
    (0.0, 0.0),
    (45.0, 20.0),
    (27.0, 31.0),
    (14.0, 49.0),
];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CuaRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for CuaRect {
    fn default() -> Self {
        Self {
            x: 24.0,
            y: 24.0,
            width: 640.0,
            height: 420.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CuaCursorPlan {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub control: (f32, f32),
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuaOverlayState {
    pub active: bool,
    pub idle_ms: u64,
    pub reduced_motion: bool,
    pub target_rect: CuaRect,
    pub occluded_rects: Vec<CuaRect>,
    pub cursor_pos: (f32, f32),
    pub cursor_label: Option<String>,
    pub cursor_plan: Option<CuaCursorPlan>,
}

impl Default for CuaOverlayState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuaOverlayReport {
    pub poll_ms: u64,
    pub corner_radius_px: u32,
    pub idle_timeout_ms: u64,
    pub cursor_viewbox: [f32; 2],
    pub cursor_svg_path: String,
    pub visible: bool,
    pub cursor_active: bool,
    pub cursor_pos: [f32; 2],
    pub occluded_count: usize,
}

impl CuaOverlayState {
    pub fn new() -> Self {
        Self {
            active: false,
            idle_ms: 0,
            reduced_motion: false,
            target_rect: CuaRect::default(),
            occluded_rects: Vec::new(),
            cursor_pos: (160.0, 120.0),
            cursor_label: None,
            cursor_plan: None,
        }
    }

    pub const fn is_visible(&self) -> bool {
        self.active && self.idle_ms < CUA_IDLE_TIMEOUT_MS
    }

    pub const fn is_cursor_active(&self) -> bool {
        self.is_visible()
    }

    pub fn plan_agent_cursor_move(&mut self, to: (f32, f32)) -> CuaCursorPlan {
        let from = self.cursor_pos;
        if self.reduced_motion {
            self.cursor_pos = to;
            let plan = CuaCursorPlan {
                from,
                to,
                control: ((from.0 + to.0) * 0.5, (from.1 + to.1) * 0.5),
                duration_ms: 0,
            };
            self.cursor_plan = Some(plan);
            return plan;
        }
        let dx = to.0 - from.0;
        let dy = to.1 - from.1;
        let dist = (dx * dx + dy * dy).sqrt();
        let arc_offset = (dist * 0.14).clamp(12.0, 56.0);
        let nx = if dist > 1e-3 { -dy / dist } else { 0.0 };
        let ny = if dist > 1e-3 { dx / dist } else { -1.0 };
        let control = (
            (from.0 + to.0) * 0.5 + nx * arc_offset,
            (from.1 + to.1) * 0.5 + ny * arc_offset,
        );
        let duration_ms = ((160.0 + dist * 0.45).clamp(160.0, 480.0)).round() as u64;
        let plan = CuaCursorPlan {
            from,
            to,
            control,
            duration_ms,
        };
        self.cursor_plan = Some(plan);
        plan
    }

    pub fn sample_cursor_position(&self, progress_01: f32) -> (f32, f32) {
        if self.reduced_motion {
            return self.cursor_pos;
        }
        let Some(plan) = self.cursor_plan else {
            return self.cursor_pos;
        };
        let u = progress_01.clamp(0.0, 1.0);
        let t = 1.0 - (1.0 - u).powi(3);
        let omt = 1.0 - t;
        let x = omt * omt * plan.from.0 + 2.0 * omt * t * plan.control.0 + t * t * plan.to.0;
        let y = omt * omt * plan.from.1 + 2.0 * omt * t * plan.control.1 + t * t * plan.to.1;
        (x, y)
    }

    pub fn report(&self) -> CuaOverlayReport {
        CuaOverlayReport {
            poll_ms: CUA_POLL_MS,
            corner_radius_px: CUA_CORNER_RADIUS_PX,
            idle_timeout_ms: CUA_IDLE_TIMEOUT_MS,
            cursor_viewbox: [CUA_CURSOR_VIEWBOX.0, CUA_CURSOR_VIEWBOX.1],
            cursor_svg_path: CUA_CURSOR_SVG_PATH.to_string(),
            visible: self.is_visible(),
            cursor_active: self.is_cursor_active(),
            cursor_pos: [self.cursor_pos.0, self.cursor_pos.1],
            occluded_count: self.occluded_rects.len(),
        }
    }
}

fn build_rounded_rect_stroke_path(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    stroke_px: f32,
) -> Option<Path<Pixels>> {
    if w <= 1.0 || h <= 1.0 {
        return None;
    }
    let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
    let k = 0.552_284_8 * r;
    let x0 = x;
    let y0 = y;
    let x1 = x + w;
    let y1 = y + h;
    let mut b = PathBuilder::stroke(px(stroke_px));
    b.move_to(point(px(x0 + r), px(y0)));
    b.line_to(point(px(x1 - r), px(y0)));
    b.cubic_bezier_to(
        point(px(x1), px(y0 + r)),
        point(px(x1 - r + k), px(y0)),
        point(px(x1), px(y0 + r - k)),
    );
    b.line_to(point(px(x1), px(y1 - r)));
    b.cubic_bezier_to(
        point(px(x1 - r), px(y1)),
        point(px(x1), px(y1 - r + k)),
        point(px(x1 - r + k), px(y1)),
    );
    b.line_to(point(px(x0 + r), px(y1)));
    b.cubic_bezier_to(
        point(px(x0), px(y1 - r)),
        point(px(x0 + r - k), px(y1)),
        point(px(x0), px(y1 - r + k)),
    );
    b.line_to(point(px(x0), px(y0 + r)));
    b.cubic_bezier_to(
        point(px(x0 + r), px(y0)),
        point(px(x0), px(y0 + r - k)),
        point(px(x0 + r - k), px(y0)),
    );
    b.close();
    b.build().ok()
}

fn paint_cua_overlay_canvas(
    bounds: Bounds<Pixels>,
    state: &CuaOverlayState,
    accent: Rgba,
    window: &mut Window,
) {
    let bx = f32::from(bounds.origin.x);
    let by = f32::from(bounds.origin.y);
    let bw = f32::from(bounds.size.width).max(1.0);
    let bh = f32::from(bounds.size.height).max(1.0);

    let rx = bx + state.target_rect.x.clamp(0.0, bw);
    let ry = by + state.target_rect.y.clamp(0.0, bh);
    let rw = state.target_rect.width.clamp(24.0, (bw - (rx - bx)).max(24.0));
    let rh = state.target_rect.height.clamp(24.0, (bh - (ry - by)).max(24.0));
    let radius = CUA_CORNER_RADIUS_PX as f32;

    let accent_hsla: Hsla = accent.into();
    if let Some(glow_path) = build_rounded_rect_stroke_path(rx, ry, rw, rh, radius, 8.0) {
        window.paint_path(glow_path, accent_hsla.opacity(0.24));
    }
    if let Some(ring_path) = build_rounded_rect_stroke_path(rx, ry, rw, rh, radius, 2.5) {
        window.paint_path(ring_path, accent_hsla.opacity(0.95));
    }

    for cut in &state.occluded_rects {
        let cx0 = bx + cut.x.clamp(0.0, bw);
        let cy0 = by + cut.y.clamp(0.0, bh);
        let cx1 = (cx0 + cut.width.max(0.0)).min(bx + bw);
        let cy1 = (cy0 + cut.height.max(0.0)).min(by + bh);
        if cx1 <= cx0 || cy1 <= cy0 {
            continue;
        }
        let mut mask_b = PathBuilder::fill();
        mask_b.move_to(point(px(cx0), px(cy0)));
        mask_b.line_to(point(px(cx1), px(cy0)));
        mask_b.line_to(point(px(cx1), px(cy1)));
        mask_b.line_to(point(px(cx0), px(cy1)));
        mask_b.close();
        if let Ok(mask_path) = mask_b.build() {
            window.paint_path(mask_path, Hsla::from(rgb(0x0b0d0e)).opacity(0.45));
        }
    }

    let cursor_scale = 0.56_f32;
    let cx = bx + state.cursor_pos.0.clamp(0.0, bw);
    let cy = by + state.cursor_pos.1.clamp(0.0, bh);

    let emit_cursor = |dx: f32, dy: f32, stroke: Option<f32>| -> Option<Path<Pixels>> {
        let mut b = match stroke {
            Some(w) => PathBuilder::stroke(px(w)),
            None => PathBuilder::fill(),
        };
        for (idx, &(vx, vy)) in CUA_CURSOR_POINTS.iter().enumerate() {
            let pt = point(
                px(cx + dx + vx * cursor_scale),
                px(cy + dy + vy * cursor_scale),
            );
            if idx == 0 {
                b.move_to(pt);
            } else {
                b.line_to(pt);
            }
        }
        b.close();
        b.build().ok()
    };

    if let Some(shadow_path) = emit_cursor(1.5, 2.5, None) {
        window.paint_path(shadow_path, Hsla::from(rgb(0x000000)).opacity(0.35));
    }
    if let Some(fill_path) = emit_cursor(0.0, 0.0, None) {
        window.paint_path(fill_path, accent_hsla);
    }
    if let Some(outline_path) = emit_cursor(0.0, 0.0, Some(1.6)) {
        window.paint_path(outline_path, rgb(0xffffff));
    }
}

/// Renders Surface 6's CUA overlay via GPUI `canvas(...)`, painting the `12px`-rounded
/// target highlight frame, occluded cutout rects, and `M0 0 L45 20 L27 31 L14 49 Z` cursor.
pub fn render_cua_overlay(state: &CuaOverlayState, palette: &OpenBotPalette) -> Div {
    if !state.is_visible() {
        return div();
    }
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let state_owned = state.clone();

    div()
        .relative()
        .size_full()
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_cua_overlay_canvas(bounds, &state_owned, accent, window);
                },
            )
            .size_full(),
        )
        .when_some(state.cursor_label.clone(), |el, label| {
            let lx = state.cursor_pos.0 + (CUA_CURSOR_VIEWBOX.0 * 0.56) + 4.0;
            let ly = state.cursor_pos.1 + (CUA_CURSOR_VIEWBOX.1 * 0.42);
            el.child(
                div()
                    .absolute()
                    .left(px(lx))
                    .top(px(ly))
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded_full()
                    .bg(accent)
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_on_accent)
                    .child(label),
            )
        })
}

// ============================================================================
// Surface 7 — RecoverySurface (`start-recovery-window.ts`, `start-recovery-plush.ts`)
// ============================================================================

pub const RECOVERY_WINDOW_WIDTH_PX: u32 = 520;
pub const RECOVERY_WINDOW_HEIGHT_PX: u32 = 640;
pub const RECOVERY_WINDOW_MIN_WIDTH_PX: u32 = 420;
pub const RECOVERY_WINDOW_MIN_HEIGHT_PX: u32 = 420;
pub const RECOVERY_CRASH_THRESHOLD: usize = 4;
pub const RECOVERY_WINDOW_MS: u64 = 60_000;
pub const RECOVERY_SHOW_TIMEOUT_MS: u64 = 15_000;
pub const RECOVERY_DARK_BG_HEX: &str = "#1c1c1e";
pub const RECOVERY_LIGHT_BG_HEX: &str = "#ffffff";
pub const RECOVERY_PLUSH_CHARACTER: &str = "cloe";
pub const RECOVERY_ACTIONS: [&str; 6] = [
    "retry",
    "previous-version",
    "latest-version",
    "export-diagnostics",
    "open-data-folder",
    "quit",
];

/// Returns `true` iff any sliding window of `4` crashes (`> 3`) spans `<= 60_000ms`.
pub fn should_trigger_recovery(crash_timestamps_ms: &[u64]) -> bool {
    if crash_timestamps_ms.len() < RECOVERY_CRASH_THRESHOLD {
        return false;
    }
    let mut sorted = crash_timestamps_ms.to_vec();
    sorted.sort_unstable();
    sorted
        .windows(RECOVERY_CRASH_THRESHOLD)
        .any(|w| w[RECOVERY_CRASH_THRESHOLD - 1].saturating_sub(w[0]) <= RECOVERY_WINDOW_MS)
}

/// Zero-bundle recovery isolation guard: blocks every URL except `data:`.
pub fn is_external_url_blocked(url: &str) -> bool {
    !url.trim().starts_with("data:")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryAction {
    Retry,
    PreviousVersion,
    LatestVersion,
    ExportDiagnostics,
    OpenDataFolder,
    Quit,
}

impl RecoveryAction {
    pub const ALL: [Self; 6] = [
        Self::Retry,
        Self::PreviousVersion,
        Self::LatestVersion,
        Self::ExportDiagnostics,
        Self::OpenDataFolder,
        Self::Quit,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::PreviousVersion => "previous-version",
            Self::LatestVersion => "latest-version",
            Self::ExportDiagnostics => "export-diagnostics",
            Self::OpenDataFolder => "open-data-folder",
            Self::Quit => "quit",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Retry => "Retry Launch",
            Self::PreviousVersion => "Switch to Previous Version",
            Self::LatestVersion => "Install Latest Version",
            Self::ExportDiagnostics => "Export Diagnostics",
            Self::OpenDataFolder => "Open Data Folder",
            Self::Quit => "Quit OpenBot",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "retry" => Some(Self::Retry),
            "previous-version" | "previous_version" => Some(Self::PreviousVersion),
            "latest-version" | "latest_version" => Some(Self::LatestVersion),
            "export-diagnostics" | "export_diagnostics" => Some(Self::ExportDiagnostics),
            "open-data-folder" | "open_data_folder" => Some(Self::OpenDataFolder),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySurfaceState {
    pub crash_timestamps_ms: Vec<u64>,
    pub crash_reason: String,
    pub current_version: String,
    pub previous_version: Option<String>,
    pub latest_version: Option<String>,
    pub requested_url: Option<String>,
    pub selected_action: Option<RecoveryAction>,
}

impl Default for RecoverySurfaceState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySurfaceReport {
    pub triggered: bool,
    pub window_width_px: u32,
    pub window_height_px: u32,
    pub min_width_px: u32,
    pub min_height_px: u32,
    pub crash_threshold: usize,
    pub window_ms: u64,
    pub plush_character: String,
    pub plush_svg_present: bool,
    pub actions: Vec<String>,
    pub external_url_blocked: bool,
}

impl RecoverySurfaceState {
    pub fn new() -> Self {
        Self {
            crash_timestamps_ms: Vec::new(),
            crash_reason: "Renderer exited repeatedly within 60 seconds".to_string(),
            current_version: "0.78.0".to_string(),
            previous_version: Some("0.77.2".to_string()),
            latest_version: Some("0.78.1".to_string()),
            requested_url: None,
            selected_action: None,
        }
    }

    pub fn is_triggered(&self) -> bool {
        should_trigger_recovery(&self.crash_timestamps_ms)
    }

    pub fn external_url_blocked(&self) -> bool {
        self.requested_url
            .as_deref()
            .is_some_and(is_external_url_blocked)
    }

    pub fn plush_svg(&self) -> String {
        recovery_plush_svg()
    }

    /// Generates the zero-bundle self-contained `data:text/html` document embedding M2's
    /// `cloe` 4-slice `plus-lighter` `recovery_plush_svg()` and all 6 recovery actions.
    pub fn build_zero_bundle_data_url(&self) -> String {
        let svg = recovery_plush_svg();
        let actions_html: String = RecoveryAction::ALL
            .iter()
            .map(|a| format!("<button data-action=\"{}\">{}</button>", a.as_str(), a.label()))
            .collect();
        let html = format!(
            "<!doctype html><html><body data-character=\"{RECOVERY_PLUSH_CHARACTER}\">{svg}<h1>OpenBot Recovery</h1><p>{}</p><div>{actions_html}</div></body></html>",
            self.crash_reason
        );
        format!("data:text/html;charset=utf-8,{html}")
    }

    pub fn report(&self) -> RecoverySurfaceReport {
        let svg = recovery_plush_svg();
        RecoverySurfaceReport {
            triggered: self.is_triggered(),
            window_width_px: RECOVERY_WINDOW_WIDTH_PX,
            window_height_px: RECOVERY_WINDOW_HEIGHT_PX,
            min_width_px: RECOVERY_WINDOW_MIN_WIDTH_PX,
            min_height_px: RECOVERY_WINDOW_MIN_HEIGHT_PX,
            crash_threshold: RECOVERY_CRASH_THRESHOLD,
            window_ms: RECOVERY_WINDOW_MS,
            plush_character: RECOVERY_PLUSH_CHARACTER.to_string(),
            plush_svg_present: !svg.is_empty(),
            actions: RECOVERY_ACTIONS.iter().map(|s| (*s).to_string()).collect(),
            external_url_blocked: self.external_url_blocked(),
        }
    }
}

/// Renders Surface 7's Startup `Recovery` window (`520x640`, `cloe` plush via M2's
/// `recovery_plush_svg()`, zero-bundle isolation status, and all 6 recovery action buttons).
pub fn render_recovery_surface(state: &RecoverySurfaceState, palette: &OpenBotPalette) -> Div {
    let bg_color = match palette.scheme {
        OpenBotColorScheme::Dark => rgb(0x1c1c1e),
        OpenBotColorScheme::Light => rgb(0xffffff),
    };
    let card_bg = pal_rgba(palette, "--openbot-bg-surface", 0x242426);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x44181c);
    let control_bg = pal_rgba(palette, "--openbot-bg-control", 0x2a2a2c);
    let control_hover = pal_rgba(palette, "--openbot-bg-control-hover", 0x343438);

    let svg_markup = recovery_plush_svg();
    let plush_art = match BundledPlushAssets::global().render_image("bloub/cloe-corps.webp") {
        Ok(cloe_img) => div()
            .w(px(108.0))
            .h(px(108.0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                img(cloe_img)
                    .w(px(104.0))
                    .h(px(104.0))
                    .object_fit(ObjectFit::Contain),
            ),
        Err(_) => render_plush_seed_avatar(
            RECOVERY_PLUSH_CHARACTER,
            None,
            None,
            AvatarMood::Failed,
            0.0,
            true,
            96.0,
        ),
    };

    let mut actions_col = div().w_full().flex().flex_col().gap(px(8.0));
    for (idx, action) in RecoveryAction::ALL.iter().enumerate() {
        let (btn_bg, btn_fg, btn_border) = match action {
            RecoveryAction::Retry => (accent, text_on_accent, accent),
            RecoveryAction::Quit => (danger_soft, danger, danger),
            _ => (control_bg, text_primary, border),
        };
        let detail = match action {
            RecoveryAction::PreviousVersion => state
                .previous_version
                .as_deref()
                .map(|v| format!("v{v}"))
                .unwrap_or_default(),
            RecoveryAction::LatestVersion => state
                .latest_version
                .as_deref()
                .map(|v| format!("v{v}"))
                .unwrap_or_default(),
            _ => action.as_str().to_string(),
        };
        actions_col = actions_col.child(
            div()
                .id(("recovery-action-btn", idx))
                .w_full()
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(12.0))
                .bg(btn_bg)
                .border_1()
                .border_color(btn_border)
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .hover(move |s| s.bg(control_hover))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(btn_fg)
                        .child(action.label()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(if *action == RecoveryAction::Retry {
                            text_on_accent
                        } else {
                            text_muted
                        })
                        .child(detail),
                ),
        );
    }

    div()
        .w(px(RECOVERY_WINDOW_WIDTH_PX as f32))
        .h(px(RECOVERY_WINDOW_HEIGHT_PX as f32))
        .min_w(px(RECOVERY_WINDOW_MIN_WIDTH_PX as f32))
        .min_h(px(RECOVERY_WINDOW_MIN_HEIGHT_PX as f32))
        .p(px(28.0))
        .bg(bg_color)
        .rounded(px(20.0))
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .gap(px(14.0))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .child(plush_art)
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(text_primary)
                        .child("OpenBot Recovery Mode"),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(text_secondary)
                        .child(state.crash_reason.clone()),
                )
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(card_bg)
                        .border_1()
                        .border_color(border)
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(format!(
                            "cloe zero-bundle · {} crashes · svg {}",
                            state.crash_timestamps_ms.len(),
                            if svg_markup.is_empty() { "fallback" } else { "embedded" }
                        )),
                ),
        )
        .when(state.external_url_blocked(), |el| {
            el.child(
                div()
                    .w_full()
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(danger_soft)
                    .border_1()
                    .border_color(danger)
                    .text_size(px(12.0))
                    .text_color(danger)
                    .child("External URL blocked: recovery mode only allows data: documents."),
            )
        })
        .child(actions_col)
}

// ============================================================================
// Unified Desktop OS Surfaces State & Deterministic `desktop-os-surfaces` Evaluator
// ============================================================================

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DesktopOsSurfacesState {
    pub main_window: MainWindowState,
    pub chat_window: ChatWindowState,
    pub dynamic_island: DynamicIslandState,
    pub agent_dock: AgentDockState,
    pub browser_dock: BrowserDockState,
    pub cua: CuaOverlayState,
    pub recovery: RecoverySurfaceState,
}

impl DesktopOsSurfacesState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn to_report_json(&self) -> Value {
        json!({
            "surfaces": SURFACE_NAMES,
            "main_window": self.main_window.report(),
            "chat_window": self.chat_window.report(),
            "dynamic_island": self.dynamic_island.report(),
            "agent_dock": self.agent_dock.report(),
            "browser_dock": self.browser_dock.report(),
            "cua": self.cua.report(),
            "recovery": self.recovery.report(),
        })
    }
}

fn json_u64(input: &Value, key: &str, default_val: u64) -> u64 {
    input.get(key).and_then(Value::as_u64).unwrap_or(default_val)
}

fn json_bool(input: &Value, key: &str, default_val: bool) -> bool {
    input.get(key).and_then(Value::as_bool).unwrap_or(default_val)
}

/// Deterministic evaluator backing `ui-probe --scenario desktop-os-surfaces` (`OB-F19`).
pub fn evaluate_desktop_os_surfaces_probe(
    input: &Value,
) -> Result<Value, OsSurfaceValidationError> {
    let mut surfaces = DesktopOsSurfacesState::new();
    let reduced_motion = json_bool(input, "reduced_motion", false);

    // 1. MainWindow
    surfaces.main_window.width_px = json_u64(input, "main_width_px", 1280) as u32;
    surfaces.main_window.height_px = json_u64(input, "main_height_px", 800) as u32;
    surfaces.main_window.working_agents_count =
        json_u64(input, "working_agents_count", 0) as usize;
    surfaces.main_window.attempt_close = json_bool(input, "attempt_close_main", false);
    if let Some(hold_ms) = input.get("cmd_q_hold_ms").and_then(Value::as_u64) {
        surfaces.main_window.update_cmd_q_hold(hold_ms);
    }

    // 2. ChatWindow
    let cw_agent_id = input
        .get("chat_window_agent_id")
        .and_then(Value::as_str)
        .unwrap_or("agent-1");
    surfaces.chat_window = ChatWindowState::new(cw_agent_id);
    surfaces.chat_window.width_px = json_u64(input, "chat_window_width_px", 520) as u32;
    surfaces.chat_window.height_px = json_u64(input, "chat_window_height_px", 720) as u32;
    if json_bool(input, "delete_chat_window_agent", false) {
        surfaces.chat_window.set_target_deleted(true);
    }

    // 3. DynamicIsland
    surfaces.dynamic_island.reduced_motion = reduced_motion;
    if let Some(pref_val) = input.get("island_preference").filter(|v| !v.is_null()) {
        let wp_raw = match pref_val
            .get("widthPercent")
            .or_else(|| pref_val.get("width_percent"))
        {
            Some(v) => v.as_i64().ok_or_else(|| {
                OsSurfaceValidationError::new(
                    "island_preference",
                    "widthPercent must be an integer multiple of 5 within range 20..=130",
                )
            })?,
            None => 100,
        };
        let hp_raw = match pref_val
            .get("heightPercent")
            .or_else(|| pref_val.get("height_percent"))
        {
            Some(v) => v.as_i64().ok_or_else(|| {
                OsSurfaceValidationError::new(
                    "island_preference",
                    "heightPercent must be an integer multiple of 5 within range 75..=125",
                )
            })?,
            None => 100,
        };
        if wp_raw < 0 || hp_raw < 0 {
            return Err(OsSurfaceValidationError::new(
                "island_preference",
                "widthPercent and heightPercent must be non-negative and within valid range",
            ));
        }
        DynamicIslandPreferenceValidator::validate_dimensions(wp_raw as u32, hp_raw as u32)
            .map_err(|msg| OsSurfaceValidationError::new("island_preference", msg))?;
        if let Ok(pref) = serde_json::from_value::<DynamicIslandPreference>(pref_val.clone()) {
            surfaces
                .dynamic_island
                .set_preference(pref)
                .map_err(|msg| OsSurfaceValidationError::new("island_preference", msg))?;
        }
    }

    if let Some(reply_val) = input.get("island_inline_reply").and_then(Value::as_str) {
        surfaces
            .dynamic_island
            .submit_inline_reply(reply_val)
            .map_err(|msg| OsSurfaceValidationError::new("island_inline_reply", msg))?;
    }

    surfaces.dynamic_island.interacting = json_bool(input, "island_interacting", false);
    surfaces.dynamic_island.typing = json_bool(input, "island_typing", false);

    if let Some(cur_val) = input.get("island_current").filter(|v| !v.is_null()) {
        let cur: DynamicIslandPresentationItem = serde_json::from_value(cur_val.clone())
            .map_err(|err| OsSurfaceValidationError::new("island_current", err.to_string()))?;
        surfaces.dynamic_island.current = cur;
    }
    if let Some(next_val) = input.get("island_next").filter(|v| !v.is_null()) {
        let next: DynamicIslandPresentationItem = serde_json::from_value(next_val.clone())
            .map_err(|err| OsSurfaceValidationError::new("island_next", err.to_string()))?;
        surfaces.dynamic_island.apply_incoming(next);
    }

    // 4. AgentDock
    let seats_count = json_u64(input, "dock_seats_count", 0) as usize;
    AgentDockState::validate_seat_count(seats_count)
        .map_err(|msg| OsSurfaceValidationError::new("dock_seats_count", msg))?;
    let seats = (0..seats_count)
        .map(|idx| AgentDockSeat::agent(format!("dock-agent-{idx}"), format!("Agent {idx}")))
        .collect();
    surfaces
        .agent_dock
        .set_seats(seats)
        .map_err(|msg| OsSurfaceValidationError::new("dock_seats_count", msg))?;
    surfaces.agent_dock.idle_ms = json_u64(input, "dock_idle_ms", 0);
    surfaces.agent_dock.popover_open = json_bool(input, "dock_popover_open", false);
    surfaces.agent_dock.pointer_in = json_bool(input, "dock_pointer_in", false);
    surfaces.agent_dock.near = json_bool(input, "dock_near", false);
    surfaces.agent_dock.dragging = json_bool(input, "dock_dragging", false);
    surfaces.agent_dock.shelf_shown = json_bool(input, "dock_shelf_shown", false);
    surfaces.agent_dock.menu_open = json_bool(input, "dock_menu_open", false);
    surfaces.agent_dock.reduced_motion = reduced_motion;

    // 5. BrowserDock
    if let Some(bd_val) = input.get("browser_dock_island").filter(|v| !v.is_null()) {
        if let Some(kind_str) = bd_val.get("kind").and_then(Value::as_str) {
            surfaces.browser_dock.kind = BrowserDockIslandKind::parse(kind_str);
        }
        surfaces.browser_dock.has_question = bd_val
            .get("question")
            .or_else(|| bd_val.get("has_question"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }
    surfaces.browser_dock.elapsed_ms = json_u64(input, "browser_dock_elapsed_ms", 0);

    // 6. CUA
    surfaces.cua.active = json_bool(input, "cua_active", false);
    surfaces.cua.idle_ms = json_u64(input, "cua_idle_ms", 0);
    surfaces.cua.reduced_motion = reduced_motion;

    // 7. Recovery
    if let Some(ts_arr) = input.get("crash_timestamps_ms").and_then(Value::as_array) {
        surfaces.recovery.crash_timestamps_ms =
            ts_arr.iter().filter_map(Value::as_u64).collect();
    }
    surfaces.recovery.requested_url = input
        .get("recovery_request_url")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(surfaces.to_report_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn main_window_and_detached_chat_window_invariants() {
        let mut main = MainWindowState::new(700, 500);
        main.working_agents_count = 2;
        main.attempt_close = true;
        assert!(main.auto_zoomed());
        assert!(main.close_guard_prompted());
        assert_eq!(main.clamped_size(), (960, 640));
        assert_eq!(main.update_cmd_q_hold(CMD_Q_HOLD_MS), 1.0);
        assert!(main.should_quit_from_hold());

        let mut cw = ChatWindowState::new("agent-alpha");
        cw.width_px = 240;
        assert_eq!(cw.effective_width_px(), 380);
        assert_eq!(cw.effective_height_px(), 720);
        assert!(cw.rails_hidden && cw.sidebar_hidden);
        assert!(!cw.is_settings_tab_visible("dynamic-island"));
        assert!(!cw.is_settings_tab_visible("agent-dock"));
        assert!(cw.is_settings_tab_visible("general"));
        assert!(cw.on_agent_deleted("agent-alpha"));
        assert!(cw.auto_closed);
    }

    #[std::prelude::v1::test]
    fn dynamic_island_validators_and_critical_presentation_lock() {
        assert!(DynamicIslandPreferenceValidator::validate_dimensions(100, 100).is_ok());
        for (w, h) in [(15, 100), (135, 100), (103, 100), (100, 70), (100, 130)] {
            assert!(DynamicIslandPreferenceValidator::validate_dimensions(w, h).is_err());
        }
        assert!(validate_inline_reply("Ship it").is_ok());
        assert!(validate_inline_reply("   ").is_err());
        assert!(validate_inline_reply(&"R".repeat(20_001)).is_err());

        let approval = DynamicIslandPresentationItem {
            mode: DynamicIslandMode::Approval,
            server_id: "local".into(),
            request_id: Some("r1".into()),
            ..Default::default()
        };
        let question = DynamicIslandPresentationItem {
            mode: DynamicIslandMode::Question,
            server_id: "local".into(),
            request_id: Some("r2".into()),
            ..Default::default()
        };
        let tr_locked = evaluate_presentation_transition(&approval, &question, true, false);
        assert!(tr_locked.locked && tr_locked.critical_lock);
        assert_eq!(tr_locked.active_mode, DynamicIslandMode::Approval);
        assert_eq!(tr_locked.queued_mode, Some(DynamicIslandMode::Question));

        let resolved = DynamicIslandPresentationItem {
            mode: DynamicIslandMode::Working,
            server_id: "local".into(),
            resolved_request_id: Some("r1".into()),
            ..Default::default()
        };
        let tr_unlocked = evaluate_presentation_transition(&approval, &resolved, true, false);
        assert!(!tr_unlocked.locked);
        assert_eq!(tr_unlocked.active_mode, DynamicIslandMode::Working);
        assert_eq!(tr_unlocked.queued_mode, None);

        let msg = DynamicIslandPresentationItem {
            mode: DynamicIslandMode::Message,
            server_id: "local".into(),
            request_id: Some("m1".into()),
            ..Default::default()
        };
        let work = DynamicIslandPresentationItem {
            mode: DynamicIslandMode::Working,
            server_id: "local".into(),
            request_id: Some("w2".into()),
            ..Default::default()
        };
        let tr_typing = evaluate_presentation_transition(&msg, &work, true, true);
        assert!(tr_typing.locked);
        assert_eq!(tr_typing.active_mode, DynamicIslandMode::Message);
    }

    #[std::prelude::v1::test]
    fn agent_dock_browser_dock_cua_and_recovery_rules() {
        assert!(AgentDockState::validate_seat_count(24).is_ok());
        assert!(AgentDockState::validate_seat_count(25).is_err());
        let mut dock = AgentDockState::new();
        dock.idle_ms = 400;
        assert!(!dock.is_tucked());
        dock.idle_ms = 950;
        assert!(dock.is_tucked());
        dock.popover_open = true;
        assert!(!dock.is_tucked());

        let mut bd = BrowserDockState::new();
        bd.kind = BrowserDockIslandKind::Reply;
        bd.elapsed_ms = 6_100;
        assert!(!bd.is_open());
        bd.kind = BrowserDockIslandKind::Secret;
        bd.elapsed_ms = 15_000;
        assert!(bd.needs_answer() && bd.is_open());

        let mut cua = CuaOverlayState::new();
        cua.active = true;
        cua.idle_ms = 1_000;
        assert!(cua.is_visible() && cua.is_cursor_active());
        cua.idle_ms = 61_000;
        assert!(!cua.is_visible());
        cua.reduced_motion = true;
        let plan = cua.plan_agent_cursor_move((420.0, 260.0));
        assert_eq!(plan.duration_ms, 0);
        assert_eq!(cua.cursor_pos, (420.0, 260.0));

        assert!(!should_trigger_recovery(&[1_000, 20_000, 40_000]));
        assert!(!should_trigger_recovery(&[1_000, 70_000, 140_000, 210_000]));
        assert!(should_trigger_recovery(&[1_000, 10_000, 20_000, 30_000]));
        assert!(is_external_url_blocked("https://evil.test/payload.js"));
        assert!(!is_external_url_blocked("data:text/html,<h1>Recovery</h1>"));
    }

    #[std::prelude::v1::test]
    fn desktop_os_surfaces_probe_and_gpui_builders_succeed() {
        let out = evaluate_desktop_os_surfaces_probe(&json!({
            "main_width_px": 700,
            "working_agents_count": 2,
            "attempt_close_main": true,
            "chat_window_agent_id": "a1",
            "island_current": {"mode": "question", "serverId": "local", "requestId": "r1"},
            "island_inline_reply": "Ship it now",
            "dock_seats_count": 6,
            "dock_idle_ms": 950,
            "browser_dock_island": {"kind": "reply", "question": false},
            "browser_dock_elapsed_ms": 6100,
            "cua_active": true,
            "crash_timestamps_ms": [1000, 15000, 30000, 45000],
            "recovery_request_url": "https://evil.test/x.js"
        }))
        .expect("probe succeeds");

        assert_eq!(out["main_window"]["auto_zoomed"], true);
        assert_eq!(out["main_window"]["close_guard_prompted"], true);
        assert_eq!(out["dynamic_island"]["priority"], 0);
        assert_eq!(out["agent_dock"]["tucked"], true);
        assert_eq!(out["browser_dock"]["open"], false);
        assert_eq!(out["cua"]["visible"], true);
        assert_eq!(out["recovery"]["triggered"], true);
        assert_eq!(out["recovery"]["external_url_blocked"], true);

        let palette = OpenBotPalette::dark();
        let state = DesktopOsSurfacesState::new();
        let _ = render_close_guard_dialog(&state.main_window, &palette);
        let _ = render_detached_chat_window_chrome(&state.chat_window, &palette, 0.0);
        let _ = render_dynamic_island(&state.dynamic_island, &palette, 0.0);
        let _ = render_agent_dock(&state.agent_dock, &palette, 0.0);
        let _ = render_browser_dock(&state.browser_dock, &palette);
        let _ = render_cua_overlay(&state.cua, &palette);
        let _ = render_recovery_surface(&state.recovery, &palette);
    }
}
