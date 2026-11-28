//! 1:1 OpenBot `GlobalSearch` (+ `GlobalSearchPlush`), `FirstAgentSetup`, and `AgentSettingsPanel` (`OB-F14`).
//!
//! Mirrors upstream `nightly-labs/openbot`:
//! - `packages/ui/src/components/{GlobalSearch.tsx, GlobalSearchPlush.tsx}`
//! - `packages/ui/src/features/agents/{FirstAgentSetup.tsx, AgentSettingsPanel.tsx, agent-setting-links.ts}`

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::agents::{
    AGENT_BACKUP_MODEL_LIMIT, AGENT_FOLDER_LIMIT, AgentAccess, AgentModelChoice,
    AgentReasoningEffort, AgentSummary, BusyMessageMode,
};
use openbot_contracts::avatar::{
    AVATAR_COLOR_CHOICES, AvatarColorChoice, AvatarHue, AvatarMood, AvatarStyle, BloubExpression,
    BloubShape, PlushTransition, avatar_seed_for_face,
};
use openbot_contracts::limits::INPUT_LIMITS;
use openbot_contracts::providers::AgentProviderId;
use serde::{Deserialize, Serialize};

pub use crate::components::bloub_avatar::GlobalSearchPlushState;
use crate::components::bloub_avatar::{
    AgentAvatarEditorState, AgentAvatarProps, EDITOR_COLOR_TOTAL_CELLS, GLOBAL_SEARCH_DART_MS,
    GLOBAL_SEARCH_GAZE_REACH, GLOBAL_SEARCH_IDLE_MS, GLOBAL_SEARCH_LOOK_PX, GLOBAL_SEARCH_TILT_MS,
    render_agent_avatar, render_avatar_editor_popover,
};
use crate::i18n::tr_str;
use crate::theme::openbot_palette::{
    CHAT_COLOR_PRESETS, OpenBotPalette, parse_css_color_to_rgba,
};
use crate::theme::{IconName, tg_icon_rgba};
use crate::views::sidebar::sidebar_search_view::split_highlight_segments;

use super::shell_state::{AGENT_SETTINGS_MIN_PX, ShellValidationError};

/// Public alias matching OpenBot `ReasoningEffort` (`none..ultra`, zero temperature).
pub type ReasoningEffort = AgentReasoningEffort;

/// GlobalSearch debounce window in milliseconds (`GlobalSearch.tsx`).
pub const GLOBAL_SEARCH_DEBOUNCE_MS: u64 = 150;
/// Maximum results shown per category on the `All` tab (`ALL_SECTION_LIMIT = 5`).
pub const GLOBAL_SEARCH_ALL_SECTION_LIMIT: usize = 5;
/// Row height for each search hit (`RESULT_HEIGHT = 36px`).
pub const GLOBAL_SEARCH_RESULT_HEIGHT_PX: f32 = 36.0;
/// Section header height (`SECTION_HEIGHT = 28px`).
pub const GLOBAL_SEARCH_SECTION_HEIGHT_PX: f32 = 28.0;
/// Spaced section header height (`SPACED_SECTION_HEIGHT = 34px`).
pub const GLOBAL_SEARCH_SPACED_SECTION_HEIGHT_PX: f32 = 34.0;
/// Plush idle head-tilt angle in degrees (`-10deg`).
pub const GLOBAL_SEARCH_TILT_DEG: f64 = -10.0;

/// Exact 6 `GlobalSearch` tabs (`SEARCH_TABS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalSearchTab {
    #[default]
    All,
    Agents,
    Channels,
    Messages,
    Files,
    Routines,
}

impl GlobalSearchTab {
    pub const ALL: [Self; 6] = [
        Self::All,
        Self::Agents,
        Self::Channels,
        Self::Messages,
        Self::Files,
        Self::Routines,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Agents => "agents",
            Self::Channels => "channels",
            Self::Messages => "messages",
            Self::Files => "files",
            Self::Routines => "routines",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Agents => "Agents",
            Self::Channels => "Channels",
            Self::Messages => "Messages",
            Self::Files => "Files",
            Self::Routines => "Routines",
        }
    }

    pub fn localized_label(self, lang_id: &str) -> &'static str {
        if lang_id.starts_with("zh") {
            match self {
                Self::All => "全部",
                Self::Agents => "助手",
                Self::Channels => "群聊",
                Self::Messages => "消息",
                Self::Files => "文件",
                Self::Routines => "定时任务",
            }
        } else {
            self.label()
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|t| t.as_str() == lower)
    }
}

/// Categorized result sections inside `GlobalSearch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GlobalSearchSectionKind {
    Agents,
    Channels,
    Messages,
    Files,
    Routines,
    Actions,
    Settings,
}

impl GlobalSearchSectionKind {
    pub const ALL: [Self; 7] = [
        Self::Agents,
        Self::Channels,
        Self::Messages,
        Self::Files,
        Self::Routines,
        Self::Actions,
        Self::Settings,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Agents => "Agents",
            Self::Channels => "Channels",
            Self::Messages => "Messages",
            Self::Files => "Files",
            Self::Routines => "Routines",
            Self::Actions => "Actions",
            Self::Settings => "Settings",
        }
    }

    pub fn localized_label(self, lang_id: &str) -> &'static str {
        if lang_id.starts_with("zh") {
            match self {
                Self::Agents => "助手",
                Self::Channels => "群聊",
                Self::Messages => "消息",
                Self::Files => "文件",
                Self::Routines => "定时任务",
                Self::Actions => "快捷操作",
                Self::Settings => "设置",
            }
        } else {
            self.label()
        }
    }

    pub const fn matches_tab(self, tab: GlobalSearchTab) -> bool {
        match tab {
            GlobalSearchTab::All => true,
            GlobalSearchTab::Agents => matches!(self, Self::Agents),
            GlobalSearchTab::Channels => matches!(self, Self::Channels),
            GlobalSearchTab::Messages => matches!(self, Self::Messages),
            GlobalSearchTab::Files => matches!(self, Self::Files),
            GlobalSearchTab::Routines => matches!(self, Self::Routines),
        }
    }

    pub const fn singular_kind(self) -> &'static str {
        match self {
            Self::Agents => "agent",
            Self::Channels => "channel",
            Self::Messages => "message",
            Self::Files => "file",
            Self::Routines => "routine",
            Self::Actions => "action",
            Self::Settings => "settings",
        }
    }
}

/// Single search hit rendered inside `GlobalSearch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSearchResultItem {
    pub id: String,
    pub section: GlobalSearchSectionKind,
    pub title: String,
    pub subtitle: String,
    pub badge: Option<String>,
    pub avatar_seed: Option<String>,
    pub shortcut_hint: Option<String>,
}

/// Deterministic telemetry snapshot of `GlobalSearchPlush` gaze and idle scan timings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GlobalSearchPlushMetrics {
    pub gaze_x: f64,
    pub gaze_y: f64,
    pub idle_ms: u64,
    pub look_px: u32,
    pub dart_ms: u32,
    pub tilt_ms: u32,
    pub tilt_deg: f64,
    pub animated: bool,
}

/// State controller for `Cmd/Ctrl+K` `GlobalSearch` and its interactive `GlobalSearchPlush`.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalSearchState {
    pub open: bool,
    pub active_tab: GlobalSearchTab,
    pub query: String,
    pub debounced_query: String,
    pub last_input_ms: u64,
    pub searching: bool,
    pub caret_x: f64,
    pub selected_index: usize,
    pub results: Vec<GlobalSearchResultItem>,
    pub plush: GlobalSearchPlushState,
    pub remote_request_seq: u64,
    pub remote_stale_discarded: u64,
    pub remote_error: Option<String>,
    pub last_activated_target_id: Option<String>,
}

/// Single-source-of-truth geometry snapshot for `GlobalSearch` (`min(640px, vw - 48px)`, `top = 112px`,
/// `48px` input bar, `36px` rows/footer, `56px` empty-state plush).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalSearchModalLayout {
    pub width_px: f32,
    pub top_px: f32,
    pub list_max_h_px: f32,
    pub bg_rgb: u32,
    pub bg_hex: &'static str,
    pub ring_border: bool,
    pub input_height_px: f32,
    pub footer_height_px: f32,
    pub footer_hints: [&'static str; 4],
    pub row_height_px: f32,
    pub section_header_height_px: f32,
    pub row_avatar_px: f32,
    pub plush_size_px: f32,
}

pub fn compute_global_search_modal_layout(
    viewport_w: f32,
    viewport_h: f32,
    is_dark: bool,
) -> GlobalSearchModalLayout {
    let vw = if viewport_w.is_finite() && viewport_w > 0.0 {
        viewport_w
    } else {
        1440.0
    };
    let vh = if viewport_h.is_finite() && viewport_h > 0.0 {
        viewport_h
    } else {
        900.0
    };
    let width_px = 640.0_f32.min((vw - 48.0).max(0.0));
    let top_px = if vh >= 240.0 {
        112.0
    } else {
        (vh * 0.14).max(16.0)
    };
    let list_max_h_px = (vh - top_px - 120.0).clamp(180.0, 400.0);
    GlobalSearchModalLayout {
        width_px,
        top_px,
        list_max_h_px,
        bg_rgb: if is_dark { 0x1E1E1E } else { 0xFFFFFF },
        bg_hex: if is_dark { "#1e1e1e" } else { "#ffffff" },
        ring_border: true,
        input_height_px: 48.0,
        footer_height_px: 36.0,
        footer_hints: ["↑↓ Navigate", "↵ Open", "Tab Filter", "Esc Close"],
        row_height_px: GLOBAL_SEARCH_RESULT_HEIGHT_PX,
        section_header_height_px: GLOBAL_SEARCH_SECTION_HEIGHT_PX,
        row_avatar_px: 20.0,
        plush_size_px: 56.0,
    }
}

/// View-model for a single visible `GlobalSearch` row (shared by GPUI rendering & runtime inspection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalSearchRowViewModel {
    pub id: String,
    pub kind: &'static str,
    pub title: String,
    pub subtitle: String,
    pub selected: bool,
    pub kbd_enter_visible: bool,
    pub avatar_px: f32,
    pub highlighted_marks: Vec<String>,
}

impl Default for GlobalSearchState {
    fn default() -> Self {
        Self::new(false)
    }
}

