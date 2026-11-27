//! OpenBot Vertically-Splittable Right `SidePanel` (`OB-F16`).
//!
//! Mirrors `SidePanel.tsx`, `SettingsPanel.tsx`, `side-split-layout.ts`, and `panels-store.ts`:
//! - Exact geometry: `SETTINGS_PANEL_DEFAULT = 360.0`, `SETTINGS_PANEL_MAX = 1600.0`,
//!   `CONVERSATION_PANEL_MIN = 96.0`, `MAX_SIDE_SPLITS = 2`, `SIDE_SPLIT_MIN = 120.0`,
//!   `SIDE_MAIN_GROUP_MIN = 160.0`, `DRAG_START_PX = 4.0`
//! - Exact 6 dockable `SidePanelPaneKind`s: `Browser`, `Files`, `Threads`, `Settings`,
//!   `Question` (ephemeral <=20-turn tangent), `Subagent`
//! - Strict split bounds validation (`validate_split_config`), split/reorder/maximize/resize actions,
//!   and GPUI `0.2.2` `render_side_panel(state, palette, now_sec) -> Div`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::AvatarMood;
use openbot_contracts::limits::INPUT_LIMITS;
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{AgentAvatarProps, render_agent_avatar};
use crate::theme::openbot_palette::{
    OpenBotPalette, file_reference_badge, file_reference_tone, parse_css_color_to_rgba,
};
use crate::theme::{IconName, tg_icon_rgba};

/// Default and minimum right side panel width in logical pixels (`360px`).
pub const SETTINGS_PANEL_DEFAULT: f32 = 360.0;
/// Maximum right side panel width in logical pixels (`1600px`).
pub const SETTINGS_PANEL_MAX: f32 = 1600.0;
/// Minimum width reserved for the conversation pane when expanding the side panel (`96px`).
pub const CONVERSATION_PANEL_MIN: f32 = 96.0;
/// Maximum vertically stacked side-panel split areas (`2`).
pub const MAX_SIDE_SPLITS: usize = 2;
/// Minimum height in pixels for the lower split group (`120px`).
pub const SIDE_SPLIT_MIN: f32 = 120.0;
/// Minimum height in pixels for the upper main split group (`160px`).
pub const SIDE_MAIN_GROUP_MIN: f32 = 160.0;
/// Pointer drag distance threshold in pixels before initiating tab/pane drag (`4px`).
pub const DRAG_START_PX: f32 = 4.0;
/// Maximum embedded browser tabs inside the `Browser` side pane (`25`).
pub const SIDE_BROWSER_MAX_TABS: usize = INPUT_LIMITS.browser_tabs;
/// Maximum ephemeral turns inside the `Question` side pane (`20`).
pub const SIDE_QUESTION_MAX_TURNS: usize = INPUT_LIMITS.side_question_turns;
/// Maximum answer length for an ephemeral side question (`32_000`).
pub const SIDE_QUESTION_MAX_ANSWER: usize = INPUT_LIMITS.side_question_answer;
/// OpenBot right side-panel header height in logical pixels (`38px`).
pub const OPENBOT_SIDE_PANEL_HEADER_HEIGHT: f32 = 38.0;
/// Centered 4-button `SidePanelLauncher` stack max width (`320px`).
pub const OPENBOT_LAUNCHER_MAX_WIDTH: f32 = 320.0;
/// Centered 4-button `SidePanelLauncher` button height (`36px`).
pub const OPENBOT_LAUNCHER_BUTTON_HEIGHT: f32 = 36.0;

/// Context-aware target kind for the 4th OpenBot launcher button & `Settings` pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidePanelSettingsTarget {
    #[default]
    Assistant,
    Channel,
    Contact,
}

impl SidePanelSettingsTarget {
    pub const fn launcher_label(self, is_zh: bool) -> &'static str {
        match (self, is_zh) {
            (Self::Assistant, true) => "打开 助手设置",
            (Self::Assistant, false) => "Open Agent settings",
            (Self::Channel, true) => "打开 群聊设置",
            (Self::Channel, false) => "Open Channel settings",
            (Self::Contact, true) => "打开 联系人资料",
            (Self::Contact, false) => "Open Contact profile",
        }
    }

    pub const fn tab_label(self, is_zh: bool) -> &'static str {
        match (self, is_zh) {
            (Self::Assistant, true) => "助手设置",
            (Self::Assistant, false) => "Agent settings",
            (Self::Channel, true) => "群聊设置",
            (Self::Channel, false) => "Channel settings",
            (Self::Contact, true) => "联系人资料",
            (Self::Contact, false) => "Contact",
        }
    }
}

/// Canonical 4-button OpenBot `SidePanelLauncher` pane list (`image_2.png` 1:1).
pub const LAUNCHER_PRIMARY_PANES: [SidePanelPaneKind; 4] = [
    SidePanelPaneKind::Browser,
    SidePanelPaneKind::Files,
    SidePanelPaneKind::Threads,
    SidePanelPaneKind::Settings,
];

/// All 6 dockable OpenBot right side-panel panes (`SidePanel.tsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidePanelPaneKind {
    Browser,
    Files,
    Threads,
    Settings,
    Question,
    Subagent,
}

