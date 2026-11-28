//! OpenBot `383a35d2` unified `52px` left icon `PageRail` (`AppleAppSidebar`).
//!
//! Houses Messages (`Chats`) fixed at top, user-pinnable app verticals (`社区`, `图书`, `音乐`, `游戏`,
//! `应用商店`) + `...` `RailMoreMenu` (Pin/Unpin across all 16 secondary destinations with atomic
//! JSON persistence), and a single unified bottom status/account stack (`UsageRings` canvas, Help `?`,
//! Settings `⚙` with `#ff1e5e` update dot, `24×1px` hairline divider, and sole `24×24px` Workspace
//! Account plush switcher tile).

use std::fs;
use std::path::Path;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::CreationKind;
use openbot_contracts::providers::AgentProviderId;
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{ServerPlushLogo, render_server_plush_logo};
use crate::i18n::tr_str;
use crate::theme::{
    IconName, OPENBOT_BADGE_NEW_DARK, OPENBOT_BADGE_NEW_LIGHT, OPENBOT_LAYER_TOOLTIP,
    TdesktopThemePalette, tg_icon_rgba,
};
use crate::views::content_hub::ContentHubSection;
use crate::views::openbot::page_rail::{paint_usage_rings_canvas, render_usage_hover_card};
use crate::views::openbot::shell_state::{
    PageRailHideableItem, UsageRingMetric, UsageRingSpec, write_json_atomic,
};

/// Canonical OpenBot `383a35d2` left `PageRail` width (`52px`).
pub const OPENBOT_PAGE_RAIL_WIDTH: f32 = 52.0;
/// Backward-compatible expanded width constant aliased to `OPENBOT_PAGE_RAIL_WIDTH` (`52px`).
pub const EXPANDED_WIDTH: f32 = OPENBOT_PAGE_RAIL_WIDTH;
/// Backward-compatible collapsed width constant aliased to `OPENBOT_PAGE_RAIL_WIDTH` (`52px`).
pub const COLLAPSED_WIDTH: f32 = OPENBOT_PAGE_RAIL_WIDTH;
/// Top macOS traffic-light clearance & window drag strip height (`calc(38px + 4px) = 42px`).
pub const TOP_CLEARANCE_HEIGHT: f32 = 42.0;
/// Rail button square dimension (`36x36px`).
pub const RAIL_BUTTON_SIZE_PX: f32 = 36.0;
/// Rail button corner radius (`12px`).
pub const RAIL_BUTTON_RADIUS_PX: f32 = 12.0;
/// Rail icon square dimension (`20x20px`).
pub const RAIL_ICON_SIZE_PX: f32 = 20.0;
/// Vertical gap between buttons inside each rail group (`6px`).
pub const RAIL_GROUP_GAP_PX: f32 = 6.0;
/// Bottom padding below the sole account switcher tile (`12px`).
pub const RAIL_BOTTOM_PADDING_PX: f32 = 12.0;
/// Rail Lucide-aligned icon stroke width (`1.5px`, `tokens.css:L321`).
pub const RAIL_ICON_STROKE_PX: f32 = 1.5;
/// `RailMoreMenu` popover placement (`PageRail.tsx:L538`).
pub const RAIL_MORE_MENU_PLACEMENT: &str = "right-start";
/// `RailMoreMenu` popover gutter offset (`10px`, `PageRail.tsx:L539`).
pub const RAIL_MORE_MENU_GUTTER_PX: f32 = 10.0;
/// Settings update indicator dot diameter (`8px`).
pub const RAIL_UPDATE_DOT_SIZE_PX: f32 = 8.0;
/// Bottom divider dimensions (`24x1px`).
pub const RAIL_DIVIDER_WIDTH_PX: f32 = 24.0;
pub const RAIL_DIVIDER_HEIGHT_PX: f32 = 1.0;
/// Sole workspace account plush tile dimension (`24x24px`).
pub const RAIL_ACCOUNT_TILE_PX: f32 = 24.0;
/// Minimum vertical free gap between top nav group and bottom utilities (`12px`).
pub const RAIL_MIN_FREE_GAP_PX: f32 = 12.0;
/// Tooltip pill corner radius (`8px`).
pub const RAIL_TOOLTIP_RADIUS_PX: f32 = 8.0;

/// Atomic JSON filename storing user-pinned `PageRail` sections inside `preferences_dir`.
pub const PAGE_RAIL_PINS_FILE: &str = "openbot-page-rail-pins-v1.json";

/// Legacy v1 default pinned sections before Workflows was promoted into the default left rail.
const LEGACY_V1_DEFAULT_PINNED_SECTIONS: [AppleSidebarSection; 5] = [
    AppleSidebarSection::Community,
    AppleSidebarSection::Books,
    AppleSidebarSection::Music,
    AppleSidebarSection::Games,
    AppleSidebarSection::AppStore,
];

/// Default pinned sections mounted directly under fixed `Chats` in the `52px` `PageRail`,
/// placing `Workflows` first directly beneath `Chats` to match OpenBot's `PageRail` layout.
pub const DEFAULT_PINNED_SECTIONS: [AppleSidebarSection; 6] = [
    AppleSidebarSection::Workflows,
    AppleSidebarSection::Community,
    AppleSidebarSection::Books,
    AppleSidebarSection::Music,
    AppleSidebarSection::Games,
    AppleSidebarSection::AppStore,
];

/// Complete ordered catalog of all 16 pinnable `RailMoreMenu` destinations (excludes fixed `Chats`).
pub const PINNABLE_SECTIONS: [AppleSidebarSection; 16] = [
    AppleSidebarSection::Workflows,
    AppleSidebarSection::Community,
    AppleSidebarSection::Books,
    AppleSidebarSection::Music,
    AppleSidebarSection::Games,
    AppleSidebarSection::AppStore,
    AppleSidebarSection::MyCreations,
    AppleSidebarSection::Contacts,
    AppleSidebarSection::Calls,
    AppleSidebarSection::Tasks,
    AppleSidebarSection::Schedule,
    AppleSidebarSection::Library,
    AppleSidebarSection::Usage,
    AppleSidebarSection::McpTools,
    AppleSidebarSection::RemoteComputer,
    AppleSidebarSection::Browser,
];

/// Top-level destinations surfaced in `AppleAppSidebar` (`PageRail` + `RailMoreMenu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppleSidebarSection {
    Chats,
    Community,
    Books,
    Music,
    Games,
    AppStore,
    MyCreations,
    Contacts,
    Calls,
    Workflows,
    Tasks,
    Schedule,
    Library,
    Usage,
    McpTools,
    RemoteComputer,
    Browser,
}