impl GlobalSearchState {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            open: false,
            active_tab: GlobalSearchTab::All,
            query: String::new(),
            debounced_query: String::new(),
            last_input_ms: 0,
            searching: false,
            caret_x: 0.0,
            selected_index: 0,
            results: Vec::new(),
            plush: GlobalSearchPlushState::new(reduced_motion),
            remote_request_seq: 0,
            remote_stale_discarded: 0,
            remote_error: None,
            last_activated_target_id: None,
        }
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.plush.set_reduced_motion(reduced_motion);
    }

    pub fn set_query(&mut self, query: impl Into<String>, caret_x: f64, now_ms: u64) {
        self.query = query.into();
        self.caret_x = if caret_x.is_finite() { caret_x } else { 0.0 };
        self.last_input_ms = now_ms;
        self.selected_index = 0;
        self.plush
            .update_caret(self.caret_x, 0.0, 0.0, 0.0, now_ms as f64);
    }

    pub fn set_caret_x(&mut self, caret_x: f64, now_ms: u64) {
        self.caret_x = if caret_x.is_finite() { caret_x } else { 0.0 };
        self.plush
            .update_caret(self.caret_x, 0.0, 0.0, 0.0, now_ms as f64);
    }

    pub fn set_searching(&mut self, searching: bool, now_ms: u64) {
        self.searching = searching;
        self.plush.set_searching(searching, now_ms as f64);
    }

    pub fn should_flush_debounce(&self, now_ms: u64) -> bool {
        self.query != self.debounced_query
            && now_ms.saturating_sub(self.last_input_ms) >= GLOBAL_SEARCH_DEBOUNCE_MS
    }

    pub fn flush_debounce(&mut self) {
        self.debounced_query = self.query.clone();
    }

    /// Starts a remote search request when the debounce window has elapsed (returns monotonic sequence ID).
    pub fn begin_remote_search(&mut self, now_ms: u64) -> Option<u64> {
        if !self.should_flush_debounce(now_ms) || self.query.trim().is_empty() {
            return None;
        }
        self.flush_debounce();
        self.remote_request_seq = self.remote_request_seq.saturating_add(1);
        self.remote_error = None;
        self.set_searching(true, now_ms);
        Some(self.remote_request_seq)
    }

    /// Applies remote search results only when `seq` matches the latest in-flight sequence (drops stale responses).
    pub fn apply_remote_results(
        &mut self,
        seq: u64,
        items: Vec<GlobalSearchResultItem>,
    ) -> bool {
        if seq != self.remote_request_seq {
            self.remote_stale_discarded = self.remote_stale_discarded.saturating_add(1);
            return false;
        }
        self.results = items;
        self.selected_index = 0;
        self.searching = false;
        self.remote_error = None;
        true
    }

    /// Records a remote search failure for the active request sequence.
    pub fn fail_remote_search(&mut self, seq: u64, err: impl Into<String>) -> bool {
        if seq != self.remote_request_seq {
            self.remote_stale_discarded = self.remote_stale_discarded.saturating_add(1);
            return false;
        }
        self.searching = false;
        self.remote_error = Some(err.into());
        true
    }

    /// Retries the last debounced remote search query with a new monotonic sequence ID.
    pub fn retry_remote_search(&mut self, now_ms: u64) -> u64 {
        self.remote_request_seq = self.remote_request_seq.saturating_add(1);
        self.remote_error = None;
        self.set_searching(true, now_ms);
        self.remote_request_seq
    }

    /// Activates the currently selected result, recording `last_activated_target_id` and closing the palette.
    pub fn execute_selected(&mut self) -> Option<GlobalSearchResultItem> {
        let item = self.selected_result()?.clone();
        self.last_activated_target_id = Some(item.id.clone());
        self.close_palette();
        Some(item)
    }

    /// Single source of truth for whether the `56px` `GlobalSearchPlush` empty state is shown.
    pub fn is_plush_visible(&self) -> bool {
        self.flat_filtered_results().is_empty()
    }

    /// Percentage caret gaze offsets matching upstream `app-shell.css:L423-L424` (`±7%` X, `±5%` Y).
    pub fn plush_gaze_offset_pct(&self) -> (f64, f64) {
        let m = self.plush_metrics();
        (m.gaze_x * 7.0, m.gaze_y * 5.0)
    }

    /// Builds visible row view-models (highlight marks, `20px` avatar, `↵` kbd on selected row).
    pub fn build_visible_row_view_models(&self) -> Vec<GlobalSearchRowViewModel> {
        let flat = self.flat_filtered_results();
        let active_idx = if flat.is_empty() {
            0
        } else {
            self.selected_index.min(flat.len() - 1)
        };
        flat.into_iter()
            .enumerate()
            .map(|(idx, item)| {
                let mut marks = Vec::new();
                if !self.query.trim().is_empty() {
                    let (_, hit_title, _) = split_highlight_segments(&item.title, &self.query);
                    if !hit_title.is_empty() {
                        marks.push(hit_title);
                    }
                    let (_, hit_sub, _) = split_highlight_segments(&item.subtitle, &self.query);
                    if !hit_sub.is_empty() {
                        marks.push(hit_sub);
                    }
                }
                let selected = idx == active_idx;
                GlobalSearchRowViewModel {
                    id: item.id.clone(),
                    kind: item.section.singular_kind(),
                    title: item.title.clone(),
                    subtitle: item.subtitle.clone(),
                    selected,
                    kbd_enter_visible: selected,
                    avatar_px: 20.0,
                    highlighted_marks: marks,
                }
            })
            .collect()
    }

    /// Exact `GlobalSearchPlush` telemetry (`gaze_x = (caret_x / 160).clamp(-1, 1)`, `idle_ms = 500`, `look_px = 4`).
    /// Under `reduced_motion`, gaze & idle head tilt are strictly zeroed (`C10` fix).
    pub fn plush_metrics(&self) -> GlobalSearchPlushMetrics {
        if self.plush.reduced_motion {
            return GlobalSearchPlushMetrics {
                gaze_x: 0.0,
                gaze_y: 0.0,
                idle_ms: GLOBAL_SEARCH_IDLE_MS,
                look_px: GLOBAL_SEARCH_LOOK_PX as u32,
                dart_ms: GLOBAL_SEARCH_DART_MS as u32,
                tilt_ms: GLOBAL_SEARCH_TILT_MS as u32,
                tilt_deg: 0.0,
                animated: false,
            };
        }
        let gaze_x = if self.caret_x.is_finite() {
            (self.caret_x / GLOBAL_SEARCH_GAZE_REACH).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        GlobalSearchPlushMetrics {
            gaze_x,
            gaze_y: 0.0,
            idle_ms: GLOBAL_SEARCH_IDLE_MS,
            look_px: GLOBAL_SEARCH_LOOK_PX as u32,
            dart_ms: GLOBAL_SEARCH_DART_MS as u32,
            tilt_ms: GLOBAL_SEARCH_TILT_MS as u32,
            tilt_deg: GLOBAL_SEARCH_TILT_DEG,
            animated: true,
        }
    }

    /// Groups search results by section, enforcing `ALL_SECTION_LIMIT = 5` on `GlobalSearchTab::All`.
    pub fn filtered_sections(&self) -> Vec<(GlobalSearchSectionKind, Vec<&GlobalSearchResultItem>)> {
        let q = self.query.trim().to_ascii_lowercase();
        let limit = if self.active_tab == GlobalSearchTab::All {
            GLOBAL_SEARCH_ALL_SECTION_LIMIT
        } else {
            usize::MAX
        };
        let mut out = Vec::new();
        for section in GlobalSearchSectionKind::ALL {
            if !section.matches_tab(self.active_tab) {
                continue;
            }
            let items: Vec<&GlobalSearchResultItem> = self
                .results
                .iter()
                .filter(|item| item.section == section)
                .filter(|item| {
                    q.is_empty()
                        || item.title.to_ascii_lowercase().contains(&q)
                        || item.subtitle.to_ascii_lowercase().contains(&q)
                })
                .take(limit)
                .collect();
            if !items.is_empty() {
                out.push((section, items));
            }
        }
        out
    }

    /// Opens the floating command palette and resets filter/query/selection cleanly.
    pub fn open_palette(&mut self) {
        self.open = true;
        self.active_tab = GlobalSearchTab::All;
        self.query.clear();
        self.debounced_query.clear();
        self.caret_x = 0.0;
        self.selected_index = 0;
        self.searching = false;
    }

    /// Opens the floating command palette only if no blocking modal is currently active (`UI-F07`).
    pub fn try_open_palette(&mut self, modal_open: bool) -> bool {
        if modal_open {
            self.open = false;
            self.searching = false;
            return false;
        }
        self.open_palette();
        true
    }

    /// Closes the floating command palette.
    pub fn close_palette(&mut self) {
        self.open = false;
        self.searching = false;
    }

    /// Returns flattened visible results across sections respecting `ALL_SECTION_LIMIT = 5`.
    pub fn flat_filtered_results(&self) -> Vec<&GlobalSearchResultItem> {
        self.filtered_sections()
            .into_iter()
            .flat_map(|(_, items)| items)
            .collect()
    }

    /// Wraps keyboard selection (`ArrowUp`/`ArrowDown`) across visible results (`shouldFocusWrap = true`).
    pub fn move_selection(&mut self, delta: isize) {
        let len = self.flat_filtered_results().len();
        if len == 0 {
            self.selected_index = 0;
            return;
        }
        let len_i = len as isize;
        let cur = (self.selected_index.min(len - 1)) as isize;
        let next = (cur + delta.rem_euclid(len_i)).rem_euclid(len_i) as usize;
        self.selected_index = next;
    }

    /// Cycles the active filter tab forward (`Tab`) or backward (`Shift+Tab`).
    pub fn cycle_tab(&mut self, forward: bool) {
        let all = GlobalSearchTab::ALL;
        let cur = all
            .iter()
            .position(|&t| t == self.active_tab)
            .unwrap_or(0);
        let next = if forward {
            (cur + 1) % all.len()
        } else {
            (cur + all.len() - 1) % all.len()
        };
        self.active_tab = all[next];
        self.selected_index = 0;
    }

    /// Returns the currently highlighted result hit, if any.
    pub fn selected_result(&self) -> Option<&GlobalSearchResultItem> {
        let flat = self.flat_filtered_results();
        if flat.is_empty() {
            return None;
        }
        let idx = self.selected_index.min(flat.len() - 1);
        flat.get(idx).copied()
    }

    /// Returns the 1-indexed result for `Cmd/Ctrl+1..9` direct jump (`GlobalSearch.tsx`).
    pub fn result_at_shortcut_digit(&self, digit_1_to_9: usize) -> Option<&GlobalSearchResultItem> {
        if !(1..=9).contains(&digit_1_to_9) {
            return None;
        }
        let flat = self.flat_filtered_results();
        flat.get(digit_1_to_9 - 1).copied()
    }

    /// Appends typed text to `query` and updates `GlobalSearchPlush` caret tracking.
    pub fn push_query_text(&mut self, text: &str, now_ms: u64) {
        if text.is_empty() {
            return;
        }
        let mut next = self.query.clone();
        next.push_str(text);
        let caret = (next.chars().count() as f64) * 8.0;
        self.set_query(next, caret, now_ms);
    }

    /// Removes the last Unicode scalar from `query` and updates `GlobalSearchPlush` caret tracking.
    pub fn backspace_query(&mut self, now_ms: u64) {
        let mut next = self.query.clone();
        if next.pop().is_some() {
            let caret = (next.chars().count() as f64) * 8.0;
            self.set_query(next, caret, now_ms);
        }
    }

    /// Clears `query` and resets selection + caret gaze.
    pub fn clear_query(&mut self, now_ms: u64) {
        self.set_query(String::new(), 0.0, now_ms);
    }
}

fn pal(palette: &OpenBotPalette, token: &str, fallback: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback))
}