impl SidePanelPaneKind {
    pub const ALL: [Self; 6] = [
        Self::Browser,
        Self::Files,
        Self::Threads,
        Self::Settings,
        Self::Question,
        Self::Subagent,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Files => "files",
            Self::Threads => "threads",
            Self::Settings => "settings",
            Self::Question => "question",
            Self::Subagent => "subagent",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Browser => "Browser",
            Self::Files => "Files",
            Self::Threads => "Threads",
            Self::Settings => "Settings",
            Self::Question => "Question",
            Self::Subagent => "Subagent",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "browser" => Some(Self::Browser),
            "files" => Some(Self::Files),
            "threads" => Some(Self::Threads),
            "settings" => Some(Self::Settings),
            "question" => Some(Self::Question),
            "subagent" => Some(Self::Subagent),
            _ => None,
        }
    }

    pub const fn icon(self) -> IconName {
        match self {
            Self::Browser => IconName::ComputerDesktop,
            Self::Files => IconName::FileDoc,
            Self::Threads => IconName::Reply,
            Self::Settings => IconName::SettingsGear,
            Self::Question => IconName::Search,
            Self::Subagent => IconName::BotRobot,
        }
    }

    pub const fn launcher_label(self, target: SidePanelSettingsTarget, is_zh: bool) -> &'static str {
        match (self, is_zh) {
            (Self::Browser, true) => "打开 浏览器",
            (Self::Browser, false) => "Open Browser",
            (Self::Files, true) => "打开 文件",
            (Self::Files, false) => "Open Files",
            (Self::Threads, true) => "打开 话题",
            (Self::Threads, false) => "Open Threads",
            (Self::Settings, _) => target.launcher_label(is_zh),
            (Self::Question, true) => "打开 旁问",
            (Self::Question, false) => "Open Side question",
            (Self::Subagent, true) => "打开 子助手",
            (Self::Subagent, false) => "Open Subagent",
        }
    }

    pub const fn localized_tab_label(
        self,
        target: SidePanelSettingsTarget,
        is_zh: bool,
    ) -> &'static str {
        match (self, is_zh) {
            (Self::Browser, true) => "浏览器",
            (Self::Browser, false) => "Browser",
            (Self::Files, true) => "文件",
            (Self::Files, false) => "Files",
            (Self::Threads, true) => "话题",
            (Self::Threads, false) => "Threads",
            (Self::Settings, _) => target.tab_label(is_zh),
            (Self::Question, true) => "旁问",
            (Self::Question, false) => "Side question",
            (Self::Subagent, true) => "子助手",
            (Self::Subagent, false) => "Subagent",
        }
    }
}

/// Returns the canonical 4-button launcher pane order for the current peer target.
pub fn launcher_panes_for_target(_target: SidePanelSettingsTarget) -> Vec<SidePanelPaneKind> {
    LAUNCHER_PRIMARY_PANES.to_vec()
}

/// Strictly validates vertical split count (`<= 2`), lower split height (`>= 120px`),
/// and split share ratio (`0.0 < split_share < 1.0`).
pub fn validate_split_config(
    splits: &[SidePanelPaneKind],
    split_share: f64,
    split_height_px: Option<u32>,
) -> Result<(), String> {
    if splits.len() > MAX_SIDE_SPLITS {
        return Err(format!(
            "Side panel vertical split count {} exceeds maximum limit of {} splits.",
            splits.len(),
            MAX_SIDE_SPLITS
        ));
    }
    if let Some(height_px) = split_height_px
        && (height_px as f32) < SIDE_SPLIT_MIN
    {
        return Err(format!(
            "Side panel split height {}px is below minimum bounds of {}px.",
            height_px, SIDE_SPLIT_MIN as u32
        ));
    }
    if !split_share.is_finite() || split_share <= 0.0 || split_share >= 1.0 {
        return Err(format!(
            "Side panel split_share ({split_share}) is out of range: ratio must lie strictly within bounds (0.0, 1.0)."
        ));
    }
    Ok(())
}

/// Embedded browser tab inside the `Browser` side pane (`<= 25` tabs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideBrowserTab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub group_label: Option<String>,
    pub loading: bool,
}

/// Multi-tab browser state (`BrowserAddressRow`, `BrowserProfileSelect`, detach).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideBrowserPaneState {
    pub tabs: Vec<SideBrowserTab>,
    pub active_tab_id: Option<String>,
    pub address_input: String,
    pub profile_name: String,
    pub available_profiles: Vec<String>,
    pub detached: bool,
}

impl Default for SideBrowserPaneState {
    fn default() -> Self {
        let initial = SideBrowserTab {
            id: "tab-1".to_string(),
            title: "OpenBot Browser".to_string(),
            url: "https://openbot.local".to_string(),
            group_label: None,
            loading: false,
        };
        Self {
            active_tab_id: Some(initial.id.clone()),
            address_input: initial.url.clone(),
            tabs: vec![initial],
            profile_name: "Default".to_string(),
            available_profiles: vec!["Default".to_string(), "Work".to_string()],
            detached: false,
        }
    }
}

impl SideBrowserPaneState {
    pub fn open_tab(&mut self, tab: SideBrowserTab) -> Result<(), String> {
        if self.tabs.len() >= SIDE_BROWSER_MAX_TABS {
            return Err(format!(
                "Browser tab count exceeds maximum limit of {SIDE_BROWSER_MAX_TABS} tabs."
            ));
        }
        self.active_tab_id = Some(tab.id.clone());
        self.address_input = tab.url.clone();
        self.tabs.push(tab);
        Ok(())
    }