impl AppleSidebarSection {
    /// Converts this sidebar destination into a [`ContentHubSection`] (`None` for non-ContentHub entries).
    pub fn to_content_hub_section(self) -> Option<ContentHubSection> {
        match self {
            Self::Books => Some(ContentHubSection::Books),
            Self::Music => Some(ContentHubSection::Music),
            Self::Games => Some(ContentHubSection::Games),
            Self::AppStore => Some(ContentHubSection::AppStore),
            Self::MyCreations => Some(ContentHubSection::MyCreations),
            _ => None,
        }
    }

    /// Maps a [`ContentHubSection`] back to its corresponding [`AppleSidebarSection`].
    pub fn from_content_hub_section(section: ContentHubSection) -> Self {
        match section {
            ContentHubSection::Books => Self::Books,
            ContentHubSection::Music => Self::Music,
            ContentHubSection::Games => Self::Games,
            ContentHubSection::AppStore => Self::AppStore,
            ContentHubSection::MyCreations => Self::MyCreations,
        }
    }

    /// Returns `true` when this destination may be pinned/unpinned in `RailMoreMenu` (`Chats` is fixed).
    pub const fn is_pinnable(self) -> bool {
        !matches!(self, Self::Chats)
    }

    /// Returns the existing embedded [`IconName`] for this `PageRail` destination.
    pub const fn icon_name(self) -> IconName {
        match self {
            Self::Chats => IconName::BotRobot,
            Self::Community => IconName::CommunityGlobe,
            Self::Books => IconName::HubBooks,
            Self::Music => IconName::HubMusic,
            Self::Games => IconName::HubGames,
            Self::AppStore => IconName::HubAppStore,
            Self::MyCreations => IconName::HubCreations,
            Self::Contacts => IconName::ContactsUsers,
            Self::Calls => IconName::PhoneCall,
            Self::Workflows => IconName::WorkflowWaypoints,
            Self::Tasks => IconName::SelectCheck,
            Self::Schedule => IconName::ClockSending,
            Self::Library => IconName::FileDoc,
            Self::Usage => IconName::Poll,
            Self::McpTools => IconName::TerminalMcp,
            Self::RemoteComputer => IconName::ComputerDesktop,
            Self::Browser => IconName::ScreenShare,
        }
    }

    /// Returns the localized `zh-CN` / `en` label used in tooltips and `RailMoreMenu`.
    pub fn localized_label(self, lang: &str) -> &'static str {
        match self {
            Self::Chats => tr_str(lang, "消息", "Chats"),
            Self::Community => tr_str(lang, "社区", "Community"),
            Self::Books => tr_str(lang, "图书", "Books"),
            Self::Music => tr_str(lang, "音乐", "Music"),
            Self::Games => tr_str(lang, "游戏", "Games"),
            Self::AppStore => tr_str(lang, "应用商店", "App Store"),
            Self::MyCreations => tr_str(lang, "云端创作", "My Creations"),
            Self::Contacts => tr_str(lang, "联系人", "Contacts"),
            Self::Calls => tr_str(lang, "通话", "Calls"),
            Self::Workflows => tr_str(lang, "工作流", "Workflows"),
            Self::Tasks => tr_str(lang, "任务", "Tasks"),
            Self::Schedule => tr_str(lang, "定时任务", "Schedule"),
            Self::Library => tr_str(lang, "文件库", "Library"),
            Self::Usage => tr_str(lang, "用量", "Usage"),
            Self::McpTools => tr_str(lang, "MCP 工具", "MCP Tools"),
            Self::RemoteComputer => tr_str(lang, "远程电脑", "Remote Computer"),
            Self::Browser => tr_str(lang, "浏览器", "Browser"),
        }
    }

    /// Returns the stable GPUI element ID for this rail destination.
    pub const fn element_id(self) -> &'static str {
        match self {
            Self::Chats => "apple-sidebar-nav-chats",
            Self::Community => "apple-sidebar-nav-community",
            Self::Books => "apple-sidebar-nav-books",
            Self::Music => "apple-sidebar-nav-music",
            Self::Games => "apple-sidebar-nav-games",
            Self::AppStore => "apple-sidebar-nav-appstore",
            Self::MyCreations => "apple-sidebar-nav-my-creations",
            Self::Contacts => "apple-sidebar-nav-contacts",
            Self::Calls => "apple-sidebar-nav-calls",
            Self::Workflows => "apple-sidebar-nav-workflows",
            Self::Tasks => "apple-sidebar-nav-tasks",
            Self::Schedule => "apple-sidebar-nav-schedule",
            Self::Library => "apple-sidebar-nav-library",
            Self::Usage => "apple-sidebar-nav-usage",
            Self::McpTools => "apple-sidebar-nav-mcp-tools",
            Self::RemoteComputer => "apple-sidebar-nav-remote-computer",
            Self::Browser => "apple-sidebar-nav-browser",
        }
    }
}

/// Actions surfaced inside the bottom `?` `RailHelpMenu` popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RailHelpAction {
    Shortcuts,
    ExportData,
    About,
}

/// Persisted schema written atomically to `<preferences_dir>/openbot-page-rail-pins-v1.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRailPinsPersisted {
    pub version: u32,
    pub pinned_sections: Vec<AppleSidebarSection>,
    #[serde(default)]
    pub hidden_bottom_items: Vec<PageRailHideableItem>,
}

/// Filters non-pinnable sections (`Chats`) and deduplicates pinned entries while preserving order.
pub fn sanitize_pinned_sections(raw: Vec<AppleSidebarSection>) -> Vec<AppleSidebarSection> {
    let mut out = Vec::with_capacity(raw.len());
    for sec in raw {
        if sec.is_pinnable() && !out.contains(&sec) {
            out.push(sec);
        }
    }
    out
}

/// Deduplicates hideable bottom items while preserving order.
pub fn sanitize_hidden_bottom_items(raw: Vec<PageRailHideableItem>) -> Vec<PageRailHideableItem> {
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// Loads persisted `PageRail` pins from `<preferences_dir>/openbot-page-rail-pins-v1.json`.
///
/// Missing or syntax-corrupt JSON falls back to [`DEFAULT_PINNED_SECTIONS`]. A valid JSON object
/// with `"pinned_sections": [...]` leniently drops unknown/empty section strings while preserving
/// explicit empty `[]`.
pub fn load_page_rail_pins(preferences_dir: &Path) -> Vec<AppleSidebarSection> {
    let path = preferences_dir.join(PAGE_RAIL_PINS_FILE);
    let Ok(bytes) = fs::read(&path) else {
        return DEFAULT_PINNED_SECTIONS.to_vec();
    };
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return DEFAULT_PINNED_SECTIONS.to_vec();
    };
    let Some(arr) = val.get("pinned_sections").and_then(|v| v.as_array()) else {
        return DEFAULT_PINNED_SECTIONS.to_vec();
    };
    let version = val.get("version").and_then(|v| v.as_u64()).unwrap_or(1);
    let parsed: Vec<AppleSidebarSection> = arr
        .iter()
        .filter_map(|elem| serde_json::from_value::<AppleSidebarSection>(elem.clone()).ok())
        .collect();
    let mut pins = sanitize_pinned_sections(parsed);
    if version < 2 && pins.as_slice() == LEGACY_V1_DEFAULT_PINNED_SECTIONS.as_slice() {
        pins.insert(0, AppleSidebarSection::Workflows);
    }
    pins
}