/// Renders the `Cmd/Ctrl+K` `GlobalSearch` modal (`640px`, `16px` radius, `48px` top input bar +
/// inline sliding-pill tabs, `36px` categorized rows with `Enter` `<Kbd>`, `56px` `GlobalSearchPlush`
/// empty state, and `36px` keyboard footer).
pub fn render_global_search_modal(
    state: &mut GlobalSearchState,
    palette: &OpenBotPalette,
    now_ms: f64,
) -> Div {
    let dark = palette.scheme.is_dark();
    let modal_layout = compute_global_search_modal_layout(1440.0, 900.0, dark);
    let card_bg = rgb(modal_layout.bg_rgb);
    let visible_rows = state.build_visible_row_view_models();
    let ring = palette
        .rgba("--openbot-dialog-ring")
        .unwrap_or_else(|| palette.border_strong_hairline());
    let divider = palette.border();
    let hover = palette.hover();
    let active_bg = palette.bg_control_active();
    let kbd_bg = palette.bg_control_hover();
    let text_pri = palette.text_primary();
    let text_sec = palette.text_secondary();
    let text_muted = palette.text_muted();

    let sections = state.filtered_sections();

    let mut tab_strip = div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .flex_shrink_0();
    for tab in GlobalSearchTab::ALL {
        let active = state.active_tab == tab;
        tab_strip = tab_strip.child(
            div()
                .id(SharedString::from(format!("openbot-gs-tab-{}", tab.as_str())))
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .when(active, |el| el.bg(active_bg))
                .flex()
                .items_center()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if active { text_pri } else { text_muted })
                .hover(move |s| if active { s } else { s.text_color(text_sec) })
                .child(tab.label()),
        );
    }

    let mut list_body = div()
        .id("openbot-global-search-results")
        .flex_1()
        .min_h(px(0.0))
        .max_h(px(400.0))
        .overflow_y_scroll()
        .p(px(6.0))
        .flex()
        .flex_col();

    let mut flat_idx = 0usize;
    for (sec_idx, (sec_kind, items)) in sections.iter().enumerate() {
        let header_h = if sec_idx == 0 {
            GLOBAL_SEARCH_SECTION_HEIGHT_PX
        } else {
            GLOBAL_SEARCH_SPACED_SECTION_HEIGHT_PX
        };
        list_body = list_body.child(
            div()
                .h(px(header_h))
                .px(px(8.0))
                .pb(px(4.0))
                .flex()
                .items_end()
                .justify_between()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_muted)
                .child(sec_kind.label())
                .child(items.len().to_string()),
        );
        for item in items {
            let row_vm = visible_rows.get(flat_idx);
            let selected = row_vm.map_or(flat_idx == state.selected_index, |vm| vm.selected);
            let kbd_enter_visible = row_vm.map_or(selected, |vm| vm.kbd_enter_visible);
            let avatar_px = row_vm.map_or(20.0, |vm| vm.avatar_px);
            flat_idx += 1;
            let leading: AnyElement = match item.section {
                GlobalSearchSectionKind::Agents | GlobalSearchSectionKind::Messages => {
                    let seed = item.avatar_seed.as_deref().unwrap_or(item.id.as_str());
                    crate::components::avatar::render_plush_bot_avatar(
                        seed,
                        None,
                        None,
                        None,
                        avatar_px,
                        AvatarMood::Idle,
                        None,
                    )
                    .into_any_element()
                }
                GlobalSearchSectionKind::Channels => div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_muted)
                    .child("#")
                    .into_any_element(),
                GlobalSearchSectionKind::Files => div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon_rgba(IconName::FileDoc, 15.0, text_muted))
                    .into_any_element(),
                GlobalSearchSectionKind::Routines => div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon_rgba(IconName::ClockSending, 15.0, text_muted))
                    .into_any_element(),
                GlobalSearchSectionKind::Actions => div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon_rgba(IconName::ComposePen, 14.0, text_muted))
                    .into_any_element(),
                GlobalSearchSectionKind::Settings => div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon_rgba(IconName::SettingsGear, 15.0, text_muted))
                    .into_any_element(),
            };
            list_body = list_body.child(
                div()
                    .id(SharedString::from(format!("openbot-gs-row-{}", item.id)))
                    .h(px(GLOBAL_SEARCH_RESULT_HEIGHT_PX))
                    .flex_shrink_0()
                    .w_full()
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .when(selected, |el| el.bg(active_bg))
                    .hover(move |s| if selected { s } else { s.bg(hover) })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(leading)
                            .child(
                                div()
                                    .max_w(px(220.0))
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_pri)
                                    .child(item.title.clone()),
                            )
                            .when(!item.subtitle.is_empty(), |el| {
                                el.child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(13.5))
                                        .text_color(text_muted)
                                        .child(item.subtitle.clone()),
                                )
                            }),
                    )
                    .when(kbd_enter_visible, |el| {
                        el.child(
                            div()
                                .h(px(20.0))
                                .px(px(5.0))
                                .rounded(px(4.0))
                                .bg(kbd_bg)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(11.0))
                                .text_color(text_muted)
                                .child("↵"),
                        )
                    }),
            );
        }
    }

    if state.is_plush_visible() || flat_idx == 0 {
        let empty_plush = state.plush.render_element(now_ms, modal_layout.plush_size_px);
        list_body = list_body.child(
            div()
                .min_h(px(148.0))
                .py(px(12.0))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .child(empty_plush)
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(text_muted)
                        .child(if state.searching {
                            "Searching…"
                        } else {
                            "No results found"
                        }),
                ),
        );
    }

    let kbd_chip = |label: &'static str| {
        div()
            .h(px(20.0))
            .px(px(5.0))
            .rounded(px(4.0))
            .bg(active_bg)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(text_sec)
            .child(label)
    };

    div()
        .w(px(modal_layout.width_px))
        .rounded(px(16.0))
        .bg(card_bg)
        .border_1()
        .border_color(ring)
        .shadow_lg()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .h(px(modal_layout.input_height_px))
                .flex_shrink_0()
                .w_full()
                .pl(px(16.0))
                .pr(px(8.0))
                .border_b_1()
                .border_color(divider)
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(tg_icon_rgba(IconName::Search, 16.0, text_muted))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(15.0))
                        .text_color(if state.query.is_empty() {
                            text_muted
                        } else {
                            text_pri
                        })
                        .child(if state.query.is_empty() {
                            "Search".to_string()
                        } else {
                            state.query.clone()
                        }),
                )
                .child(tab_strip),
        )
        .child(list_body)
        .child(
            div()
                .h(px(modal_layout.footer_height_px))
                .flex_shrink_0()
                .w_full()
                .px(px(16.0))
                .border_t_1()
                .border_color(divider)
                .flex()
                .items_center()
                .gap(px(16.0))
                .text_size(px(13.0))
                .text_color(text_muted)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(kbd_chip("↑"))
                        .child(kbd_chip("↓"))
                        .child("Navigate"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(kbd_chip("↵"))
                        .child("Open"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(kbd_chip("Tab"))
                        .child("Filter"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(kbd_chip("Esc"))
                        .child("Close"),
                ),
        )
}

/// Exact number of curated faces shown in `FirstAgentSetup` (`FirstAgentSetup.tsx`).
pub const FIRST_AGENT_FACE_COUNT: usize = 12;
/// Eye optical scale multiplier for `FirstAgentSetup` face cards (`FACE_CHOICE_EYE_SCALE = 1.25`).
pub const FACE_CHOICE_EYE_SCALE: f64 = 1.25;
/// Exact color choice swatch count in `FirstAgentSetup` (`"A"` auto + 16 explicit swatches = `17`).
pub const FIRST_AGENT_COLOR_CHOICES_COUNT: usize = 17;
/// Exact number of momentum-draggable starter suggestions (`6`).
pub const FIRST_AGENT_SUGGESTIONS_COUNT: usize = 6;
/// Maximum allowed purpose / instruction length in `FirstAgentSetup` (`32_000`).
pub const FIRST_AGENT_PURPOSE_MAX_CHARS: usize = INPUT_LIMITS.agent_instructions_file;
/// Momentum carousel friction (`0.88`), drag threshold (`8px`), max velocity (`1.75`).
pub const SUGGESTION_MOMENTUM_FRICTION: f32 = 0.88;
pub const SUGGESTION_DRAG_THRESHOLD_PX: f32 = 8.0;
pub const SUGGESTION_MAX_VELOCITY: f32 = 1.75;

/// Canonical `FirstAgentSetup` Create-Agent Studio geometry (`first-agent-setup.css:L7-L150`).
pub const FIRST_AGENT_FORM_WIDTH_PX: f32 = 420.0;
pub const FIRST_AGENT_HEADER_HEIGHT_PX: f32 = 38.0;
pub const FIRST_AGENT_HEADER_AVATAR_PX: f32 = 18.0;
pub const FIRST_AGENT_HERO_SIZE_PX: f32 = 72.0;
pub const FIRST_AGENT_SWATCH_COLS: usize = 9;
pub const FIRST_AGENT_SWATCH_ROWS: usize = 2;
pub const FIRST_AGENT_SWATCH_SIZE_PX: f32 = 18.0;
pub const FIRST_AGENT_FACE_BTN_PX: f32 = 33.0;
pub const FIRST_AGENT_FACE_PREVIEW_PX: f32 = 24.0;
pub const FIRST_AGENT_PURPOSE_MIN_HEIGHT_PX: f32 = 64.0;
pub const FIRST_AGENT_MODEL_ROW_HEIGHT_PX: f32 = 44.0;
pub const FIRST_AGENT_CTA_MIN_WIDTH_PX: f32 = 132.0;
pub const FIRST_AGENT_REC_CARD_WIDTH_PX: f32 = 256.0;
pub const FIRST_AGENT_REC_CARD_HEIGHT_PX: f32 = 112.0;
pub const FIRST_AGENT_REC_CARD_AVATAR_PX: f32 = 42.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FirstAgentSetupMode {
    #[default]
    First,
    Additional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstAgentFaceChoice {
    pub shape: BloubShape,
    pub expression: BloubExpression,
    pub label: &'static str,
}

/// Exact 12 curated onboarding faces (`FIRST_AGENT_FACES`).
pub const FIRST_AGENT_FACES: [FirstAgentFaceChoice; FIRST_AGENT_FACE_COUNT] = [
    FirstAgentFaceChoice { shape: BloubShape::Squircle, expression: BloubExpression::Heureux, label: "Happy Squircle" },
    FirstAgentFaceChoice { shape: BloubShape::Cercle, expression: BloubExpression::Curieux, label: "Curious Circle" },
    FirstAgentFaceChoice { shape: BloubShape::Galet, expression: BloubExpression::Excite, label: "Excited Pebble" },
    FirstAgentFaceChoice { shape: BloubShape::Capsule, expression: BloubExpression::Attentif, label: "Attentive Capsule" },
    FirstAgentFaceChoice { shape: BloubShape::Nuage, expression: BloubExpression::Neutre, label: "Calm Cloud" },
    FirstAgentFaceChoice { shape: BloubShape::Hexagone, expression: BloubExpression::Fier, label: "Proud Hexagon" },
    FirstAgentFaceChoice { shape: BloubShape::Triangle, expression: BloubExpression::Surpris, label: "Bright Triangle" },
    FirstAgentFaceChoice { shape: BloubShape::Pentagone, expression: BloubExpression::Hilare, label: "Joyful Pentagon" },
    FirstAgentFaceChoice { shape: BloubShape::Carre, expression: BloubExpression::Timide, label: "Gentle Square" },
    FirstAgentFaceChoice { shape: BloubShape::Losange, expression: BloubExpression::Confus, label: "Puzzled Diamond" },
    FirstAgentFaceChoice { shape: BloubShape::Fantome, expression: BloubExpression::Excite, label: "Playful Ghost" },
    FirstAgentFaceChoice { shape: BloubShape::Poulpe, expression: BloubExpression::Curieux, label: "Clever Octopus" },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstAgentSuggestion {
    pub id: &'static str,
    pub title: &'static str,
    pub default_name: &'static str,
    pub description: &'static str,
    pub prompt: &'static str,
}

/// Exact 6 momentum-draggable onboarding suggestions (`inbox-helper..learning-coach`).
pub const FIRST_AGENT_SUGGESTIONS: [FirstAgentSuggestion; FIRST_AGENT_SUGGESTIONS_COUNT] = [
    FirstAgentSuggestion {
        id: "inbox-helper",
        title: "Inbox Helper",
        default_name: "Inbox Helper",
        description: "Triage emails, surface urgent threads, and draft crisp replies.",
        prompt: "Help me triage my inbox, summarize urgent threads, and draft polite replies.",
    },
    FirstAgentSuggestion {
        id: "trip-planner",
        title: "Trip Planner",
        default_name: "Trip Planner",
        description: "Build day-by-day itineraries, compare flights, and track bookings.",
        prompt: "Plan trips with day-by-day itineraries, transit options, and local food picks.",
    },
    FirstAgentSuggestion {
        id: "personal-organizer",
        title: "Personal Organizer",
        default_name: "Personal Organizer",
        description: "Coordinate daily tasks, reminders, and recurring routines.",
        prompt: "Keep my tasks, calendar routines, and weekly goals organized.",
    },
    FirstAgentSuggestion {
        id: "shopping-scout",
        title: "Shopping Scout",
        default_name: "Shopping Scout",
        description: "Compare specs, reviews, and prices before every purchase.",
        prompt: "Research products, compare verified specs and prices, and spot trade-offs.",
    },
    FirstAgentSuggestion {
        id: "writing-partner",
        title: "Writing Partner",
        default_name: "Writing Partner",
        description: "Outline, edit, and sharpen memos, docs, and long-form drafts.",
        prompt: "Collaborate with me on outlines, clear prose, and editorial feedback.",
    },
    FirstAgentSuggestion {
        id: "learning-coach",
        title: "Learning Coach",
        default_name: "Learning Coach",
        description: "Break down tough concepts with active practice and study plans.",
        prompt: "Teach me new subjects step by step with concise examples and quizzes.",
    },
];

/// Finds one of the 6 onboarding suggestions by canonical slug (`"inbox-helper"`..`"learning-coach"`).
pub fn find_first_agent_suggestion(id: &str) -> Option<&'static FirstAgentSuggestion> {
    let trimmed = id.trim();
    FIRST_AGENT_SUGGESTIONS.iter().find(|s| s.id == trimmed)
}

/// State model for `FirstAgentSetup` (zero starter bots, 18 providers, 8 reasoning efforts, ZERO temperature).
#[derive(Debug, Clone, PartialEq)]
pub struct FirstAgentSetupState {
    pub mode: FirstAgentSetupMode,
    pub name: String,
    pub purpose: String,
    pub selected_face_index: usize,
    pub avatar_seed: String,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub auto_color: bool,
    pub provider: AgentProviderId,
    pub model: String,
    pub reasoning_effort: AgentReasoningEffort,
    pub matched_suggestion_id: Option<String>,
    pub carousel_offset_px: f32,
    pub carousel_velocity: f32,
    pub generating_profile: bool,
    pub error_message: Option<String>,
    pub purpose_over_limit_len: Option<usize>,
}

impl Default for FirstAgentSetupState {
    fn default() -> Self {
        Self::first_run()
    }
}

impl FirstAgentSetupState {
    pub fn first_run() -> Self {
        Self::new(FirstAgentSetupMode::First)
    }

    pub fn new(mode: FirstAgentSetupMode) -> Self {
        let face = FIRST_AGENT_FACES[0];
        let seed = avatar_seed_for_face("new-agent", face.shape, face.expression, AvatarStyle::Plush)
            .unwrap_or_else(|_| "new-agent".to_string());
        Self {
            mode,
            name: String::new(),
            purpose: String::new(),
            selected_face_index: 0,
            avatar_seed: seed,
            avatar_hue: None,
            avatar_color: None,
            auto_color: true,
            provider: AgentProviderId::Codex,
            model: "gpt-6-luna".to_string(),
            reasoning_effort: AgentReasoningEffort::Low,
            matched_suggestion_id: None,
            carousel_offset_px: 0.0,
            carousel_velocity: 0.0,
            generating_profile: false,
            error_message: None,
            purpose_over_limit_len: None,
        }
    }

    pub const fn can_cancel_with_escape(&self) -> bool {
        matches!(self.mode, FirstAgentSetupMode::Additional)
    }

    pub fn select_face(&mut self, index: usize) {
        let clamped = index.min(FIRST_AGENT_FACE_COUNT - 1);
        self.selected_face_index = clamped;
        let face = FIRST_AGENT_FACES[clamped];
        if let Ok(seed) =
            avatar_seed_for_face("new-agent", face.shape, face.expression, AvatarStyle::Plush)
        {
            self.avatar_seed = seed;
        }
    }

    pub fn select_auto_color(&mut self) {
        self.auto_color = true;
        self.avatar_hue = None;
        self.avatar_color = None;
    }

    pub fn select_color_choice(&mut self, choice: AvatarColorChoice) {
        self.auto_color = false;
        match choice {
            AvatarColorChoice::Hue { hue, .. } => {
                self.avatar_hue = Some(hue);
                self.avatar_color = None;
            }
            AvatarColorChoice::Preset { hue, color, .. } => {
                self.avatar_hue = hue;
                self.avatar_color = Some(color.to_string());
            }
        }
    }

    pub fn select_provider(&mut self, provider: AgentProviderId) {
        self.provider = provider;
        let default_model = provider.descriptor().default_model;
        if !default_model.is_empty() {
            self.model = default_model.to_string();
        }
    }

    pub fn select_suggestion(&mut self, suggestion_id: &str) -> bool {
        let Some(suggestion) = find_first_agent_suggestion(suggestion_id) else {
            return false;
        };
        self.matched_suggestion_id = Some(suggestion.id.to_string());
        if self.name.trim().is_empty() {
            self.name = suggestion.default_name.to_string();
        }
        self.purpose = suggestion.prompt.to_string();
        self.purpose_over_limit_len = None;
        self.error_message = None;
        true
    }

    pub fn set_purpose(&mut self, purpose: impl Into<String>) -> Result<(), ShellValidationError> {
        let text = purpose.into();
        let len = text.chars().count();
        if len > FIRST_AGENT_PURPOSE_MAX_CHARS {
            let err = ShellValidationError::new(
                "setup_purpose_too_long",
                format!(
                    "setup purpose instruction is too long ({len}): maximum limit is {FIRST_AGENT_PURPOSE_MAX_CHARS} characters"
                ),
            );
            self.matched_suggestion_id = None;
            self.purpose_over_limit_len = Some(len);
            self.error_message = Some(err.message.clone());
            return Err(err);
        }
        // Manual edit clears matched starter template badge (`C11` fix).
        self.matched_suggestion_id = None;
        self.purpose = text;
        self.purpose_over_limit_len = None;
        self.error_message = None;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ShellValidationError> {
        if self.name.trim().is_empty() {
            return Err(ShellValidationError::new(
                "agent_name_empty",
                "agent name cannot be empty",
            ));
        }
        let name_len = self.name.chars().count();
        if name_len > INPUT_LIMITS.agent_name {
            return Err(ShellValidationError::new(
                "agent_name_too_long",
                format!(
                    "agent name length ({name_len}) exceeds limit of {} characters",
                    INPUT_LIMITS.agent_name
                ),
            ));
        }
        if let Some(over_len) = self.purpose_over_limit_len {
            return Err(ShellValidationError::new(
                "setup_purpose_too_long",
                format!(
                    "setup purpose instruction is too long ({over_len}): maximum limit is {FIRST_AGENT_PURPOSE_MAX_CHARS} characters"
                ),
            ));
        }
        let purpose_len = self.purpose.chars().count();
        if purpose_len > FIRST_AGENT_PURPOSE_MAX_CHARS {
            return Err(ShellValidationError::new(
                "setup_purpose_too_long",
                format!(
                    "setup purpose instruction is too long ({purpose_len}): maximum limit is {FIRST_AGENT_PURPOSE_MAX_CHARS} characters"
                ),
            ));
        }
        Ok(())
    }

    /// Returns the active swatch index in the 18-cell (`9x2`) grid (`0` = Auto, `1..=17` = `AVATAR_COLOR_CHOICES`).
    pub fn selected_swatch_index(&self) -> usize {
        if self.auto_color {
            return 0;
        }
        AVATAR_COLOR_CHOICES
            .iter()
            .position(|choice| match *choice {
                AvatarColorChoice::Hue { hue, .. } => {
                    self.avatar_color.is_none() && self.avatar_hue == Some(hue)
                }
                AvatarColorChoice::Preset { color, .. } => {
                    self.avatar_color.as_deref() == Some(color)
                }
            })
            .map_or(0, |idx| idx + 1)
    }

    pub fn handle_escape(&mut self) -> bool {
        if self.can_cancel_with_escape() {
            self.purpose_over_limit_len = None;
            self.error_message = None;
            true
        } else {
            false
        }
    }

    pub fn record_backend_error(&mut self, msg: impl Into<String>) {
        self.generating_profile = false;
        self.error_message = Some(msg.into());
    }

    pub fn build_create_managed_bot_request(
        &self,
    ) -> Result<gpui_telegram_proto::miniapp_contracts::CreateFabushiBotRequest, ShellValidationError>
    {
        self.validate()?;
        let face = FIRST_AGENT_FACES[self.selected_face_index.min(FIRST_AGENT_FACE_COUNT - 1)];
        Ok(gpui_telegram_proto::miniapp_contracts::CreateFabushiBotRequest {
            display_name: self.name.trim().to_string(),
            description: self.purpose.trim().to_string(),
            avatar_shape: Some(format!("plush:{}", face.shape.as_str())),
            avatar_color: self.avatar_color.clone(),
            preferred_model: Some(self.model.clone()),
            thinking_level: Some(self.reasoning_effort.as_str().to_string()),
            ..Default::default()
        })
    }

    /// Advances the suggestion strip momentum (`friction = 0.88`, `max_velocity = 1.75`).
    pub fn step_suggestion_momentum(&mut self, delta_px: f32) {
        if !delta_px.is_finite() {
            return;
        }
        if !self.carousel_offset_px.is_finite() {
            self.carousel_offset_px = 0.0;
        }
        if !self.carousel_velocity.is_finite() {
            self.carousel_velocity = 0.0;
        }
        if delta_px.abs() >= SUGGESTION_DRAG_THRESHOLD_PX {
            self.carousel_velocity =
                (delta_px / 16.0).clamp(-SUGGESTION_MAX_VELOCITY, SUGGESTION_MAX_VELOCITY);
        }
        self.carousel_offset_px = (self.carousel_offset_px + self.carousel_velocity * 16.0)
            .clamp(-640.0, 0.0);
        self.carousel_velocity *= SUGGESTION_MOMENTUM_FRICTION;
        if self.carousel_velocity.abs() < 0.01 {
            self.carousel_velocity = 0.0;
        }
    }
}

/// Single-source-of-truth layout snapshot for `FirstAgentSetup` Create-Agent Studio (`UI-F08`).
#[derive(Debug, Clone, PartialEq)]
pub struct FirstAgentSetupLayoutSnapshot {
    pub form_width_px: f32,
    pub canvas_bg_hex: String,
    pub header_height_px: f32,
    pub header_avatar_px: f32,
    pub header_live_name: String,
    pub header_cancel_visible: bool,
    pub hero_size_px: f32,
    pub hero_is_plush_3d: bool,
    pub hero_pointer_follow: bool,
    pub hero_shape: &'static str,
    pub swatch_cols: usize,
    pub swatch_rows: usize,
    pub swatch_total: usize,
    pub swatch_px: f32,
    pub swatch_selected_idx: usize,
    pub swatch_selection_ring_css: String,
    pub shape_count: usize,
    pub face_btn_px: f32,
    pub face_preview_px: f32,
    pub shape_eye_scale: f64,
    pub shapes_contain_goutte: bool,
    pub field_name: String,
    pub field_purpose: String,
    pub purpose_min_height_px: f32,
    pub model_row_height_px: f32,
    pub submit_label: &'static str,
    pub submit_bg_hex: String,
    pub submit_fg_hex: String,
    pub cta_min_width_px: f32,
    pub submit_disabled: bool,
    pub error_message: Option<String>,
    pub rec_total_templates: usize,
    pub rec_visible_cards: usize,
    pub rec_card_w_px: f32,
    pub rec_card_h_px: f32,
    pub rec_card_avatar_px: f32,
    pub rec_suggestion_id: Option<String>,
    pub rec_hydrated: bool,
}

pub fn compute_first_agent_setup_layout(
    state: &FirstAgentSetupState,
    palette: &OpenBotPalette,
    lang_id: &str,
) -> FirstAgentSetupLayoutSnapshot {
    let face = FIRST_AGENT_FACES[state.selected_face_index.min(FIRST_AGENT_FACE_COUNT - 1)];
    let accent_hex = palette.token("--openbot-accent-text").unwrap_or("#42a0ff");
    let live_name = if state.name.trim().is_empty() {
        tr_str(lang_id, "新建助手", "New agent").to_string()
    } else {
        state.name.trim().to_string()
    };
    FirstAgentSetupLayoutSnapshot {
        form_width_px: FIRST_AGENT_FORM_WIDTH_PX,
        canvas_bg_hex: palette
            .token("--openbot-bg-canvas")
            .unwrap_or("#1a1a1a")
            .to_string(),
        header_height_px: FIRST_AGENT_HEADER_HEIGHT_PX,
        header_avatar_px: FIRST_AGENT_HEADER_AVATAR_PX,
        header_live_name: live_name,
        header_cancel_visible: state.can_cancel_with_escape(),
        hero_size_px: FIRST_AGENT_HERO_SIZE_PX,
        hero_is_plush_3d: true,
        hero_pointer_follow: true,
        hero_shape: face.shape.as_str(),
        swatch_cols: FIRST_AGENT_SWATCH_COLS,
        swatch_rows: FIRST_AGENT_SWATCH_ROWS,
        swatch_total: EDITOR_COLOR_TOTAL_CELLS,
        swatch_px: FIRST_AGENT_SWATCH_SIZE_PX,
        swatch_selected_idx: state.selected_swatch_index(),
        swatch_selection_ring_css: format!("1px solid {accent_hex}"),
        shape_count: FIRST_AGENT_FACES.len(),
        face_btn_px: FIRST_AGENT_FACE_BTN_PX,
        face_preview_px: FIRST_AGENT_FACE_PREVIEW_PX,
        shape_eye_scale: FACE_CHOICE_EYE_SCALE,
        shapes_contain_goutte: FIRST_AGENT_FACES
            .iter()
            .any(|f| f.shape.as_str() == "goutte"),
        field_name: state.name.clone(),
        field_purpose: state.purpose.clone(),
        purpose_min_height_px: FIRST_AGENT_PURPOSE_MIN_HEIGHT_PX,
        model_row_height_px: FIRST_AGENT_MODEL_ROW_HEIGHT_PX,
        submit_label: if state.generating_profile {
            tr_str(lang_id, "创建中…", "Creating…")
        } else {
            tr_str(lang_id, "创建助手", "Create agent")
        },
        submit_bg_hex: palette
            .token("--openbot-surface-light")
            .unwrap_or("#f0f0f0")
            .to_string(),
        submit_fg_hex: palette
            .token("--openbot-text-on-light")
            .unwrap_or("#141414")
            .to_string(),
        cta_min_width_px: FIRST_AGENT_CTA_MIN_WIDTH_PX,
        submit_disabled: state.validate().is_err() || state.generating_profile,
        error_message: state.error_message.clone(),
        rec_total_templates: orca_core::bot_ecosystem::templates::BUILTIN_BOT_TEMPLATES.len(),
        rec_visible_cards: FIRST_AGENT_SUGGESTIONS.len(),
        rec_card_w_px: FIRST_AGENT_REC_CARD_WIDTH_PX,
        rec_card_h_px: FIRST_AGENT_REC_CARD_HEIGHT_PX,
        rec_card_avatar_px: FIRST_AGENT_REC_CARD_AVATAR_PX,
        rec_suggestion_id: state.matched_suggestion_id.clone(),
        rec_hydrated: state.matched_suggestion_id.is_some() && !state.purpose.trim().is_empty(),
    }
}

/// Renders the 1:1 OpenBot `FirstAgentSetup` onboarding / new-agent studio (`OB-F14` / `UI-F08`).
pub fn render_first_agent_setup(
    state: &FirstAgentSetupState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let layout = compute_first_agent_setup_layout(state, palette, "en");
    let dark = palette.scheme.is_dark();
    let bg = palette.bg_canvas();
    let control = palette.bg_control();
    let hover = palette.hover();
    let border = palette.border_strong_hairline();
    let text_pri = palette.text_primary();
    let text_sec = palette.text_secondary();
    let text_muted = palette.text_muted();
    let accent_ring = palette.accent_text();
    let accent_soft = palette.accent_soft();
    let danger = palette.danger();
    let cta_bg = palette.surface_light();
    let cta_fg = palette.text_on_light();

    let selected_face = FIRST_AGENT_FACES[state.selected_face_index.min(FIRST_AGENT_FACE_COUNT - 1)];
    let mut header_avatar_props = AgentAvatarProps::new(&state.avatar_seed)
        .with_size(layout.header_avatar_px)
        .with_mood(AvatarMood::Idle)
        .with_hue_and_color(state.avatar_hue, state.avatar_color.clone());
    header_avatar_props.shape_override = Some(selected_face.shape);
    header_avatar_props.expression_override = Some(selected_face.expression);
    header_avatar_props.dark_scheme = dark;

    let mut hero_props = AgentAvatarProps::new(&state.avatar_seed)
        .with_size(layout.hero_size_px)
        .with_mood(AvatarMood::Idle)
        .with_hue_and_color(state.avatar_hue, state.avatar_color.clone());
    hero_props.shape_override = Some(selected_face.shape);
    hero_props.expression_override = Some(selected_face.expression);
    hero_props.eye_scale = layout.shape_eye_scale;
    hero_props.follow_pointer = layout.hero_pointer_follow;
    hero_props.dark_scheme = dark;

    // Exact 9x2 = 18 circular color swatches: index 0 Auto (#8b5cf6) + all 17 AVATAR_COLOR_CHOICES
    let mut swatch_cells: Vec<AnyElement> = Vec::with_capacity(layout.swatch_total);
    swatch_cells.push(
        div()
            .id("openbot-setup-color-auto")
            .w(px(28.0))
            .h(px(28.0))
            .rounded_full()
            .cursor_pointer()
            .when(layout.swatch_selected_idx == 0, |el| {
                el.border_1().border_color(accent_ring)
            })
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(layout.swatch_px))
                    .h(px(layout.swatch_px))
                    .rounded_full()
                    .bg(rgb(0x8B5CF6))
                    .border_1()
                    .border_color(palette.border_strong()),
            )
            .into_any_element(),
    );
    for (idx, choice) in AVATAR_COLOR_CHOICES.iter().enumerate() {
        let swatch_hex = match *choice {
            AvatarColorChoice::Hue { swatch, .. } | AvatarColorChoice::Preset { swatch, .. } => {
                swatch
            }
        };
        let is_selected = idx + 1 == layout.swatch_selected_idx;
        let swatch_rgba = parse_css_color_to_rgba(swatch_hex).unwrap_or_else(|| rgb(0x3B93F0));
        swatch_cells.push(
            div()
                .id(SharedString::from(format!("openbot-setup-color-{idx}")))
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .cursor_pointer()
                .when(is_selected, |el| el.border_1().border_color(accent_ring))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(layout.swatch_px))
                        .h(px(layout.swatch_px))
                        .rounded_full()
                        .bg(swatch_rgba)
                        .border_1()
                        .border_color(palette.border_strong()),
                )
                .into_any_element(),
        );
    }
    let row_1: Vec<AnyElement> = swatch_cells
        .drain(..layout.swatch_cols.min(swatch_cells.len()))
        .collect();
    let row_2: Vec<AnyElement> = swatch_cells;
    let color_grid = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.0))
        .child(div().flex().items_center().justify_center().gap(px(4.0)).children(row_1))
        .child(div().flex().items_center().justify_center().gap(px(4.0)).children(row_2));

    // Exact 12 curated faces (`33x33px`, `24x24px` preview, `FACE_CHOICE_EYE_SCALE = 1.25`)
    let mut face_row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(px(2.0));
    for (idx, face) in FIRST_AGENT_FACES.iter().enumerate() {
        let selected = idx == state.selected_face_index;
        let face_seed = if state.auto_color {
            format!("openbot-face-seed-{idx}")
        } else {
            state.avatar_seed.clone()
        };
        let mut fp = AgentAvatarProps::new(&face_seed)
            .with_size(layout.face_preview_px)
            .with_hue_and_color(state.avatar_hue, state.avatar_color.clone());
        fp.shape_override = Some(face.shape);
        fp.expression_override = Some(face.expression);
        fp.eye_scale = layout.shape_eye_scale;
        fp.transition = PlushTransition::Cut;
        fp.dark_scheme = dark;

        face_row = face_row.child(
            div()
                .id(SharedString::from(format!("openbot-setup-face-{idx}")))
                .w(px(layout.face_btn_px))
                .h(px(layout.face_btn_px))
                .rounded(px(8.0))
                .when(selected, |el| {
                    el.bg(accent_soft).border_1().border_color(accent_ring)
                })
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| if selected { s } else { s.bg(hover) })
                .child(render_agent_avatar(&fp, None, now_sec)),
        );
    }

    // 6 momentum-draggable starter suggestions (`256x112px` cards with `42px` 3D plush)
    let mut suggestions_row = div()
        .id("openbot-setup-suggestions")
        .w_full()
        .min_w(px(0.0))
        .overflow_x_scroll()
        .flex()
        .items_center()
        .gap(px(10.0));
    for (idx, suggestion) in FIRST_AGENT_SUGGESTIONS.iter().enumerate() {
        let matched = state.matched_suggestion_id.as_deref() == Some(suggestion.id);
        let sug_plush = crate::components::avatar::render_plush_bot_avatar(
            suggestion.id,
            Some(FIRST_AGENT_FACES[idx % FIRST_AGENT_FACE_COUNT].shape),
            None,
            None,
            layout.rec_card_avatar_px,
            AvatarMood::Idle,
            None,
        );
        suggestions_row = suggestions_row.child(
            div()
                .id(SharedString::from(format!("openbot-setup-sug-{}", suggestion.id)))
                .w(px(layout.rec_card_w_px))
                .h(px(layout.rec_card_h_px))
                .p(px(16.0))
                .rounded(px(12.0))
                .flex_shrink_0()
                .when(matched, |el| {
                    el.bg(accent_soft).border_1().border_color(accent_ring)
                })
                .when(!matched, |el| el.border_1().border_color(border))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(14.0))
                .hover(move |s| if matched { s } else { s.bg(hover) })
                .child(sug_plush)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_pri)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(suggestion.title),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .line_height(px(18.0))
                                .text_color(text_muted)
                                .child(suggestion.description),
                        ),
                ),
        );
    }

    let scroll_body = div()
        .id("openbot-first-agent-setup")
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .px(px(24.0))
        .py(px(20.0))
        .flex()
        .flex_col()
        .justify_between()
        .gap(px(24.0))
        .child(
            div()
                .w(px(layout.form_width_px))
                .max_w_full()
                .mx_auto()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.0))
                .child(render_agent_avatar(&hero_props, None, now_sec))
                .child(color_grid)
                .child(face_row)
                .child(
                    div()
                        .w_full()
                        .mt(px(4.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(3.0))
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_sec)
                                        .child("Name")
                                        .child(div().text_color(danger).child("*")),
                                )
                                .child(
                                    div()
                                        .h(px(38.0))
                                        .px(px(12.0))
                                        .rounded(px(8.0))
                                        .bg(control)
                                        .border_1()
                                        .border_color(border)
                                        .flex()
                                        .items_center()
                                        .text_size(px(14.0))
                                        .text_color(if state.name.is_empty() {
                                            text_muted
                                        } else {
                                            text_pri
                                        })
                                        .child(if state.name.is_empty() {
                                            "New agent".to_string()
                                        } else {
                                            state.name.clone()
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_sec)
                                        .child("What should this agent help you with?"),
                                )
                                .child(
                                    div()
                                        .min_h(px(layout.purpose_min_height_px))
                                        .p(px(10.0))
                                        .rounded(px(8.0))
                                        .bg(control)
                                        .border_1()
                                        .border_color(border)
                                        .text_size(px(13.5))
                                        .text_color(if state.purpose.is_empty() {
                                            text_muted
                                        } else {
                                            text_pri
                                        })
                                        .child(if state.purpose.is_empty() {
                                            "Plan trips, compare options, or handle daily work."
                                                .to_string()
                                        } else {
                                            state.purpose.clone()
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .h(px(layout.model_row_height_px))
                                .px(px(12.0))
                                .rounded(px(8.0))
                                .bg(control)
                                .border_1()
                                .border_color(border)
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_pri)
                                        .child("Model"),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(text_sec)
                                        .child(format!(
                                            "{} · {}",
                                            state.model,
                                            state.reasoning_effort.as_str()
                                        )),
                                ),
                        )
                        .when_some(layout.error_message.clone(), |el, err| {
                            el.child(div().text_size(px(12.0)).text_color(danger).child(err))
                        })
                        .child(
                            div()
                                .mt(px(4.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(8.0))
                                .when(layout.header_cancel_visible, |el| {
                                    el.child(
                                        div()
                                            .id("openbot-setup-cancel")
                                            .h(px(36.0))
                                            .px(px(14.0))
                                            .rounded(px(8.0))
                                            .bg(control)
                                            .border_1()
                                            .border_color(border)
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .text_size(px(13.5))
                                            .text_color(text_sec)
                                            .hover(move |s| s.bg(hover))
                                            .child("Cancel"),
                                    )
                                })
                                .child(
                                    div()
                                        .id("openbot-setup-create")
                                        .min_w(px(layout.cta_min_width_px))
                                        .h(px(36.0))
                                        .px(px(16.0))
                                        .rounded(px(8.0))
                                        .bg(cta_bg)
                                        .when(layout.submit_disabled, |el| el.opacity(0.42))
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(cta_fg)
                                        .child(layout.submit_label),
                                ),
                        ),
                ),
        )
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_muted)
                        .child("Suggestions"),
                )
                .child(suggestions_row),
        );

    let live_header = div()
        .id("openbot-first-agent-setup-header")
        .w_full()
        .h(px(layout.header_height_px))
        .flex_shrink_0()
        .px(px(14.0))
        .border_b_1()
        .border_color(palette.border_hairline())
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(render_agent_avatar(&header_avatar_props, None, now_sec))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_pri)
                        .child(layout.header_live_name),
                ),
        )
        .when(layout.header_cancel_visible, |el| {
            el.child(
                div()
                    .id("openbot-setup-header-cancel")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .text_size(px(12.5))
                    .text_color(text_muted)
                    .hover(move |s| s.bg(hover))
                    .child("Cancel"),
            )
        });

    div()
        .w_full()
        .h_full()
        .bg(bg)
        .flex()
        .flex_col()
        .child(live_header)
        .child(scroll_body)
}