    pub fn close_tab(&mut self, tab_id: &str) -> bool {
        let before = self.tabs.len();
        self.tabs.retain(|t| t.id != tab_id);
        if self.tabs.len() == before {
            return false;
        }
        if self.active_tab_id.as_deref() == Some(tab_id) {
            self.active_tab_id = self.tabs.last().map(|t| t.id.clone());
            self.address_input = self
                .tabs
                .last()
                .map(|t| t.url.clone())
                .unwrap_or_default();
        }
        true
    }
}

/// Preview renderer mode for an open file tab inside the `Files` side pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideFilePreviewKind {
    Code,
    Markdown,
    Spreadsheet,
    Document,
    Media,
    Folder,
}

/// Open file preview tab inside the `Files` side pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideFileTab {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: SideFilePreviewKind,
    pub preview_lines: Vec<String>,
}

/// Multi-tab file preview state for the `Files` side pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideFilesPaneState {
    pub tabs: Vec<SideFileTab>,
    pub active_tab_id: Option<String>,
    pub current_folder: String,
}

impl Default for SideFilesPaneState {
    fn default() -> Self {
        let initial = SideFileTab {
            id: "file-readme".to_string(),
            name: "README.md".to_string(),
            path: "workspace/README.md".to_string(),
            kind: SideFilePreviewKind::Markdown,
            preview_lines: vec![
                "# Workspace Overview".to_string(),
                "Preview code, Markdown, XLSX/CSV, DOCX, and media tabs.".to_string(),
            ],
        };
        Self {
            active_tab_id: Some(initial.id.clone()),
            tabs: vec![initial],
            current_folder: "workspace".to_string(),
        }
    }
}

/// Side-thread summary entry displayed in the `Threads` pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideThreadItem {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub message_count: usize,
    pub unread: bool,
}

/// Draggable side-threads list state for the `Threads` pane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideThreadsPaneState {
    pub threads: Vec<SideThreadItem>,
    pub active_thread_id: Option<String>,
}

/// Inspector summary for the `Settings` pane (`AgentSettingsPanel` / `ChannelSettingsPane`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideSettingsPaneState {
    pub target_name: String,
    pub is_channel: bool,
    pub active_section: String,
    pub read_only: bool,
    pub isolated_context: bool,
}

impl Default for SideSettingsPaneState {
    fn default() -> Self {
        Self {
            target_name: "Agent Settings".to_string(),
            is_channel: false,
            active_section: "General".to_string(),
            read_only: false,
            isolated_context: false,
        }
    }
}

/// Single ephemeral Q&A turn in the `Question` side pane (`<= 20` turns, read-only tangent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideQuestionTurn {
    pub question: String,
    pub answer: String,
}

/// Ephemeral `<= 20`-turn tangent state for the `Question` side pane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideQuestionPaneState {
    pub turns: Vec<SideQuestionTurn>,
    pub draft_question: String,
}

impl SideQuestionPaneState {
    pub fn append_turn(
        &mut self,
        question: impl Into<String>,
        answer: impl Into<String>,
    ) -> Result<(), String> {
        if self.turns.len() >= SIDE_QUESTION_MAX_TURNS {
            return Err(format!(
                "Side question thread reached maximum limit of {SIDE_QUESTION_MAX_TURNS} turns."
            ));
        }
        let q = question.into();
        let a = answer.into();
        if q.trim().is_empty() {
            return Err("Side question cannot be blank.".to_string());
        }
        if a.chars().count() > SIDE_QUESTION_MAX_ANSWER {
            return Err(format!(
                "Side question answer exceeds maximum limit of {SIDE_QUESTION_MAX_ANSWER} characters."
            ));
        }
        self.turns.push(SideQuestionTurn {
            question: q,
            answer: a,
        });
        self.draft_question.clear();
        Ok(())
    }
}

/// Execution step row inside the `Subagent` inspector pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideSubagentStep {
    pub label: String,
    pub status: String,
    pub elapsed_ms: u64,
}

/// Live subagent step-by-step inspector state for the `Subagent` pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideSubagentPaneState {
    pub subagent_id: String,
    pub title: String,
    pub status: String,
    pub elapsed_sec: u64,
    pub steps: Vec<SideSubagentStep>,
}

impl Default for SideSubagentPaneState {
    fn default() -> Self {
        Self {
            subagent_id: "subagent-research".to_string(),
            title: "Subagent Inspector".to_string(),
            status: "idle".to_string(),
            elapsed_sec: 0,
            steps: Vec::new(),
        }
    }
}

/// Complete state for the vertically-splittable right `SidePanel` (`openbot:side-split-layout`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidePanelState {
    pub open: bool,
    pub width_px: f32,
    pub active_splits: Vec<SidePanelPaneKind>,
    pub lower_share: f64,
    pub maximized: bool,
    pub browser: SideBrowserPaneState,
    pub files: SideFilesPaneState,
    pub threads: SideThreadsPaneState,
    pub settings: SideSettingsPaneState,
    pub question: SideQuestionPaneState,
    pub subagent: SideSubagentPaneState,
}