/// Loads persisted hidden optional bottom rail items from `<preferences_dir>/openbot-page-rail-pins-v1.json`.
pub fn load_page_rail_hidden_items(preferences_dir: &Path) -> Vec<PageRailHideableItem> {
    let path = preferences_dir.join(PAGE_RAIL_PINS_FILE);
    let Ok(bytes) = fs::read(&path) else {
        return Vec::new();
    };
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(arr) = val.get("hidden_bottom_items").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let parsed: Vec<PageRailHideableItem> = arr
        .iter()
        .filter_map(|elem| {
            elem.as_str()
                .and_then(PageRailHideableItem::parse)
                .or_else(|| serde_json::from_value::<PageRailHideableItem>(elem.clone()).ok())
        })
        .collect();
    sanitize_hidden_bottom_items(parsed)
}

/// Atomically persists `PageRail` pins and hidden optional bottom items.
pub fn save_page_rail_prefs(
    preferences_dir: &Path,
    pins: &[AppleSidebarSection],
    hidden: &[PageRailHideableItem],
) -> std::io::Result<()> {
    let path = preferences_dir.join(PAGE_RAIL_PINS_FILE);
    let payload = PageRailPinsPersisted {
        version: 2,
        pinned_sections: sanitize_pinned_sections(pins.to_vec()),
        hidden_bottom_items: sanitize_hidden_bottom_items(hidden.to_vec()),
    };
    write_json_atomic(&path, &payload)
}

/// Atomically persists `PageRail` pins to `<preferences_dir>/openbot-page-rail-pins-v1.json`,
/// preserving any previously saved hidden bottom items.
pub fn save_page_rail_pins(
    preferences_dir: &Path,
    pins: &[AppleSidebarSection],
) -> std::io::Result<()> {
    let existing_hidden = load_page_rail_hidden_items(preferences_dir);
    save_page_rail_prefs(preferences_dir, pins, &existing_hidden)
}

/// Single-source-of-truth tooltip pill layout consumed by `render_tooltip_pill`.
#[derive(Debug, Clone, PartialEq)]
pub struct TooltipLayoutSnapshot {
    pub visible: bool,
    pub label: String,
    pub radius_px: f32,
    pub z_index: usize,
}

pub fn compute_tooltip_layout(visible: bool, label: &str) -> TooltipLayoutSnapshot {
    TooltipLayoutSnapshot {
        visible,
        label: label.to_string(),
        radius_px: RAIL_TOOLTIP_RADIUS_PX,
        z_index: OPENBOT_LAYER_TOOLTIP,
    }
}

/// Single-source-of-truth bottom utility stack geometry for `PageRail`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRailBottomLayout {
    pub usage_ring_slots: usize,
    pub collapsed_more_rings_count: usize,
    pub min_free_px: f32,
    pub help_visible: bool,
    pub settings_visible: bool,
    pub update_dot_px: f32,
    pub update_dot_hex: &'static str,
    pub divider_w_px: f32,
    pub divider_h_px: f32,
    pub account_tile_px: f32,
    pub account_tile_is_plush: bool,
}

/// Single-source-of-truth `52px` left `PageRail` geometry snapshot consumed directly by `AppleAppSidebar::render`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRailLayoutSnapshot {
    pub visible: bool,
    pub width_px: f32,
    pub padding_top_px: f32,
    pub padding_bottom_px: f32,
    pub gap_px: f32,
    pub button_size_px: f32,
    pub button_radius_px: f32,
    pub icon_size_px: f32,
    pub icon_stroke_px: f32,
    pub more_menu_open: bool,
    pub more_menu_placement: &'static str,
    pub more_menu_gutter_px: f32,
    pub more_menu_left_px: f32,
    pub bottom_items: PageRailBottomLayout,
}

/// Events emitted by [`AppleAppSidebar`] to [`crate::views::AppShell`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppleSidebarEvent {
    SelectSection(AppleSidebarSection),
    ToggleCreationsDrawer(bool),
    SelectCreationKindFilter(Option<CreationKind>),
    ToggleCollapsed(bool),
    OpenCreatorBot(Option<CreationKind>),
    OpenAccountMenu,
    OpenSettings,
    TogglePin(AppleSidebarSection),
    OpenHelp,
    OpenHelpAction(RailHelpAction),
    PreloadWorkflows,
}

pub(crate) fn default_usage_ring_metric() -> UsageRingMetric {
    UsageRingMetric {
        provider: AgentProviderId::Codex,
        label: "AI Usage".to_string(),
        primary_ratio: Some(0.0),
        secondary_ratio: None,
        unavailable: false,
        reset_label: None,
        detail_summary: Some("No AI usage recorded yet".to_string()),
    }
}

/// Unified `52px` OpenBot `PageRail` navigation entity.
pub struct AppleAppSidebar {
    pub active_section: AppleSidebarSection,
    pub highlight_active_section: bool,
    pub settings_active: bool,
    pub is_collapsed: bool,
    pub creations_drawer_open: bool,
    pub creations_filters_expanded: bool,
    pub active_creation_kind_filter: Option<CreationKind>,
    pub unread_chats_count: u32,
    pub community_unread_count: u32,
    pub creations_count: usize,
    pub pending_review_count: usize,
    pub account_name: String,
    pub account_initials: String,
    pub is_connected: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub highlighted_destination: Option<AppleSidebarSection>,
    pub pinned_sections: Vec<AppleSidebarSection>,
    pub more_menu_open: bool,
    pub help_menu_open: bool,
    pub has_update_notice: bool,
    pub usage_ring_metric: UsageRingMetric,
    pub usage_ring_metrics: Vec<UsageRingMetric>,
    pub hidden_bottom_items: Vec<PageRailHideableItem>,
    pub undo_toast_item: Option<PageRailHideableItem>,
    pub narrow_hidden: bool,
    pub hovered_tooltip: Option<SharedString>,
    pub hovered_more_row: Option<AppleSidebarSection>,
    pub usage_hover_open: bool,
}