/// 5 views of `AgentSettingsPanel` (`Main` + 4 drill-down `AgentSettingsPage` subpages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSettingsSubpage {
    #[default]
    Main,
    Permissions,
    Folders,
    BackupModels,
    Advanced,
}

impl AgentSettingsSubpage {
    pub const ALL: [Self; 5] = [
        Self::Main,
        Self::Permissions,
        Self::Folders,
        Self::BackupModels,
        Self::Advanced,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Permissions => "permissions",
            Self::Folders => "folders",
            Self::BackupModels => "backup-models",
            Self::Advanced => "advanced",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "Agent Settings",
            Self::Permissions => "Permissions",
            Self::Folders => "Folders",
            Self::BackupModels => "Backup Models",
            Self::Advanced => "Advanced",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentNotificationLevel {
    #[default]
    All,
    Mentions,
    Muted,
}

impl AgentNotificationLevel {
    pub const ALL: [Self; 3] = [Self::All, Self::Mentions, Self::Muted];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All messages",
            Self::Mentions => "Mentions only",
            Self::Muted => "Muted",
        }
    }
}

/// Deep links and lifecycle actions exposed in `AgentSettingsPanel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSettingsLinkKind {
    Instructions,
    Memories,
    Skills,
    Routines,
    SharedTables,
    Usage,
    Duplicate,
    ExportOpenBot,
    DeleteHistory,
    DeleteAgent,
}