impl Default for SidePanelState {
    fn default() -> Self {
        Self {
            open: true,
            width_px: SETTINGS_PANEL_DEFAULT,
            active_splits: vec![SidePanelPaneKind::Browser],
            lower_share: 0.5,
            maximized: false,
            browser: SideBrowserPaneState::default(),
            files: SideFilesPaneState::default(),
            threads: SideThreadsPaneState::default(),
            settings: SideSettingsPaneState::default(),
            question: SideQuestionPaneState::default(),
            subagent: SideSubagentPaneState::default(),
        }
    }
}

impl SidePanelState {
    pub fn new(active_splits: Vec<SidePanelPaneKind>) -> Result<Self, String> {
        validate_split_config(&active_splits, 0.5, None)?;
        Ok(Self {
            open: !active_splits.is_empty(),
            active_splits,
            ..Self::default()
        })
    }

    pub fn available_pane_ids() -> Vec<&'static str> {
        SidePanelPaneKind::ALL
            .iter()
            .map(|p| p.as_str())
            .collect()
    }

    pub fn active_split_ids(&self) -> Vec<&'static str> {
        self.active_splits.iter().map(|p| p.as_str()).collect()
    }

    /// Returns `true` when a pointer drag distance meets `DRAG_START_PX` (`4px`).
    pub fn drag_exceeds_threshold(delta_x: f32, delta_y: f32) -> bool {
        delta_x.hypot(delta_y) >= DRAG_START_PX
    }

    /// Opens or focuses `pane` in the upper main group.
    pub fn open_pane(&mut self, pane: SidePanelPaneKind) {
        self.open = true;
        if self.active_splits.is_empty() {
            self.active_splits.push(pane);
        } else if !self.active_splits.contains(&pane) {
            self.active_splits[0] = pane;
        }
    }

    /// Splits `pane` into the lower vertical group (`<= MAX_SIDE_SPLITS = 2`).
    pub fn split_down(&mut self, pane: SidePanelPaneKind) -> Result<(), String> {
        self.open = true;
        if let Some(pos) = self.active_splits.iter().position(|&p| p == pane) {
            if self.active_splits.len() == 2 && pos == 0 {
                self.active_splits.swap(0, 1);
            }
            return Ok(());
        }
        let mut next = self.active_splits.clone();
        next.push(pane);
        validate_split_config(&next, self.lower_share, None)?;
        self.active_splits = next;
        Ok(())
    }

    /// Moves `pane` up into the primary (upper) split slot.
    pub fn move_up(&mut self, pane: SidePanelPaneKind) -> bool {
        if let Some(pos) = self.active_splits.iter().position(|&p| p == pane)
            && pos > 0
        {
            self.active_splits.swap(pos - 1, pos);
            return true;
        }
        false
    }

    /// Moves `pane` down into the secondary (lower) split slot.
    pub fn move_down(&mut self, pane: SidePanelPaneKind) -> bool {
        if let Some(pos) = self.active_splits.iter().position(|&p| p == pane)
            && pos + 1 < self.active_splits.len()
        {
            self.active_splits.swap(pos, pos + 1);
            return true;
        }
        false
    }

    /// Removes `pane` from active splits; closes the side panel if none remain.
    pub fn close_pane(&mut self, pane: SidePanelPaneKind) -> bool {
        let before = self.active_splits.len();
        self.active_splits.retain(|&p| p != pane);
        if self.active_splits.is_empty() {
            self.open = false;
        }
        self.active_splits.len() != before
    }

    /// Removes `pane` from active splits while keeping `self.open == true` so an empty
    /// panel falls back cleanly to OpenBot's 4-button `SidePanelLauncher`.
    pub fn close_pane_keep_launcher(&mut self, pane: SidePanelPaneKind) -> bool {
        let before = self.active_splits.len();
        self.active_splits.retain(|&p| p != pane);
        if self.active_splits.is_empty() {
            self.maximized = false;
        }
        self.active_splits.len() != before
    }

    /// Opens the right `SidePanel` directly onto the empty 4-button `SidePanelLauncher`.
    pub fn open_empty_launcher(&mut self) {
        self.open = true;
        self.active_splits.clear();
        self.maximized = false;
    }

    /// Toggles maximized mode and returns the new maximized state.
    pub fn toggle_maximize(&mut self) -> bool {
        self.maximized = !self.maximized;
        self.maximized
    }

    /// Resizes width within `[360, 1600]` while keeping at least `CONVERSATION_PANEL_MIN = 96px`.
    pub fn resize_width(&mut self, requested_px: f32, viewport_width_px: Option<f32>) -> f32 {
        let max_by_viewport = viewport_width_px
            .map(|vw| (vw - CONVERSATION_PANEL_MIN).max(SETTINGS_PANEL_DEFAULT))
            .unwrap_or(SETTINGS_PANEL_MAX);
        let upper = SETTINGS_PANEL_MAX.min(max_by_viewport);
        self.width_px = requested_px.clamp(SETTINGS_PANEL_DEFAULT, upper);
        self.width_px
    }

    /// Updates `lower_share`, enforcing `(0.0, 1.0)` bounds and `120px`/`160px` vertical minimums.
    pub fn set_split_share(
        &mut self,
        share: f64,
        panel_height_px: Option<f32>,
    ) -> Result<f64, String> {
        let lower_height_px = panel_height_px.map(|h| (h as f64 * share).floor().max(0.0) as u32);
        validate_split_config(&self.active_splits, share, lower_height_px)?;
        if let Some(total_h) = panel_height_px {
            let upper_h = (total_h as f64) * (1.0 - share);
            if upper_h < SIDE_MAIN_GROUP_MIN as f64 {
                return Err(format!(
                    "Upper main split height ({upper_h:.0}px) is below minimum bounds of {}px.",
                    SIDE_MAIN_GROUP_MIN as u32
                ));
            }
        }
        self.lower_share = share;
        Ok(self.lower_share)
    }

    /// Serializes persisted split layout (`openbot:side-split-layout`).
    pub fn persisted_layout_json(&self) -> serde_json::Value {
        serde_json::json!({
            "areas": self.active_split_ids(),
            "lower": self.lower_share,
            "widthPx": self.width_px,
            "maximized": self.maximized,
        })
    }
}