impl EventEmitter<AppleSidebarEvent> for AppleAppSidebar {}

impl AppleAppSidebar {
    pub fn new(palette: TdesktopThemePalette, language_id: impl Into<String>) -> Self {
        Self::new_with_pins(palette, language_id, DEFAULT_PINNED_SECTIONS.to_vec())
    }

    pub fn new_with_pins(
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        pins: Vec<AppleSidebarSection>,
    ) -> Self {
        let default_ring = default_usage_ring_metric();
        Self {
            active_section: AppleSidebarSection::Chats,
            highlight_active_section: true,
            settings_active: false,
            is_collapsed: false,
            creations_drawer_open: false,
            creations_filters_expanded: false,
            active_creation_kind_filter: None,
            unread_chats_count: 0,
            community_unread_count: 0,
            creations_count: 0,
            pending_review_count: 0,
            account_name: String::new(),
            account_initials: String::new(),
            is_connected: false,
            palette,
            language_id: language_id.into(),
            highlighted_destination: Some(AppleSidebarSection::Chats),
            pinned_sections: sanitize_pinned_sections(pins),
            more_menu_open: false,
            help_menu_open: false,
            has_update_notice: false,
            usage_ring_metric: default_ring.clone(),
            usage_ring_metrics: vec![default_ring],
            hidden_bottom_items: Vec::new(),
            undo_toast_item: None,
            narrow_hidden: false,
            hovered_tooltip: None,
            hovered_more_row: None,
            usage_hover_open: false,
        }
    }