/// Complete state controller for `AgentSettingsPanel` (`min_width = 360px`, `284px` avatar editor popover).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSettingsState {
    pub agent_id: String,
    pub subpage: AgentSettingsSubpage,
    pub panel_width_px: f32,
    pub avatar_editor: AgentAvatarEditorState,
    pub name: String,
    pub title: String,
    pub description: String,
    pub provider: AgentProviderId,
    pub provider_account_id: Option<String>,
    pub model: String,
    pub reasoning_effort: AgentReasoningEffort,
    pub fast_mode: bool,
    pub ultrafast: bool,
    pub chat_color: Option<String>,
    pub notification_level: AgentNotificationLevel,
    pub access: AgentAccess,
    pub computer_use: bool,
    pub local_scripts: bool,
    pub additional_folders: Vec<String>,
    pub excluded_folders: Vec<String>,
    pub backup_models_automatic: bool,
    pub backup_models: Vec<AgentModelChoice>,
    pub busy_message_mode: BusyMessageMode,
    pub browser_profile_id: Option<String>,
    pub allow_new_chat: bool,
    pub starts_new_chats: bool,
    pub quick_acknowledgement: bool,
    pub builtin_memory: bool,
    pub instructions_chars: usize,
    pub memories_count: usize,
    pub skills_count: usize,
    pub routines_count: usize,
    pub shared_tables_count: usize,
}