fn palette_color(palette: &OpenBotPalette, token: &str, fallback: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback))
}

fn render_browser_pane(browser: &SideBrowserPaneState, palette: &OpenBotPalette) -> Div {
    let control = palette_color(palette, "--openbot-bg-control", 0x212121);
    let active_bg = palette_color(palette, "--openbot-bg-control-active", 0x2a2a2a);
    let canvas_bg = palette_color(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = palette_color(palette, "--openbot-accent-soft", 0x1a304d);

    let mut tab_strip = div()
        .id("openbot-side-browser-tabs")
        .flex()
        .items_center()
        .gap(px(4.0))
        .min_w(px(0.0))
        .overflow_x_scroll();

    for (idx, tab) in browser.tabs.iter().enumerate() {
        let is_active = browser.active_tab_id.as_deref() == Some(tab.id.as_str());
        tab_strip = tab_strip.child(
            div()
                .id(("openbot-side-browser-tab", idx))
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(if is_active { active_bg } else { control })
                .border_1()
                .border_color(if is_active { accent } else { border })
                .when_some(tab.group_label.as_ref(), |el, grp| {
                    el.child(
                        div()
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .bg(accent_soft)
                            .text_xs()
                            .text_color(accent)
                            .child(grp.clone()),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_active { text_primary } else { text_muted })
                        .max_w(px(120.0))
                        .truncate()
                        .child(tab.title.clone()),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(px(6.0))
        .p(px(8.0))
        .child(tab_strip)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(canvas_bg)
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(text_primary)
                        .truncate()
                        .child(browser.address_input.clone()),
                )
                .child(
                    div()
                        .px(px(7.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(accent_soft)
                        .text_xs()
                        .text_color(accent)
                        .child(browser.profile_name.clone()),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(64.0))
                .rounded(px(10.0))
                .bg(canvas_bg)
                .border_1()
                .border_color(border)
                .p(px(10.0))
                .flex()
                .flex_col()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::ComputerDesktop, 14.0, accent))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_primary)
                                .child(if browser.detached {
                                    "Detached Browser Window Active"
                                } else {
                                    "Embedded Playwright Live Viewport"
                                }),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(text_muted)
                        .child(format!(
                            "{}/{} tabs · {} profiles",
                            browser.tabs.len(),
                            SIDE_BROWSER_MAX_TABS,
                            browser.available_profiles.len()
                        )),
                ),
        )
}

fn render_files_pane(files: &SideFilesPaneState, palette: &OpenBotPalette) -> Div {
    let control = palette_color(palette, "--openbot-bg-control", 0x212121);
    let active_bg = palette_color(palette, "--openbot-bg-control-active", 0x2a2a2a);
    let canvas_bg = palette_color(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);

    let mut tab_strip = div()
        .id("openbot-side-files-tabs")
        .flex()
        .items_center()
        .gap(px(4.0))
        .min_w(px(0.0))
        .overflow_x_scroll();

    for (idx, file_tab) in files.tabs.iter().enumerate() {
        let is_active = files.active_tab_id.as_deref() == Some(file_tab.id.as_str());
        let tone = file_reference_tone(&file_tab.name);
        let (fg_hex, _) = tone.colors(palette.scheme);
        let tone_fg = parse_css_color_to_rgba(fg_hex).unwrap_or(accent);
        let badge = file_reference_badge(&file_tab.name).unwrap_or_else(|| "FILE".to_string());

        tab_strip = tab_strip.child(
            div()
                .id(("openbot-side-file-tab", idx))
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(if is_active { active_bg } else { control })
                .border_1()
                .border_color(if is_active { tone_fg } else { border })
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tone_fg)
                        .child(badge),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if is_active { text_primary } else { text_muted })
                        .child(file_tab.name.clone()),
                ),
        );
    }

    let active_file = files
        .active_tab_id
        .as_deref()
        .and_then(|id| files.tabs.iter().find(|t| t.id == id))
        .or_else(|| files.tabs.first());

    let mut preview = div()
        .id("openbot-side-file-preview")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .rounded(px(10.0))
        .bg(canvas_bg)
        .border_1()
        .border_color(border)
        .p(px(8.0))
        .flex()
        .flex_col()
        .gap(px(3.0));

    if let Some(file) = active_file {
        preview = preview.child(
            div()
                .text_xs()
                .text_color(text_muted)
                .child(format!("{} · {:?}", file.path, file.kind)),
        );
        for (line_idx, line) in file.preview_lines.iter().enumerate() {
            preview = preview.child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(22.0))
                            .text_xs()
                            .text_color(text_muted)
                            .child(format!("{}", line_idx + 1)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_primary)
                            .child(line.clone()),
                    ),
            );
        }
    } else {
        preview = preview.child(
            div()
                .text_xs()
                .text_color(text_muted)
                .child(format!("Folder: {} (no open files)", files.current_folder)),
        );
    }

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(px(6.0))
        .p(px(8.0))
        .child(tab_strip)
        .child(preview)
}