    pub fn effective_width(&self) -> f32 {
        if self.narrow_hidden {
            0.0
        } else {
            OPENBOT_PAGE_RAIL_WIDTH
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang_id: impl Into<String>) {
        self.language_id = lang_id.into();
    }

    pub fn set_active_section(&mut self, section: AppleSidebarSection) {
        self.active_section = section;
        self.highlight_active_section = true;
        self.highlighted_destination = Some(section);
    }

    pub fn set_highlight_active_section(&mut self, highlight: bool) {
        self.highlight_active_section = highlight;
    }

    pub fn set_highlighted_destination(&mut self, dest: Option<AppleSidebarSection>) {
        self.highlighted_destination = dest;
    }

    pub fn set_settings_active(&mut self, active: bool) {
        self.settings_active = active;
    }

    pub fn set_update_notice(&mut self, available: bool) {
        self.has_update_notice = available;
    }

    pub fn set_narrow_hidden(&mut self, hidden: bool) {
        self.narrow_hidden = hidden;
        if hidden {
            self.close_popovers();
        }
    }

    pub fn set_usage_ring_metric(&mut self, metric: UsageRingMetric) {
        self.usage_ring_metric = metric.clone();
        if self.usage_ring_metrics.is_empty() {
            self.usage_ring_metrics.push(metric);
        } else {
            self.usage_ring_metrics[0] = metric;
        }
    }

    pub fn set_usage_ring_metrics(&mut self, metrics: Vec<UsageRingMetric>) {
        if let Some(first) = metrics.first() {
            self.usage_ring_metric = first.clone();
        }
        self.usage_ring_metrics = metrics;
    }

    pub fn toggle_more_menu(&mut self) {
        self.more_menu_open = !self.more_menu_open;
        if self.more_menu_open {
            self.help_menu_open = false;
            self.usage_hover_open = false;
            self.hovered_tooltip = None;
        }
    }

    pub fn is_bottom_item_hidden(&self, item: PageRailHideableItem) -> bool {
        self.hidden_bottom_items.contains(&item)
    }

    /// Hides an optional bottom rail item (`credits`, `marketplace`, `help`), refusing mandatory
    /// items (`settings`, `account`). Sets `undo_toast_item` for Undo toast display.
    pub fn hide_bottom_item_by_id(&mut self, id: &str) -> bool {
        let Some(item) = PageRailHideableItem::parse(id) else {
            return false;
        };
        if !self.hidden_bottom_items.contains(&item) {
            self.hidden_bottom_items.push(item);
        }
        self.undo_toast_item = Some(item);
        true
    }

    /// Reverses the last hidden optional bottom item and clears the Undo toast.
    pub fn undo_hide_bottom_item(&mut self) -> Option<PageRailHideableItem> {
        let item = self.undo_toast_item.take()?;
        self.hidden_bottom_items.retain(|existing| *existing != item);
        Some(item)
    }

    pub fn is_pinned(&self, section: AppleSidebarSection) -> bool {
        self.pinned_sections.contains(&section)
    }

    /// Pure pin toggle used by both live GPUI clicks (`toggle_pin`) and headless state transitions.
    pub fn toggle_pin_local(&mut self, section: AppleSidebarSection) -> bool {
        if !section.is_pinnable() {
            return false;
        }
        if let Some(pos) = self.pinned_sections.iter().position(|&s| s == section) {
            self.pinned_sections.remove(pos);
        } else {
            self.pinned_sections.push(section);
        }
        true
    }

    /// Toggles pin membership for `section` (`Chats` is fixed and ignored), emitting `TogglePin`.
    pub fn toggle_pin(&mut self, section: AppleSidebarSection, cx: &mut Context<Self>) -> bool {
        if !self.toggle_pin_local(section) {
            return false;
        }
        cx.emit(AppleSidebarEvent::TogglePin(section));
        cx.notify();
        true
    }

    /// Single-source-of-truth geometry calculation consumed directly by `AppleAppSidebar::render`.
    pub fn compute_layout(&self, viewport_h: f32) -> PageRailLayoutSnapshot {
        let visible = !self.narrow_hidden;
        let width_px = self.effective_width();
        let help_visible = !self.is_bottom_item_hidden(PageRailHideableItem::Help);
        let settings_visible = true;
        let total_rings = self.usage_ring_metrics.len().max(1);
        let top_btn_count = 1 + self.pinned_sections.len() + 1;
        let top_h = TOP_CLEARANCE_HEIGHT
            + (top_btn_count as f32) * RAIL_BUTTON_SIZE_PX
            + (top_btn_count.saturating_sub(1) as f32) * RAIL_GROUP_GAP_PX;
        let fixed_bottom_btns = usize::from(help_visible) + usize::from(settings_visible) + 1;
        let divider_total_h = RAIL_DIVIDER_HEIGHT_PX + 4.0 + RAIL_GROUP_GAP_PX;
        let fixed_bottom_h = RAIL_BOTTOM_PADDING_PX
            + (fixed_bottom_btns as f32) * RAIL_BUTTON_SIZE_PX
            + (fixed_bottom_btns.saturating_sub(1) as f32) * RAIL_GROUP_GAP_PX
            + divider_total_h;
        let slot_h = RAIL_BUTTON_SIZE_PX + RAIL_GROUP_GAP_PX;
        let safe_vp_h = if viewport_h.is_finite() {
            viewport_h.max(0.0)
        } else {
            0.0
        };
        let avail_for_rings = (safe_vp_h - top_h - fixed_bottom_h - RAIL_MIN_FREE_GAP_PX).max(0.0);
        let max_ring_slots = ((avail_for_rings / slot_h).floor() as usize).max(1);
        let (usage_ring_slots, collapsed_more_rings_count) = if total_rings <= max_ring_slots {
            (total_rings, 0)
        } else {
            let shown = max_ring_slots.saturating_sub(1).max(1);
            (shown, total_rings.saturating_sub(shown))
        };
        let rings_h = (usage_ring_slots as f32) * slot_h
            + if collapsed_more_rings_count > 0 {
                slot_h
            } else {
                0.0
            };
        let free_px = (safe_vp_h - top_h - fixed_bottom_h - rings_h).max(RAIL_MIN_FREE_GAP_PX);
        let update_dot_hex = if self.palette.mode.is_dark() {
            OPENBOT_BADGE_NEW_DARK
        } else {
            OPENBOT_BADGE_NEW_LIGHT
        };

        PageRailLayoutSnapshot {
            visible,
            width_px,
            padding_top_px: TOP_CLEARANCE_HEIGHT,
            padding_bottom_px: RAIL_BOTTOM_PADDING_PX,
            gap_px: RAIL_GROUP_GAP_PX,
            button_size_px: RAIL_BUTTON_SIZE_PX,
            button_radius_px: RAIL_BUTTON_RADIUS_PX,
            icon_size_px: RAIL_ICON_SIZE_PX,
            icon_stroke_px: RAIL_ICON_STROKE_PX,
            more_menu_open: self.more_menu_open,
            more_menu_placement: RAIL_MORE_MENU_PLACEMENT,
            more_menu_gutter_px: RAIL_MORE_MENU_GUTTER_PX,
            more_menu_left_px: OPENBOT_PAGE_RAIL_WIDTH + RAIL_MORE_MENU_GUTTER_PX,
            bottom_items: PageRailBottomLayout {
                usage_ring_slots,
                collapsed_more_rings_count,
                min_free_px: free_px,
                help_visible,
                settings_visible,
                update_dot_px: RAIL_UPDATE_DOT_SIZE_PX,
                update_dot_hex,
                divider_w_px: RAIL_DIVIDER_WIDTH_PX,
                divider_h_px: RAIL_DIVIDER_HEIGHT_PX,
                account_tile_px: RAIL_ACCOUNT_TILE_PX,
                account_tile_is_plush: true,
            },
        }
    }

    /// Returns the currently active `PageRail` destination for visual highlight.
    pub fn active_rail_destination(&self) -> Option<AppleSidebarSection> {
        if let Some(dest) = self.highlighted_destination {
            Some(dest)
        } else if self.highlight_active_section {
            Some(self.active_section)
        } else {
            None
        }
    }

    /// Returns `true` when the active destination lives inside `...` `RailMoreMenu` (is unpinned).
    pub fn current_in_more_menu(&self) -> bool {
        match self.active_rail_destination() {
            Some(sec) if sec.is_pinnable() => !self.is_pinned(sec),
            _ => false,
        }
    }

    /// Closes transient rail popovers (`RailMoreMenu`, `RailHelpMenu`, hover `UsageCard`).
    pub fn close_popovers(&mut self) -> bool {
        let was_open = self.more_menu_open
            || self.help_menu_open
            || self.usage_hover_open
            || self.hovered_more_row.is_some();
        self.more_menu_open = false;
        self.help_menu_open = false;
        self.usage_hover_open = false;
        self.hovered_more_row = None;
        was_open
    }

    pub fn set_account_info(
        &mut self,
        name: impl Into<String>,
        initials: impl Into<String>,
        is_connected: bool,
    ) {
        self.account_name = name.into();
        self.account_initials = initials.into();
        self.is_connected = is_connected;
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.is_collapsed = collapsed;
    }

    pub fn set_creations_drawer_open(&mut self, open: bool) {
        self.creations_drawer_open = open;
    }

    pub fn set_creation_kind_filter(&mut self, filter: Option<CreationKind>) {
        self.active_creation_kind_filter = filter;
    }

    pub fn set_badges(
        &mut self,
        unread_chats_count: u32,
        creations_count: usize,
        pending_review_count: usize,
    ) {
        self.unread_chats_count = unread_chats_count;
        self.creations_count = creations_count;
        self.pending_review_count = pending_review_count;
    }

    pub fn set_community_unread(&mut self, count: u32) {
        self.community_unread_count = count;
    }

    pub fn reset_for_logout_or_account_switch(&mut self) {
        self.active_section = AppleSidebarSection::Chats;
        self.highlight_active_section = true;
        self.highlighted_destination = Some(AppleSidebarSection::Chats);
        self.settings_active = false;
        self.creations_drawer_open = false;
        self.active_creation_kind_filter = None;
        self.unread_chats_count = 0;
        self.community_unread_count = 0;
        self.creations_count = 0;
        self.pending_review_count = 0;
        self.account_name.clear();
        self.account_initials.clear();
        self.is_connected = false;
        self.more_menu_open = false;
        self.help_menu_open = false;
        self.usage_hover_open = false;
        self.hovered_tooltip = None;
        self.hovered_more_row = None;
    }

    pub fn select_section(&mut self, section: AppleSidebarSection, cx: &mut Context<Self>) {
        self.active_section = section;
        self.highlight_active_section = true;
        self.highlighted_destination = Some(section);
        self.more_menu_open = false;
        self.help_menu_open = false;
        self.hovered_more_row = None;
        cx.emit(AppleSidebarEvent::SelectSection(section));
        cx.notify();
    }

    pub fn toggle_collapsed(&mut self, cx: &mut Context<Self>) {
        self.is_collapsed = !self.is_collapsed;
        cx.emit(AppleSidebarEvent::ToggleCollapsed(self.is_collapsed));
        cx.notify();
    }

    pub fn toggle_creations_drawer(&mut self, cx: &mut Context<Self>) {
        self.creations_drawer_open = !self.creations_drawer_open;
        cx.emit(AppleSidebarEvent::ToggleCreationsDrawer(
            self.creations_drawer_open,
        ));
        cx.notify();
    }
}

impl AppleAppSidebar {
    fn render_tooltip_pill(&self, label: SharedString) -> AnyElement {
        let ob = self.palette.openbot_palette();
        let tip = compute_tooltip_layout(true, label.as_ref());
        deferred(
            div()
                .absolute()
                .left(px(RAIL_BUTTON_SIZE_PX + 10.0))
                .top(px(5.0))
                .px(px(9.0))
                .py(px(4.0))
                .rounded(px(tip.radius_px))
                .bg(ob.bg_surface())
                .border_1()
                .border_color(ob.border_strong_hairline())
                .whitespace_nowrap()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ob.text_primary())
                .child(tip.label),
        )
        .with_priority(tip.z_index)
        .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_section_button(
        &self,
        section: AppleSidebarSection,
        is_active: bool,
        allow_right_click_unpin: bool,
        badge_count: u32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let ob = self.palette.openbot_palette();
        let hover_bg = ob.bg_control_hover();
        let active_bg = ob.bg_control_active();
        let icon_color = if is_active {
            ob.text_primary()
        } else {
            ob.text_muted()
        };
        let frame_bg = ob.bg_frame();
        let (plum_top, _) = ob.felt_plum();
        let label = SharedString::from(section.localized_label(&self.language_id));
        let hover_label = label.clone();
        let show_tooltip = !self.more_menu_open
            && !self.help_menu_open
            && self.hovered_tooltip.as_ref() == Some(&label);

        div()
            .id(section.element_id())
            .relative()
            .cursor_pointer()
            .w(px(RAIL_BUTTON_SIZE_PX))
            .h(px(RAIL_BUTTON_SIZE_PX))
            .rounded(px(RAIL_BUTTON_RADIUS_PX))
            .flex()
            .items_center()
            .justify_center()
            .when(is_active, |el| el.bg(active_bg))
            .when(!is_active, |el| el.hover(move |s| s.bg(hover_bg)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    if section == AppleSidebarSection::Workflows {
                        cx.emit(AppleSidebarEvent::PreloadWorkflows);
                    }
                    if this.hovered_tooltip.as_ref() != Some(&hover_label) {
                        this.hovered_tooltip = Some(hover_label.clone());
                        cx.notify();
                    }
                } else if this.hovered_tooltip.as_ref() == Some(&hover_label) {
                    this.hovered_tooltip = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select_section(section, cx);
                }),
            )
            .when(allow_right_click_unpin, |el| {
                el.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_pin(section, cx);
                    }),
                )
            })
            .child(tg_icon_rgba(
                section.icon_name(),
                RAIL_ICON_SIZE_PX,
                icon_color,
            ))
            .when(badge_count > 0, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(5.0))
                        .right(px(5.0))
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(plum_top)
                        .border_1()
                        .border_color(frame_bg),
                )
            })
            .when(show_tooltip, |el| el.child(self.render_tooltip_pill(label)))
    }
}