impl Default for AgentSettingsState {
    fn default() -> Self {
        Self::new("agent-default", "Agent")
    }
}

impl AgentSettingsState {
    pub fn new(agent_id: impl Into<String>, name: impl Into<String>) -> Self {
        let id = agent_id.into();
        Self {
            avatar_editor: AgentAvatarEditorState::new(&id, Some(id.clone()), None, None, None),
            agent_id: id,
            subpage: AgentSettingsSubpage::Main,
            panel_width_px: AGENT_SETTINGS_MIN_PX,
            name: name.into(),
            title: String::new(),
            description: String::new(),
            provider: AgentProviderId::Codex,
            provider_account_id: None,
            model: "gpt-6-luna".to_string(),
            reasoning_effort: AgentReasoningEffort::Low,
            fast_mode: false,
            ultrafast: false,
            chat_color: None,
            notification_level: AgentNotificationLevel::All,
            access: AgentAccess::Full,
            computer_use: true,
            local_scripts: false,
            additional_folders: Vec::new(),
            excluded_folders: Vec::new(),
            backup_models_automatic: true,
            backup_models: Vec::new(),
            busy_message_mode: BusyMessageMode::Steer,
            browser_profile_id: None,
            allow_new_chat: false,
            starts_new_chats: false,
            quick_acknowledgement: false,
            builtin_memory: true,
            instructions_chars: 0,
            memories_count: 0,
            skills_count: 0,
            routines_count: 0,
            shared_tables_count: 0,
        }
    }

    pub fn from_agent_summary(summary: &AgentSummary) -> Self {
        let avatar_color = summary.avatar_color.clone().flatten();
        let (backup_auto, backup_order) = match &summary.backup_models {
            Some(bm) => (bm.automatic, bm.order.clone()),
            None => (true, Vec::new()),
        };
        Self {
            avatar_editor: AgentAvatarEditorState::new(
                &summary.id,
                Some(summary.avatar_seed.clone()),
                summary.avatar_hue,
                avatar_color,
                summary.avatar_url.clone(),
            ),
            agent_id: summary.id.clone(),
            subpage: AgentSettingsSubpage::Main,
            panel_width_px: AGENT_SETTINGS_MIN_PX,
            name: summary.name.clone(),
            title: summary.title.clone(),
            description: summary.description.clone(),
            provider: summary.provider,
            provider_account_id: summary.provider_account_id.clone(),
            model: summary.model.clone(),
            reasoning_effort: summary.reasoning_effort,
            fast_mode: summary.fast_mode.unwrap_or(false),
            ultrafast: summary.ultrafast.unwrap_or(false),
            chat_color: summary.chat_color.clone(),
            notification_level: if summary.notifications {
                AgentNotificationLevel::All
            } else {
                AgentNotificationLevel::Muted
            },
            access: summary.access.unwrap_or(AgentAccess::Full),
            computer_use: summary.computer_use_enabled(),
            local_scripts: summary.allow_automation.unwrap_or(false),
            additional_folders: summary.additional_folders.clone().unwrap_or_default(),
            excluded_folders: summary.excluded_folders.clone().unwrap_or_default(),
            backup_models_automatic: backup_auto,
            backup_models: backup_order,
            busy_message_mode: summary.busy_message_mode.unwrap_or(BusyMessageMode::Steer),
            browser_profile_id: summary.browser_profile_id.clone(),
            allow_new_chat: summary.allow_new_chat.unwrap_or(false),
            starts_new_chats: summary.allow_new_chat.unwrap_or(false),
            quick_acknowledgement: summary.quick_acknowledgement.unwrap_or(false),
            builtin_memory: summary.memory_enabled(),
            instructions_chars: 0,
            memories_count: 0,
            skills_count: 0,
            routines_count: 0,
            shared_tables_count: 0,
        }
    }

    pub fn set_panel_width(&mut self, width_px: f32) {
        if width_px.is_finite() {
            self.panel_width_px = width_px.max(AGENT_SETTINGS_MIN_PX);
        }
    }

    pub fn switch_provider(&mut self, provider: AgentProviderId) {
        if self.provider != provider {
            self.provider = provider;
            self.fast_mode = false;
            self.ultrafast = false;
            let default_model = provider.descriptor().default_model;
            if !default_model.is_empty() {
                self.model = default_model.to_string();
            }
        }
    }

    pub fn set_fast_flags(
        &mut self,
        fast_mode: bool,
        ultrafast: bool,
    ) -> Result<(), ShellValidationError> {
        if ultrafast && !fast_mode {
            return Err(ShellValidationError::new(
                "ultrafast_requires_fast_mode",
                "ultrafast requires fast_mode to be enabled",
            ));
        }
        self.fast_mode = fast_mode;
        self.ultrafast = ultrafast;
        Ok(())
    }

    pub fn add_backup_model(
        &mut self,
        choice: AgentModelChoice,
    ) -> Result<(), ShellValidationError> {
        if self.backup_models.len() >= AGENT_BACKUP_MODEL_LIMIT {
            return Err(ShellValidationError::new(
                "backup_models_limit",
                format!("backup models limit is {AGENT_BACKUP_MODEL_LIMIT}"),
            ));
        }
        self.backup_models.push(choice);
        Ok(())
    }

    pub fn remove_backup_model(&mut self, index: usize) {
        if index < self.backup_models.len() {
            self.backup_models.remove(index);
        }
    }

    pub fn move_backup_model(&mut self, from: usize, to: usize) {
        if from < self.backup_models.len() && to < self.backup_models.len() && from != to {
            let item = self.backup_models.remove(from);
            self.backup_models.insert(to, item);
        }
    }

    pub fn add_additional_folder(
        &mut self,
        folder: impl Into<String>,
    ) -> Result<(), ShellValidationError> {
        if self.additional_folders.len() >= AGENT_FOLDER_LIMIT {
            return Err(ShellValidationError::new(
                "folder_limit",
                format!("additional folders limit is {AGENT_FOLDER_LIMIT}"),
            ));
        }
        let path = folder.into();
        if !path.trim().is_empty() && !self.additional_folders.contains(&path) {
            self.additional_folders.push(path);
        }
        Ok(())
    }