fn render_threads_pane(threads: &SideThreadsPaneState, palette: &OpenBotPalette) -> Div {
    let canvas_bg = palette_color(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);

    let mut list = div()
        .id("openbot-side-threads-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .gap(px(5.0))
        .p(px(8.0));

    if threads.threads.is_empty() {
        list = list.child(
            div()
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(canvas_bg)
                .border_1()
                .border_color(border)
                .text_xs()
                .text_color(text_muted)
                .child("No side threads yet. Drag a conversation thread tab here to dock it."),
        );
    } else {
        for (idx, item) in threads.threads.iter().enumerate() {
            let active = threads.active_thread_id.as_deref() == Some(item.id.as_str());
            list = list.child(
                div()
                    .id(("openbot-side-thread-row", idx))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .p(px(8.0))
                    .rounded(px(10.0))
                    .bg(canvas_bg)
                    .border_1()
                    .border_color(if active { accent } else { border })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_primary)
                                    .truncate()
                                    .child(item.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if item.unread { accent } else { text_muted })
                                    .child(format!("{} msgs", item.message_count)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .truncate()
                            .child(item.preview.clone()),
                    ),
            );
        }
    }

    div().flex().flex_col().flex_1().min_h(px(0.0)).child(list)
}

fn render_settings_pane(settings: &SideSettingsPaneState, palette: &OpenBotPalette) -> Div {
    let canvas_bg = palette_color(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let warning = palette_color(palette, "--openbot-warning", 0xff9412);
    let warning_soft = palette_color(palette, "--openbot-warning-soft", 0x3a2b16);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);

    let sections: [&str; 4] = if settings.is_channel {
        ["Members & Lead", "Browser Policy", "Channel Memories", "Routines & Archive"]
    } else {
        ["Permissions", "Folders", "Backup Models (<=8)", "Advanced"]
    };

    let mut body = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(px(6.0))
        .p(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(settings.target_name.clone()),
                )
                .when(settings.read_only, |el| {
                    el.child(
                        div()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(999.0))
                            .bg(warning_soft)
                            .text_xs()
                            .text_color(warning)
                            .child("Archived · Read-only"),
                    )
                }),
        );

    for (idx, sec) in sections.iter().enumerate() {
        let is_active = settings.active_section.eq_ignore_ascii_case(sec);
        body = body.child(
            div()
                .id(("openbot-side-settings-sec", idx))
                .flex()
                .items_center()
                .justify_between()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(canvas_bg)
                .border_1()
                .border_color(if is_active { accent } else { border })
                .child(div().text_xs().text_color(text_primary).child(*sec))
                .child(tg_icon_rgba(IconName::ChevronRight, 11.0, text_muted)),
        );
    }

    body.child(
        div()
            .text_xs()
            .text_color(text_muted)
            .child(if settings.isolated_context {
                "Isolated context: Enabled"
            } else {
                "Isolated context: Shared"
            }),
    )
}

fn render_question_pane(question: &SideQuestionPaneState, palette: &OpenBotPalette) -> Div {
    let canvas_bg = palette_color(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = palette_color(palette, "--openbot-accent-soft", 0x1a304d);

    let mut transcript = div()
        .id("openbot-side-question-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .gap(px(6.0));

    if question.turns.is_empty() {
        transcript = transcript.child(
            div()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(canvas_bg)
                .text_xs()
                .text_color(text_muted)
                .child("Ask an ephemeral side question without mutating main conversation history."),
        );
    } else {
        for turn in &question.turns {
            transcript = transcript.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .p(px(8.0))
                    .rounded(px(8.0))
                    .bg(canvas_bg)
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(accent)
                            .child(format!("Q: {}", turn.question)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_primary)
                            .child(turn.answer.clone()),
                    ),
            );
        }
    }

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(px(6.0))
        .p(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child("Ephemeral Side Question"),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(999.0))
                        .bg(accent_soft)
                        .text_xs()
                        .text_color(accent)
                        .child(format!("{}/{} turns", question.turns.len(), SIDE_QUESTION_MAX_TURNS)),
                ),
        )
        .child(transcript)
        .child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(canvas_bg)
                .border_1()
                .border_color(border)
                .text_xs()
                .text_color(if question.draft_question.is_empty() {
                    text_muted
                } else {
                    text_primary
                })
                .child(if question.draft_question.is_empty() {
                    "Ask a quick side question…".to_string()
                } else {
                    question.draft_question.clone()
                }),
        )
}