impl Render for AppleAppSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = self.compute_layout(f32::from(window.viewport_size().height));
        if !layout.visible {
            return div()
                .id("apple-app-sidebar")
                .size_0()
                .overflow_hidden();
        }
        let ob = self.palette.openbot_palette();
        let lang = self.language_id.clone();
        let frame_bg = ob.bg_frame();
        let hover_bg = ob.bg_control_hover();
        let active_bg = ob.bg_control_active();
        let icon_muted = ob.text_muted();
        let icon_primary = ob.text_primary();
        let divider_col = ob.border_hairline();
        let badge_new_col = ob.badge_new();
        let active_dest = self.active_rail_destination();
        let more_active = self.more_menu_open || self.current_in_more_menu();
        let help_active = self.help_menu_open;
        let settings_active = self.settings_active;

        let more_label = SharedString::from(tr_str(&lang, "更多", "More"));
        let more_hover = more_label.clone();
        let show_more_tip = !self.more_menu_open
            && !self.help_menu_open
            && self.hovered_tooltip.as_ref() == Some(&more_label);

        let help_label = SharedString::from(tr_str(&lang, "帮助", "Help"));
        let help_hover = help_label.clone();
        let show_help_tip = !self.more_menu_open
            && !self.help_menu_open
            && self.hovered_tooltip.as_ref() == Some(&help_label);

        let settings_label = SharedString::from(tr_str(&lang, "设置", "Settings"));
        let settings_hover = settings_label.clone();
        let show_settings_tip = !self.more_menu_open
            && !self.help_menu_open
            && self.hovered_tooltip.as_ref() == Some(&settings_label);

        let display_name = if self.account_name.trim().is_empty() {
            tr_str(&lang, "我的账号", "Account").to_string()
        } else {
            self.account_name.clone()
        };
        let account_label = SharedString::from(display_name.clone());
        let account_hover = account_label.clone();
        let show_account_tip = !self.more_menu_open
            && !self.help_menu_open
            && self.hovered_tooltip.as_ref() == Some(&account_label);
        let account_seed = if !self.account_name.trim().is_empty() {
            self.account_name.trim().to_string()
        } else if !self.account_initials.trim().is_empty() {
            self.account_initials.trim().to_string()
        } else {
            "fabushi-workspace".to_string()
        };
        let account_logo =
            ServerPlushLogo::new(account_seed).with_size(layout.bottom_items.account_tile_px);

        let mut live_usage = self.usage_ring_metric.clone();
        if !self.is_connected {
            live_usage.unavailable = true;
        }
        let ring_metric_paint = live_usage.clone();
        let ring_spec = UsageRingSpec::DEFAULT;
        let ring_track = ob.border_strong_hairline();
        let ring_primary = if self.is_connected {
            ob.success()
        } else {
            ob.text_dim()
        };
        let ring_secondary = Rgba {
            a: 0.72,
            ..ob.accent_text()
        };
        let usage_active = active_dest == Some(AppleSidebarSection::Usage);
        let show_usage_card =
            self.usage_hover_open && !self.more_menu_open && !self.help_menu_open;

        let mut top_group = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(RAIL_GROUP_GAP_PX))
            .child(self.render_section_button(
                AppleSidebarSection::Chats,
                active_dest == Some(AppleSidebarSection::Chats),
                false,
                self.unread_chats_count,
                cx,
            ));

        for &sec in &self.pinned_sections {
            let badge = if sec == AppleSidebarSection::Community {
                self.community_unread_count
            } else {
                0
            };
            top_group = top_group.child(self.render_section_button(
                sec,
                active_dest == Some(sec),
                true,
                badge,
                cx,
            ));
        }

        top_group = top_group.child(
            div()
                .id("apple-sidebar-more-btn")
                .relative()
                .cursor_pointer()
                .w(px(RAIL_BUTTON_SIZE_PX))
                .h(px(RAIL_BUTTON_SIZE_PX))
                .rounded(px(RAIL_BUTTON_RADIUS_PX))
                .flex()
                .items_center()
                .justify_center()
                .when(more_active, |el| el.bg(active_bg))
                .when(!more_active, |el| el.hover(move |s| s.bg(hover_bg)))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        if this.hovered_tooltip.as_ref() != Some(&more_hover) {
                            this.hovered_tooltip = Some(more_hover.clone());
                            cx.notify();
                        }
                    } else if this.hovered_tooltip.as_ref() == Some(&more_hover) {
                        this.hovered_tooltip = None;
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_more_menu();
                        cx.notify();
                    }),
                )
                .child(tg_icon_rgba(
                    IconName::MoreVertical,
                    RAIL_ICON_SIZE_PX,
                    if more_active {
                        icon_primary
                    } else {
                        icon_muted
                    },
                ))
                .when(show_more_tip, |el| {
                    el.child(self.render_tooltip_pill(more_label))
                }),
        );

        let bottom_group = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(RAIL_GROUP_GAP_PX))
            .child(
                div()
                    .id("apple-sidebar-usage-ring")
                    .relative()
                    .cursor_pointer()
                    .w(px(RAIL_BUTTON_SIZE_PX))
                    .h(px(RAIL_BUTTON_SIZE_PX))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(usage_active, |el| el.bg(active_bg))
                    .when(!usage_active, |el| el.hover(move |s| s.bg(hover_bg)))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if this.usage_hover_open != *hovered {
                            this.usage_hover_open = *hovered;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.usage_hover_open = false;
                            this.select_section(AppleSidebarSection::Usage, cx);
                        }),
                    )
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |bounds, _, window, _cx| {
                                paint_usage_rings_canvas(
                                    bounds,
                                    &ring_metric_paint,
                                    ring_spec,
                                    ring_track,
                                    ring_primary,
                                    ring_secondary,
                                    window,
                                );
                            },
                        )
                        .w(px(ring_spec.size_px))
                        .h(px(ring_spec.size_px)),
                    )
                    .when(show_usage_card, |el| {
                        el.child(
                            deferred(
                                div()
                                    .absolute()
                                    .left(px(RAIL_BUTTON_SIZE_PX + 10.0))
                                    .bottom(px(0.0))
                                    .occlude()
                                    .child(render_usage_hover_card(&live_usage, &ob)),
                            )
                            .with_priority(55),
                        )
                    }),
            )
            .when(layout.bottom_items.help_visible, |col| {
                col.child(
                    div()
                        .id("apple-sidebar-help-btn")
                        .relative()
                        .cursor_pointer()
                        .w(px(layout.button_size_px))
                        .h(px(layout.button_size_px))
                        .rounded(px(layout.button_radius_px))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(help_active, |el| el.bg(active_bg))
                        .when(!help_active, |el| el.hover(move |s| s.bg(hover_bg)))
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if help_active {
                            icon_primary
                        } else {
                            icon_muted
                        })
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                if this.hovered_tooltip.as_ref() != Some(&help_hover) {
                                    this.hovered_tooltip = Some(help_hover.clone());
                                    cx.notify();
                                }
                            } else if this.hovered_tooltip.as_ref() == Some(&help_hover) {
                                this.hovered_tooltip = None;
                                cx.notify();
                            }
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.help_menu_open = !this.help_menu_open;
                                if this.help_menu_open {
                                    this.more_menu_open = false;
                                    this.usage_hover_open = false;
                                    this.hovered_tooltip = None;
                                }
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                if this.hide_bottom_item_by_id("help") {
                                    cx.notify();
                                }
                            }),
                        )
                        .child("?")
                        .when(show_help_tip, |el| {
                            el.child(self.render_tooltip_pill(help_label))
                        }),
                )
            })
            .when(self.undo_toast_item.is_some(), |col| {
                col.child(
                    div()
                        .id("apple-sidebar-undo-hide-btn")
                        .cursor_pointer()
                        .w(px(layout.button_size_px))
                        .h(px(22.0))
                        .rounded(px(6.0))
                        .bg(ob.bg_control_active())
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_primary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if this.undo_hide_bottom_item().is_some() {
                                    cx.notify();
                                }
                            }),
                        )
                        .child("↺"),
                )
            })
            .child(
                div()
                    .id("apple-sidebar-settings-btn")
                    .relative()
                    .cursor_pointer()
                    .w(px(layout.button_size_px))
                    .h(px(layout.button_size_px))
                    .rounded(px(layout.button_radius_px))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(settings_active, |el| el.bg(active_bg))
                    .when(!settings_active, |el| el.hover(move |s| s.bg(hover_bg)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            if this.hovered_tooltip.as_ref() != Some(&settings_hover) {
                                this.hovered_tooltip = Some(settings_hover.clone());
                                cx.notify();
                            }
                        } else if this.hovered_tooltip.as_ref() == Some(&settings_hover) {
                            this.hovered_tooltip = None;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_popovers();
                            cx.emit(AppleSidebarEvent::OpenSettings);
                            cx.notify();
                        }),
                    )
                    .child(tg_icon_rgba(
                        IconName::SettingsGear,
                        layout.icon_size_px,
                        if settings_active {
                            icon_primary
                        } else {
                            icon_muted
                        },
                    ))
                    .when(self.has_update_notice, |el| {
                        el.child(
                            div()
                                .id("apple-sidebar-settings-update-dot")
                                .absolute()
                                .top(px(6.0))
                                .right(px(6.0))
                                .w(px(layout.bottom_items.update_dot_px))
                                .h(px(layout.bottom_items.update_dot_px))
                                .rounded_full()
                                .bg(badge_new_col)
                                .border_1()
                                .border_color(frame_bg),
                        )
                    })
                    .when(show_settings_tip, |el| {
                        el.child(self.render_tooltip_pill(settings_label))
                    }),
            )
            .child(
                div()
                    .w(px(layout.bottom_items.divider_w_px))
                    .h(px(layout.bottom_items.divider_h_px))
                    .my(px(2.0))
                    .bg(divider_col),
            )
            .child(
                div()
                    .id("apple-sidebar-account-btn")
                    .relative()
                    .cursor_pointer()
                    .w(px(layout.button_size_px))
                    .h(px(layout.button_size_px))
                    .rounded(px(layout.button_radius_px))
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            if this.hovered_tooltip.as_ref() != Some(&account_hover) {
                                this.hovered_tooltip = Some(account_hover.clone());
                                cx.notify();
                            }
                        } else if this.hovered_tooltip.as_ref() == Some(&account_hover) {
                            this.hovered_tooltip = None;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.close_popovers();
                            cx.emit(AppleSidebarEvent::OpenAccountMenu);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(layout.bottom_items.account_tile_px))
                            .h(px(layout.bottom_items.account_tile_px))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(render_server_plush_logo(&account_logo))
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(-1.0))
                                    .right(px(-1.0))
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .rounded_full()
                                    .bg(if self.is_connected {
                                        ob.success()
                                    } else {
                                        ob.text_dim()
                                    })
                                    .border_1()
                                    .border_color(frame_bg),
                            ),
                    )
                    .when(show_account_tip, |el| {
                        el.child(self.render_tooltip_pill(account_label))
                    }),
            );

        div()
            .id("apple-app-sidebar")
            .w(px(layout.width_px))
            .h_full()
            .flex_shrink_0()
            .bg(frame_bg)
            .pb(px(layout.padding_bottom_px))
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .id("apple-sidebar-top-clearance")
                            .w_full()
                            .h(px(layout.padding_top_px))
                            .flex_shrink_0()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(top_group),
            )
            .child(bottom_group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn openbot_page_rail_width_and_narrow_hidden_contracts() {
        assert!((OPENBOT_PAGE_RAIL_WIDTH - 52.0).abs() < f32::EPSILON);
        assert!((EXPANDED_WIDTH - 52.0).abs() < f32::EPSILON);
        assert!((COLLAPSED_WIDTH - 52.0).abs() < f32::EPSILON);

        let mut rail = AppleAppSidebar::new(TdesktopThemePalette::default(), "zh-CN");
        assert!((rail.effective_width() - 52.0).abs() < f32::EPSILON);
        rail.set_narrow_hidden(true);
        assert!((rail.effective_width() - 0.0).abs() < f32::EPSILON);
        rail.set_narrow_hidden(false);
        assert!((rail.effective_width() - 52.0).abs() < f32::EPSILON);
    }

    #[std::prelude::v1::test]
    fn page_rail_default_and_pinnable_sections_and_sanitize() {
        assert_eq!(DEFAULT_PINNED_SECTIONS.len(), 6);
        assert_eq!(
            DEFAULT_PINNED_SECTIONS,
            [
                AppleSidebarSection::Workflows,
                AppleSidebarSection::Community,
                AppleSidebarSection::Books,
                AppleSidebarSection::Music,
                AppleSidebarSection::Games,
                AppleSidebarSection::AppStore,
            ]
        );
        assert_eq!(
            AppleSidebarSection::Workflows.icon_name(),
            IconName::WorkflowWaypoints
        );
        assert_eq!(PINNABLE_SECTIONS.len(), 16);
        assert_eq!(PINNABLE_SECTIONS[0], AppleSidebarSection::Workflows);
        assert!(!AppleSidebarSection::Chats.is_pinnable());
        assert!(!PINNABLE_SECTIONS.contains(&AppleSidebarSection::Chats));

        let sanitized = sanitize_pinned_sections(vec![
            AppleSidebarSection::Chats,
            AppleSidebarSection::Books,
            AppleSidebarSection::Browser,
            AppleSidebarSection::Books,
            AppleSidebarSection::Chats,
        ]);
        assert_eq!(
            sanitized,
            vec![AppleSidebarSection::Books, AppleSidebarSection::Browser]
        );
    }

    #[std::prelude::v1::test]
    fn page_rail_pins_atomic_persistence_round_trip_empty_and_corrupt_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Absent file falls back to DEFAULT_PINNED_SECTIONS (with Workflows first)
        assert_eq!(
            load_page_rail_pins(dir.path()),
            DEFAULT_PINNED_SECTIONS.to_vec()
        );

        // Legacy v1 file with untouched v1 defaults transparently promotes Workflows to index 0
        let legacy_v1 = serde_json::json!({
            "version": 1,
            "pinned_sections": ["community", "books", "music", "games", "app_store"],
            "hidden_bottom_items": []
        });
        fs::write(
            dir.path().join(PAGE_RAIL_PINS_FILE),
            serde_json::to_vec(&legacy_v1).unwrap(),
        )
        .expect("write legacy v1");
        assert_eq!(
            load_page_rail_pins(dir.path()),
            DEFAULT_PINNED_SECTIONS.to_vec()
        );

        // Explicitly unpinning Workflows (saved at version 2) is preserved across reloads
        save_page_rail_pins(dir.path(), &LEGACY_V1_DEFAULT_PINNED_SECTIONS).expect("save v2");
        assert_eq!(
            load_page_rail_pins(dir.path()),
            LEGACY_V1_DEFAULT_PINNED_SECTIONS.to_vec()
        );

        // Save custom subset (with duplicate and non-pinnable Chats) -> round-trips cleanly with zero .tmp files
        let custom = vec![
            AppleSidebarSection::Chats,
            AppleSidebarSection::Browser,
            AppleSidebarSection::Workflows,
            AppleSidebarSection::Browser,
        ];
        save_page_rail_pins(dir.path(), &custom).expect("save pins");
        assert_eq!(
            load_page_rail_pins(dir.path()),
            vec![AppleSidebarSection::Browser, AppleSidebarSection::Workflows]
        );
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "zero .tmp leftovers allowed");

        // Explicit empty pin list is preserved on reload (NOT overwritten with defaults)
        save_page_rail_pins(dir.path(), &[]).expect("save empty pins");
        assert!(load_page_rail_pins(dir.path()).is_empty());

        // Corrupt JSON falls back cleanly to DEFAULT_PINNED_SECTIONS
        fs::write(dir.path().join(PAGE_RAIL_PINS_FILE), b"{not-json").expect("write corrupt");
        assert_eq!(
            load_page_rail_pins(dir.path()),
            DEFAULT_PINNED_SECTIONS.to_vec()
        );
    }

    #[std::prelude::v1::test]
    fn current_in_more_menu_reflects_unpinned_active_destination() {
        let mut rail = AppleAppSidebar::new(TdesktopThemePalette::default(), "en");
        assert!(!rail.current_in_more_menu());

        rail.set_highlighted_destination(Some(AppleSidebarSection::Workflows));
        assert!(!rail.current_in_more_menu());

        rail.set_highlighted_destination(Some(AppleSidebarSection::Books));
        assert!(!rail.current_in_more_menu());

        rail.set_highlighted_destination(Some(AppleSidebarSection::Schedule));
        assert!(rail.current_in_more_menu());

        rail.set_highlighted_destination(Some(AppleSidebarSection::Browser));
        assert!(rail.current_in_more_menu());

        rail.set_highlighted_destination(Some(AppleSidebarSection::Chats));
        assert!(!rail.current_in_more_menu());
    }
}