    pub fn add_excluded_folder(
        &mut self,
        folder: impl Into<String>,
    ) -> Result<(), ShellValidationError> {
        if self.excluded_folders.len() >= AGENT_FOLDER_LIMIT {
            return Err(ShellValidationError::new(
                "folder_limit",
                format!("excluded folders limit is {AGENT_FOLDER_LIMIT}"),
            ));
        }
        let path = folder.into();
        if !path.trim().is_empty() && !self.excluded_folders.contains(&path) {
            self.excluded_folders.push(path);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ShellValidationError> {
        if self.name.trim().is_empty() || self.name.chars().count() > INPUT_LIMITS.agent_name {
            return Err(ShellValidationError::new(
                "invalid_agent_name",
                format!("agent name must be 1..={} characters", INPUT_LIMITS.agent_name),
            ));
        }
        if self.title.chars().count() > INPUT_LIMITS.agent_title {
            return Err(ShellValidationError::new(
                "invalid_agent_title",
                format!("agent title exceeds {} characters", INPUT_LIMITS.agent_title),
            ));
        }
        if self.description.chars().count() > INPUT_LIMITS.agent_description {
            return Err(ShellValidationError::new(
                "invalid_agent_description",
                format!(
                    "agent description exceeds {} characters",
                    INPUT_LIMITS.agent_description
                ),
            ));
        }
        if self.backup_models.len() > AGENT_BACKUP_MODEL_LIMIT {
            return Err(ShellValidationError::new(
                "backup_models_limit",
                format!("backup models limit is {AGENT_BACKUP_MODEL_LIMIT}"),
            ));
        }
        if self.ultrafast && !self.fast_mode {
            return Err(ShellValidationError::new(
                "ultrafast_requires_fast_mode",
                "ultrafast requires fast_mode to be enabled",
            ));
        }
        Ok(())
    }
}

fn render_settings_nav_row(
    id: impl Into<SharedString>,
    label: &str,
    value: impl Into<String>,
    palette: &OpenBotPalette,
    danger_row: bool,
) -> Stateful<Div> {
    let hover = palette.hover();
    let text_pri = if danger_row {
        palette.danger()
    } else {
        palette.text_primary()
    };
    let text_muted = palette.text_muted();

    div()
        .id(id.into())
        .min_h(px(40.0))
        .px(px(12.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .hover(move |s| s.bg(hover))
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_pri)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(text_muted)
                        .child(value.into()),
                )
                .child(tg_icon_rgba(IconName::ChevronRight, 12.0, text_muted)),
        )
}

fn render_settings_inset_group(
    title: &str,
    palette: &OpenBotPalette,
    rows: Vec<Stateful<Div>>,
) -> Div {
    let surface = palette.bg_surface();
    let border = palette.border_hairline();
    let text_muted = palette.text_muted();
    let last_idx = rows.len().saturating_sub(1);
    let mut card = div()
        .w_full()
        .rounded(px(8.0))
        .bg(surface)
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .flex()
        .flex_col();
    for (idx, row) in rows.into_iter().enumerate() {
        card = card.child(row);
        if idx < last_idx {
            card = card.child(div().w_full().h(px(1.0)).bg(border));
        }
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .ml(px(4.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_muted)
                .child(title.to_string()),
        )
        .child(card)
}

fn render_settings_toggle_row(
    id: impl Into<SharedString>,
    label: &str,
    subtitle: &str,
    enabled: bool,
    palette: &OpenBotPalette,
) -> Stateful<Div> {
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let track_off = pal(palette, "--openbot-border-grouped", 0x333335);

    div()
        .id(id.into())
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .hover(move |s| s.bg(hover))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(13.0)).text_color(text_pri).child(label.to_string()))
                .child(div().text_size(px(11.5)).text_color(text_muted).child(subtitle.to_string())),
        )
        .child(
            div()
                .w(px(36.0))
                .h(px(20.0))
                .p(px(2.0))
                .rounded(px(999.0))
                .bg(if enabled { accent } else { track_off })
                .flex()
                .items_center()
                .when(enabled, |el| el.justify_end())
                .child(div().w(px(16.0)).h(px(16.0)).rounded_full().bg(rgb(0xFFFFFF))),
        )
}

fn render_agent_settings_main(
    state: &AgentSettingsState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let dark = palette.scheme.is_dark();
    let canvas = palette.bg_canvas();
    let surface = palette.bg_surface();
    let control = palette.bg_control();
    let border = palette.border_hairline();
    let border_strong = palette.border_strong_hairline();
    let text_pri = palette.text_primary();
    let text_sec = palette.text_secondary();
    let text_muted = palette.text_muted();
    let accent = palette.accent();

    let mut av_props = AgentAvatarProps::new(&state.avatar_editor.seed)
        .with_size(62.0)
        .with_hue_and_color(state.avatar_editor.hue, state.avatar_editor.color.clone())
        .with_url(state.avatar_editor.avatar_url.clone());
    av_props.dark_scheme = dark;

    let mut chat_colors = div().flex().items_center().gap(px(5.0)).flex_wrap();
    for preset in CHAT_COLOR_PRESETS {
        let active = state.chat_color.as_deref() == Some(preset.hex);
        let sw = parse_css_color_to_rgba(preset.hex).unwrap_or_else(|| rgb(0x4699FE));
        chat_colors = chat_colors.child(
            div()
                .id(SharedString::from(format!(
                    "openbot-as-chat-color-{}",
                    preset.id
                )))
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(sw)
                .border_1()
                .border_color(if active { accent } else { border_strong })
                .cursor_pointer(),
        );
    }

    let speed_tag = if state.ultrafast {
        "Ultrafast"
    } else if state.fast_mode {
        "Fast"
    } else {
        "Standard"
    };

    let field_box = |label: &str, value: String, min_h_px: f32| {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .ml(px(8.0))
                    .text_size(px(13.5))
                    .text_color(text_sec)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .w_full()
                    .min_h(px(min_h_px))
                    .px(px(11.0))
                    .py(px(9.0))
                    .rounded(px(8.0))
                    .bg(surface)
                    .border_1()
                    .border_color(border_strong)
                    .text_size(px(14.5))
                    .text_color(text_pri)
                    .child(value),
            )
    };

    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(
            div()
                .relative()
                .w_full()
                .pt(px(8.0))
                .pb(px(4.0))
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .id("openbot-as-avatar-trigger")
                        .relative()
                        .w(px(62.0))
                        .h(px(62.0))
                        .cursor_pointer()
                        .child(render_agent_avatar(&av_props, None, now_sec))
                        .child(
                            div()
                                .id("openbot-as-avatar-pencil")
                                .absolute()
                                .right(px(-4.0))
                                .bottom(px(-4.0))
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded_full()
                                .bg(control)
                                .border_2()
                                .border_color(canvas)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(tg_icon_rgba(IconName::Edit, 11.0, text_pri)),
                        ),
                )
                .when(state.avatar_editor.picker_open, |el| {
                    el.child(
                        deferred(
                            div()
                                .absolute()
                                .top(px(78.0))
                                .left(px(0.0))
                                .occlude()
                                .child(render_avatar_editor_popover(
                                    &state.avatar_editor,
                                    state.panel_width_px,
                                    dark,
                                    now_sec,
                                )),
                        )
                        .with_priority(60),
                    )
                }),
        )
        .child(field_box("名称 / Name", state.name.clone(), 38.0))
        .child(field_box(
            "称号 / Title",
            if state.title.is_empty() {
                "添加角色称号 (<= 120 字符)".to_string()
            } else {
                state.title.clone()
            },
            38.0,
        ))
        .child(field_box(
            "描述 / Description",
            if state.description.is_empty() {
                "暂无描述 (<= 2,000 字符)".to_string()
            } else {
                state.description.clone()
            },
            72.0,
        ))
        .child(render_settings_inset_group(
            "核心 / Brain",
            palette,
            vec![
                render_settings_nav_row(
                    "openbot-as-nav-model",
                    "模型与推理",
                    format!(
                        "{} · {} · {}",
                        state.model,
                        state.reasoning_effort.as_str(),
                        speed_tag
                    ),
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-nav-backups",
                    "备用模型",
                    format!("{}/{}", state.backup_models.len(), AGENT_BACKUP_MODEL_LIMIT),
                    palette,
                    false,
                ),
            ],
        ))
        .child(
            div()
                .w_full()
                .px(px(12.0))
                .py(px(10.0))
                .rounded(px(8.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_muted)
                        .child("气泡颜色"),
                )
                .child(chat_colors),
        )
        .child(render_settings_inset_group(
            "已知 / Knows",
            palette,
            vec![
                render_settings_nav_row(
                    "openbot-as-link-instructions",
                    "指令 (AGENTS.md)",
                    format!(
                        "{}/{}",
                        state.instructions_chars, INPUT_LIMITS.agent_instructions_file
                    ),
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-link-memories",
                    "记忆",
                    format!("已保存 {} 条", state.memories_count),
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-link-skills",
                    "技能",
                    format!("已分配 {} 个", state.skills_count),
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-link-tables",
                    "数据表",
                    format!("{} 个表", state.shared_tables_count),
                    palette,
                    false,
                ),
            ],
        ))
        .child(render_settings_inset_group(
            "行为 / Does",
            palette,
            vec![
                render_settings_nav_row(
                    "openbot-as-link-routines",
                    "自动化任务",
                    format!("已配置 {} 个", state.routines_count),
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-link-usage",
                    "用量",
                    "查看",
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-link-duplicate",
                    "复制助手",
                    "",
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-link-export",
                    "导出 .openbot",
                    "",
                    palette,
                    false,
                ),
            ],
        ))
        .child(render_settings_inset_group(
            "规则 / Rules",
            palette,
            vec![
                render_settings_nav_row(
                    "openbot-as-nav-permissions",
                    "权限",
                    match state.access {
                        AgentAccess::Full => "完全访问",
                        AgentAccess::Workspace => "工作区仅限",
                    },
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-nav-folders",
                    "文件夹",
                    format!(
                        "+{} / -{}",
                        state.additional_folders.len(),
                        state.excluded_folders.len()
                    ),
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-nav-advanced",
                    "高级",
                    match state.busy_message_mode {
                        BusyMessageMode::Steer => "Steer",
                        BusyMessageMode::Queue => "Queue",
                    },
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-nav-notifications",
                    "通知设置",
                    state.notification_level.label(),
                    palette,
                    false,
                ),
                render_settings_nav_row(
                    "openbot-as-link-clear-history",
                    "删除历史记录",
                    "",
                    palette,
                    true,
                ),
                render_settings_nav_row(
                    "openbot-as-link-delete",
                    "删除助手",
                    "",
                    palette,
                    true,
                ),
            ],
        ))
}

fn render_agent_settings_permissions(
    state: &AgentSettingsState,
    palette: &OpenBotPalette,
) -> Div {
    let surface = pal(palette, "--openbot-bg-surface", 0x212121);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal(palette, "--openbot-accent-soft", 0x163150);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let full_active = state.access == AgentAccess::Full;
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child("Filesystem & Shell Access"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("openbot-as-access-full")
                                .flex_1()
                                .h(px(32.0))
                                .rounded(px(8.0))
                                .bg(if full_active { accent_soft } else { surface })
                                .border_1()
                                .border_color(if full_active { accent } else { border })
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .text_color(if full_active { accent } else { text_muted })
                                .child("Full Access"),
                        )
                        .child(
                            div()
                                .id("openbot-as-access-workspace")
                                .flex_1()
                                .h(px(32.0))
                                .rounded(px(8.0))
                                .bg(if !full_active { accent_soft } else { surface })
                                .border_1()
                                .border_color(if !full_active { accent } else { border })
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .text_color(if !full_active { accent } else { text_muted })
                                .child("Workspace Only"),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child("Agents cannot self-widen permissions via update_profile."),
                ),
        )
        .child(
            div()
                .rounded(px(12.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .p(px(4.0))
                .flex()
                .flex_col()
                .child(render_settings_toggle_row(
                    "openbot-as-perm-cua",
                    "Computer Use",
                    "Allow controlled desktop pointer and screen actions",
                    state.computer_use,
                    palette,
                ))
                .child(render_settings_toggle_row(
                    "openbot-as-perm-scripts",
                    "Local Scripts & Automation",
                    "Allow loopback automation triggers and local script runs",
                    state.local_scripts,
                    palette,
                )),
        )
}

fn render_agent_settings_folders(state: &AgentSettingsState, palette: &OpenBotPalette) -> Div {
    let surface = pal(palette, "--openbot-bg-surface", 0x212121);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let render_group = |title: &str, items: &[String]| {
        let mut box_el = div()
            .p(px(10.0))
            .rounded(px(12.0))
            .bg(surface)
            .border_1()
            .border_color(border)
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_pri)
                    .child(format!("{title} ({}/{})", items.len(), AGENT_FOLDER_LIMIT)),
            );
        if items.is_empty() {
            box_el = box_el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted)
                    .child("None configured"),
            );
        } else {
            for path in items {
                box_el = box_el.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_pri)
                        .truncate()
                        .child(path.clone()),
                );
            }
        }
        box_el
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(render_group("Additional Folders", &state.additional_folders))
        .child(render_group("Excluded Folders", &state.excluded_folders))
}