fn render_subagent_pane(
    subagent: &SideSubagentPaneState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let canvas_bg = palette_color(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let success = palette_color(palette, "--openbot-success", 0x31cf76);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);

    let mood = match subagent.status.as_str() {
        "running" => AvatarMood::Working,
        "failed" => AvatarMood::Failed,
        "finished" | "completed" => AvatarMood::Responded,
        _ => AvatarMood::Idle,
    };
    let avatar_props = AgentAvatarProps::new(&subagent.subagent_id)
        .with_size(24.0)
        .with_mood(mood);

    let mut steps_list = div()
        .id("openbot-side-subagent-steps")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .gap(px(4.0));

    if subagent.steps.is_empty() {
        steps_list = steps_list.child(
            div()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(canvas_bg)
                .text_xs()
                .text_color(text_muted)
                .child("Select a running or completed subagent to inspect its execution trace."),
        );
    } else {
        for (idx, step) in subagent.steps.iter().enumerate() {
            let dot_color = if step.status == "running" {
                accent
            } else {
                success
            };
            steps_list = steps_list.child(
                div()
                    .id(("openbot-subagent-step", idx))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(8.0))
                    .bg(canvas_bg)
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().w(px(6.0)).h(px(6.0)).rounded(px(999.0)).bg(dot_color))
                            .child(div().text_xs().text_color(text_primary).child(step.label.clone())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .child(format!("{}ms", step.elapsed_ms)),
                    ),
            );
        }
    }

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(px(6.0))
        .p(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(render_agent_avatar(&avatar_props, None, now_sec))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child(subagent.title.clone()),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(text_muted)
                        .child(format!("{} · {}s", subagent.status, subagent.elapsed_sec)),
                ),
        )
        .child(steps_list)
}

fn render_pane_content(
    kind: SidePanelPaneKind,
    state: &SidePanelState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    match kind {
        SidePanelPaneKind::Browser => render_browser_pane(&state.browser, palette),
        SidePanelPaneKind::Files => render_files_pane(&state.files, palette),
        SidePanelPaneKind::Threads => render_threads_pane(&state.threads, palette),
        SidePanelPaneKind::Settings => render_settings_pane(&state.settings, palette),
        SidePanelPaneKind::Question => render_question_pane(&state.question, palette),
        SidePanelPaneKind::Subagent => render_subagent_pane(&state.subagent, palette, now_sec),
    }
}

/// Renders the centered OpenBot 4-button `SidePanelLauncher` stack (`image_2.png` 1:1).
pub fn render_side_panel_launcher_preview(
    target: SidePanelSettingsTarget,
    palette: &OpenBotPalette,
    is_zh: bool,
) -> Div {
    let btn_bg = palette_color(palette, "--openbot-bg-control-hover", 0x242424);
    let btn_hover = palette_color(palette, "--openbot-bg-control-active", 0x2a2a2a);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);

    let mut stack = div()
        .id("openbot-side-panel-launcher")
        .w_full()
        .max_w(px(OPENBOT_LAUNCHER_MAX_WIDTH))
        .mx_auto()
        .p(px(24.0))
        .flex()
        .flex_col()
        .gap(px(6.0));

    for (idx, pane) in launcher_panes_for_target(target).into_iter().enumerate() {
        stack = stack.child(
            div()
                .id(("openbot-side-launcher-btn", idx))
                .w_full()
                .h(px(OPENBOT_LAUNCHER_BUTTON_HEIGHT))
                .px(px(16.0))
                .rounded(px(8.0))
                .bg(btn_bg)
                .hover(move |s| s.bg(btn_hover))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_start()
                .gap(px(12.0))
                .child(tg_icon_rgba(pane.icon(), 14.0, text_muted))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_primary)
                        .child(pane.launcher_label(target, is_zh)),
                ),
        );
    }

    div()
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .child(stack)
}