fn render_agent_settings_backup_models(
    state: &AgentSettingsState,
    palette: &OpenBotPalette,
) -> Div {
    let surface = pal(palette, "--openbot-bg-surface", 0x212121);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let mut list = div()
        .p(px(10.0))
        .rounded(px(12.0))
        .bg(surface)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_pri)
                .child(format!(
                    "Failover Order ({}/{})",
                    state.backup_models.len(),
                    AGENT_BACKUP_MODEL_LIMIT
                )),
        );

    if state.backup_models.is_empty() {
        list = list.child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted)
                .child("No backup models added yet."),
        );
    } else {
        for (idx, choice) in state.backup_models.iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .py(px(4.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(text_pri)
                            .child(format!(
                                "{}. {} · {}",
                                idx + 1,
                                choice.provider.descriptor().display_name,
                                choice.model
                            )),
                    ),
            );
        }
    }

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .rounded(px(12.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .p(px(4.0))
                .child(render_settings_toggle_row(
                    "openbot-as-backup-auto",
                    "Automatic Failover",
                    "Automatically switch to the next backup model on rate limits",
                    state.backup_models_automatic,
                    palette,
                )),
        )
        .child(list)
}

fn render_agent_settings_advanced(state: &AgentSettingsState, palette: &OpenBotPalette) -> Div {
    let surface = pal(palette, "--openbot-bg-surface", 0x212121);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal(palette, "--openbot-accent-soft", 0x163150);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let steer_active = state.busy_message_mode == BusyMessageMode::Steer;
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child("Busy Message Mode"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("openbot-as-busy-steer")
                                .flex_1()
                                .h(px(30.0))
                                .rounded(px(8.0))
                                .bg(if steer_active { accent_soft } else { surface })
                                .border_1()
                                .border_color(if steer_active { accent } else { border })
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .text_color(if steer_active { accent } else { text_muted })
                                .child("Steer"),
                        )
                        .child(
                            div()
                                .id("openbot-as-busy-queue")
                                .flex_1()
                                .h(px(30.0))
                                .rounded(px(8.0))
                                .bg(if !steer_active { accent_soft } else { surface })
                                .border_1()
                                .border_color(if !steer_active { accent } else { border })
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .text_color(if !steer_active { accent } else { text_muted })
                                .child("Queue"),
                        ),
                ),
        )
        .child(
            div()
                .rounded(px(12.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .p(px(4.0))
                .flex()
                .flex_col()
                .child(render_settings_nav_row(
                    "openbot-as-adv-browser-profile",
                    "Browser Profile",
                    state
                        .browser_profile_id
                        .clone()
                        .unwrap_or_else(|| "Default".to_string()),
                    palette,
                    false,
                ))
                .child(render_settings_toggle_row(
                    "openbot-as-adv-new-chat",
                    "Allow New Chat Tool",
                    "Allow agent to open fresh side chats",
                    state.allow_new_chat,
                    palette,
                ))
                .child(render_settings_toggle_row(
                    "openbot-as-adv-starts-chats",
                    "Starts New Chats",
                    "Allow proactive thread creation on schedules",
                    state.starts_new_chats,
                    palette,
                ))
                .child(render_settings_toggle_row(
                    "openbot-as-adv-quick-ack",
                    "Quick Acknowledgement / Plan Card",
                    "Show an immediate acknowledgement card while planning",
                    state.quick_acknowledgement,
                    palette,
                ))
                .child(render_settings_toggle_row(
                    "openbot-as-adv-memory",
                    "Built-in Memory",
                    "Persist essential and searchable agent memories",
                    state.builtin_memory,
                    palette,
                )),
        )
}

/// Renders the 1:1 OpenBot `AgentSettingsPanel` (`min_width = 360px`, `284px` avatar editor popover,
/// and all 5 views: `Main`, `Permissions`, `Folders`, `BackupModels`, `Advanced`).
pub fn render_agent_settings_panel(
    state: &AgentSettingsState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let width = state.panel_width_px.max(AGENT_SETTINGS_MIN_PX);
    let bg = palette.bg_canvas();
    let border = palette.border_hairline();
    let hover = palette.bg_control_hover();
    let text_pri = palette.text_primary();
    let text_muted = palette.text_muted();

    let content = match state.subpage {
        AgentSettingsSubpage::Main => render_agent_settings_main(state, palette, now_sec),
        AgentSettingsSubpage::Permissions => render_agent_settings_permissions(state, palette),
        AgentSettingsSubpage::Folders => render_agent_settings_folders(state, palette),
        AgentSettingsSubpage::BackupModels => render_agent_settings_backup_models(state, palette),
        AgentSettingsSubpage::Advanced => render_agent_settings_advanced(state, palette),
    };

    div()
        .min_w(px(AGENT_SETTINGS_MIN_PX))
        .w(px(width))
        .h_full()
        .bg(bg)
        .border_l_1()
        .border_color(border)
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(39.0))
                .px(px(10.0))
                .border_b_1()
                .border_color(border)
                .flex()
                .items_center()
                .gap(px(8.0))
                .when(state.subpage != AgentSettingsSubpage::Main, |el| {
                    el.child(
                        div()
                            .id("openbot-as-back")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(move |s| s.bg(hover))
                            .child(tg_icon_rgba(IconName::BackArrow, 15.0, text_muted)),
                    )
                })
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(state.subpage.label()),
                ),
        )
        .child(
            div()
                .id("openbot-agent-settings-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .px(px(15.0))
                .pt(px(20.0))
                .pb(px(28.0))
                .child(content),
        )
}

/// Interactive action emitted by [`render_first_agent_setup_interactive`].
#[derive(Debug, Clone)]
pub enum FirstAgentSetupUiAction {
    Submit,
    Cancel,
    SelectSuggestion(String),
}

/// Live interactive GPUI wrapper over `render_first_agent_setup` wiring Submit, Cancel, and Starter Suggestions.
pub fn render_first_agent_setup_interactive<V: 'static>(
    state: &FirstAgentSetupState,
    palette: &OpenBotPalette,
    now_sec: f64,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, FirstAgentSetupUiAction, &mut Context<V>) + Clone + 'static,
) -> Div {
    let layout = compute_first_agent_setup_layout(state, palette, "en");
    let control = palette.bg_control();
    let hover = palette.hover();
    let border = palette.border_strong_hairline();
    let text_pri = palette.text_primary();
    let text_muted = palette.text_muted();
    let act_submit = on_action.clone();
    let act_cancel = on_action.clone();
    let submit_disabled = layout.submit_disabled;
    let can_cancel = layout.header_cancel_visible;

    let mut suggestions_row = div()
        .flex()
        .items_center()
        .justify_center()
        .flex_wrap()
        .gap(px(6.0))
        .px(px(16.0))
        .pb(px(8.0));
    for (idx, sug) in FIRST_AGENT_SUGGESTIONS.iter().enumerate() {
        let act_s = on_action.clone();
        let sid = sug.id.to_string();
        suggestions_row = suggestions_row.child(
            div()
                .id(("first-agent-live-sug", idx))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(999.0))
                .bg(control)
                .border_1()
                .border_color(border)
                .hover(move |s| s.bg(hover))
                .text_xs()
                .text_color(text_pri)
                .on_click(cx.listener(move |this, _, _, cx| {
                    act_s(this, FirstAgentSetupUiAction::SelectSuggestion(sid.clone()), cx);
                }))
                .child(sug.title),
        );
    }

    div()
        .flex_1()
        .min_h(px(0.0))
        .size_full()
        .flex()
        .flex_col()
        .child(render_first_agent_setup(state, palette, now_sec))
        .child(suggestions_row)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .pb(px(14.0))
                .when(can_cancel, |row| {
                    row.child(
                        div()
                            .id("first-agent-live-cancel")
                            .cursor_pointer()
                            .px(px(14.0))
                            .py(px(6.0))
                            .rounded(px(999.0))
                            .bg(control)
                            .text_xs()
                            .text_color(text_muted)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                act_cancel(this, FirstAgentSetupUiAction::Cancel, cx);
                            }))
                            .child("Cancel"),
                    )
                })
                .child(
                    div()
                        .id("first-agent-live-submit")
                        .px(px(16.0))
                        .py(px(6.0))
                        .rounded(px(999.0))
                        .bg(control)
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .when(submit_disabled, |el| el.opacity(0.45))
                        .when(!submit_disabled, |el| {
                            el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                act_submit(this, FirstAgentSetupUiAction::Submit, cx);
                            }))
                        })
                        .child(layout.submit_label),
                ),
        )
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[std::prelude::v1::test]
    fn global_search_debounce_move_selection_and_plush_nan_guards() {
        let mut gs = GlobalSearchState::new(false);
        gs.open_palette();
        gs.set_query("alpha", 0.0, 100);
        // Before 150ms debounce window elapses, begin_remote_search MUST return None
        assert_eq!(gs.begin_remote_search(100), None);
        assert_eq!(gs.begin_remote_search(249), None);
        // At >= 150ms debounce elapsed, fires sequence 1; duplicate call without query change returns None
        assert_eq!(gs.begin_remote_search(250), Some(1));
        assert_eq!(gs.begin_remote_search(500), None);

        // Whitespace-only query never fires remote search even after debounce
        gs.set_query("   ", 0.0, 600);
        assert_eq!(gs.begin_remote_search(900), None);

        // move_selection guards isize::MAX and isize::MIN without overflow panic
        gs.set_query("", 0.0, 1000);
        let mk = |id: &str, title: &str| GlobalSearchResultItem {
            id: id.to_string(),
            section: GlobalSearchSectionKind::Agents,
            title: title.to_string(),
            subtitle: "Sub".to_string(),
            badge: None,
            avatar_seed: None,
            shortcut_hint: None,
        };
        gs.results = vec![mk("a1", "Alpha"), mk("a2", "Beta"), mk("a3", "Gamma")];
        gs.move_selection(isize::MAX);
        assert!(gs.selected_index < 3);
        gs.move_selection(isize::MIN);
        assert!(gs.selected_index < 3);

        // plush_metrics guards non-finite caret_x
        gs.caret_x = f64::NAN;
        assert_eq!(gs.plush_metrics().gaze_x, 0.0);
        gs.caret_x = f64::INFINITY;
        assert_eq!(gs.plush_metrics().gaze_x, 0.0);
    }

    #[std::prelude::v1::test]
    fn first_agent_setup_over_limit_rejects_validate_and_suggestion_clears_error() {
        let mut st = FirstAgentSetupState::first_run();
        st.name = "Alpha".into();
        assert!(st.set_purpose("x".repeat(FIRST_AGENT_PURPOSE_MAX_CHARS + 1)).is_err());
        // Over-limit state MUST fail validate() and disable submit without substring heuristics
        assert!(st.validate().is_err());
        let ob = OpenBotPalette::dark();
        assert!(compute_first_agent_setup_layout(&st, &ob, "en").submit_disabled);

        // Selecting a starter suggestion clears over-limit & error_message cleanly
        assert!(st.select_suggestion("inbox-helper"));
        assert!(st.error_message.is_none());
        assert!(st.validate().is_ok());
        assert!(!compute_first_agent_setup_layout(&st, &ob, "en").submit_disabled);

        // Non-finite momentum step never poisons carousel_offset_px
        st.step_suggestion_momentum(f32::NAN);
        st.step_suggestion_momentum(f32::INFINITY);
        assert!(st.carousel_offset_px.is_finite());
        assert!(st.carousel_velocity.is_finite());
    }
}