/// Renders the complete vertically-splittable right `SidePanel` with active-pane header tab bar,
/// centered 4-button `SidePanelLauncher` when open with zero active tabs, vertical split handle
/// (`MAX_SIDE_SPLITS = 2`, `160px` / `120px` minimums), maximize/restore and close controls,
/// and concrete visual renderers for all 6 dockable pane kinds.
pub fn render_side_panel(
    state: &SidePanelState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    if !state.open {
        return div().w(px(0.0)).h_full();
    }

    let canvas = palette_color(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let control = palette_color(palette, "--openbot-bg-control", 0x212121);
    let control_active = palette_color(palette, "--openbot-bg-control-active", 0x2a2a2a);
    let border = palette.border_hairline();
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_secondary = palette_color(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);

    let mut shell = div()
        .h_full()
        .flex()
        .flex_row()
        .bg(canvas)
        .border_l_1()
        .border_color(border);

    if state.maximized {
        shell = shell.flex_1().min_w(px(SETTINGS_PANEL_DEFAULT));
    } else {
        shell = shell.w(px(state
            .width_px
            .clamp(SETTINGS_PANEL_DEFAULT, SETTINGS_PANEL_MAX)));
    }

    if state.active_splits.is_empty() {
        let target = if state.settings.is_channel {
            SidePanelSettingsTarget::Channel
        } else {
            SidePanelSettingsTarget::Assistant
        };
        return shell
            .child(
                div()
                    .id("openbot-side-panel-resizer")
                    .w(px(DRAG_START_PX))
                    .h_full()
                    .cursor_col_resize(),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(render_side_panel_launcher_preview(target, palette, true)),
            );
    }

    let mut header_tabs = div()
        .id("openbot-side-panel-tabs")
        .flex()
        .items_center()
        .gap(px(4.0))
        .min_w(px(0.0))
        .flex_1()
        .overflow_x_scroll();

    for (idx, pane_kind) in state.active_splits.iter().copied().enumerate() {
        let is_primary = idx == 0;
        header_tabs = header_tabs.child(
            div()
                .id(("openbot-side-pane-tab", idx))
                .h(px(28.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .bg(if is_primary { control_active } else { control })
                .child(tg_icon_rgba(
                    pane_kind.icon(),
                    14.0,
                    if is_primary { text_primary } else { text_muted },
                ))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_primary {
                            text_primary
                        } else {
                            text_secondary
                        })
                        .child(pane_kind.label()),
                ),
        );
    }

    let header = div()
        .h(px(OPENBOT_SIDE_PANEL_HEADER_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(4.0))
        .px(px(8.0))
        .border_b_1()
        .border_color(border)
        .child(header_tabs)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .id("openbot-side-split-btn")
                        .px(px(6.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(control)
                        .text_xs()
                        .text_color(text_muted)
                        .child(if state.active_splits.len() >= MAX_SIDE_SPLITS {
                            "Split 2/2"
                        } else {
                            "Split ↓"
                        }),
                )
                .child(
                    div()
                        .id("openbot-side-max-btn")
                        .px(px(6.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(control)
                        .text_xs()
                        .text_color(text_muted)
                        .child(if state.maximized { "Restore" } else { "Max" }),
                )
                .child(
                    div()
                        .id("openbot-side-close-btn")
                        .p(px(4.0))
                        .rounded(px(6.0))
                        .bg(control)
                        .child(tg_icon_rgba(IconName::Close, 12.0, text_muted)),
                ),
        );

    let mut stack = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0));

    let primary = state.active_splits[0];
    stack = stack.child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(SIDE_MAIN_GROUP_MIN))
            .child(render_pane_content(primary, state, palette, now_sec)),
    );

    if let Some(&secondary) = state.active_splits.get(1) {
        let lower_pct = (state.lower_share.clamp(0.05, 0.95) * 100.0).round() as u32;
        stack = stack
            .child(
                div()
                    .id("openbot-side-split-handle")
                    .h(px(18.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(control)
                    .border_t_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_muted)
                            .child(format!("{} · {}% split", secondary.label(), lower_pct)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().text_xs().text_color(text_muted).child("↑ Up"))
                            .child(div().text_xs().text_color(text_muted).child("↓ Down")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(SIDE_SPLIT_MIN))
                    .child(render_pane_content(secondary, state, palette, now_sec)),
            );
    }

    shell
        .child(
            div()
                .id("openbot-side-panel-resizer")
                .w(px(DRAG_START_PX))
                .h_full()
                .cursor_col_resize(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(header)
                .child(stack),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn openbot_side_panel_launcher_four_buttons_and_targets() {
        let panes = launcher_panes_for_target(SidePanelSettingsTarget::Assistant);
        assert_eq!(
            panes,
            vec![
                SidePanelPaneKind::Browser,
                SidePanelPaneKind::Files,
                SidePanelPaneKind::Threads,
                SidePanelPaneKind::Settings,
            ]
        );
        assert_eq!(
            SidePanelPaneKind::Browser.launcher_label(SidePanelSettingsTarget::Assistant, true),
            "打开 浏览器"
        );
        assert_eq!(
            SidePanelPaneKind::Files.launcher_label(SidePanelSettingsTarget::Assistant, true),
            "打开 文件"
        );
        assert_eq!(
            SidePanelPaneKind::Threads.launcher_label(SidePanelSettingsTarget::Assistant, true),
            "打开 话题"
        );
        assert_eq!(
            SidePanelPaneKind::Settings.launcher_label(SidePanelSettingsTarget::Assistant, true),
            "打开 助手设置"
        );
        assert_eq!(
            SidePanelPaneKind::Settings.launcher_label(SidePanelSettingsTarget::Channel, true),
            "打开 群聊设置"
        );
        assert_eq!(
            SidePanelPaneKind::Settings.launcher_label(SidePanelSettingsTarget::Contact, true),
            "打开 联系人资料"
        );
    }

    #[std::prelude::v1::test]
    fn openbot_side_panel_resize_split_and_close_to_launcher() {
        let mut st = SidePanelState::new(vec![SidePanelPaneKind::Settings]).expect("valid state");
        assert!(st.open);
        assert_eq!(st.resize_width(200.0, Some(1200.0)), SETTINGS_PANEL_DEFAULT);
        assert_eq!(st.resize_width(2400.0, Some(1200.0)), 1200.0 - CONVERSATION_PANEL_MIN);
        assert_eq!(st.resize_width(2400.0, Some(3000.0)), SETTINGS_PANEL_MAX);

        assert!(st.split_down(SidePanelPaneKind::Files).is_ok());
        assert_eq!(st.active_splits.len(), MAX_SIDE_SPLITS);
        assert!(st.split_down(SidePanelPaneKind::Browser).is_err());
        assert!(st.set_split_share(0.1, Some(500.0)).is_err());
        assert!(st.set_split_share(0.4, Some(500.0)).is_ok());

        assert!(st.close_pane_keep_launcher(SidePanelPaneKind::Files));
        assert!(st.close_pane_keep_launcher(SidePanelPaneKind::Settings));
        assert!(st.open);
        assert!(st.active_splits.is_empty());
        assert!(!st.maximized);
    }
}
