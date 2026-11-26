//! 1:1 OpenBot Shell State, PageRail, ServerRail, Reorderable Sidebar Layout & Atomic Persistence (`OB-F14`).
//!
//! Mirrors upstream `nightly-labs/openbot`:
//! - `src/renderer/src/layout-constants.ts`
//! - `packages/ui/src/features/window/PageRail.tsx`
//! - `packages/ui/src/features/servers/ServerRail.tsx`
//! - `packages/contracts/src/ipc-sidebar-layout.ts`
//! - `packages/ui/src/features/sidebar/{Sidebar.tsx, SidebarNav.tsx}`

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{point, px};
use openbot_contracts::limits::INPUT_LIMITS;
use openbot_contracts::providers::AgentProviderId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{AppLogoState, AppLogoVariant, ServerAppearance};
use crate::theme::openbot_palette::{BEAM_MS, ThemeTransitionSpec};

use super::agent_studio::{AgentSettingsState, FirstAgentSetupState, GlobalSearchState};

/// Default expanded workspace sidebar width (`layout-constants.ts:L8`).
pub const LEFT_PANEL_DEFAULT: f32 = 280.0;
/// Minimum expanded workspace sidebar width (`layout-constants.ts:L9`).
pub const LEFT_PANEL_MIN: f32 = 128.0;
/// Maximum expanded workspace sidebar width (`layout-constants.ts:L10`).
pub const LEFT_PANEL_MAX: f32 = 400.0;
/// Compact avatar-only workspace sidebar width (`layout-constants.ts:L11`).
pub const LEFT_PANEL_COMPACT: f32 = 88.0;
/// Drag width below which the sidebar collapses to compact `88px` (`layout-constants.ts:L12`).
pub const LEFT_PANEL_COLLAPSE_THRESHOLD: f32 = 110.0;
/// Drag width above which a compact sidebar expands (`layout-constants.ts:L13`).
pub const LEFT_PANEL_EXPAND_THRESHOLD: f32 = 118.0;
/// Minimum conversation stage width before sidebar auto-compacts (`layout-constants.ts:L14`).
pub const CONVERSATION_MIN_WIDTH: f32 = 424.0;
/// macOS ServerRail width (`layout-constants.ts:L15`).
pub const MAC_SERVER_RAIL_WIDTH: f32 = 72.0;
/// Linux/Windows desktop ServerRail width (`layout-constants.ts:L16`).
pub const SERVER_RAIL_WIDTH: f32 = 64.0;
/// Narrow window (`<= 800px`) ServerRail width (`layout-constants.ts:L17`).
pub const NARROW_SERVER_RAIL_WIDTH: f32 = 56.0;
/// Narrow viewport width breakpoint in pixels.
pub const NARROW_WINDOW_BREAKPOINT_PX: u32 = 800;
/// Default PageRail width in pixels.
pub const PAGE_RAIL_WIDTH_PX: f32 = 56.0;
/// Canonical People section identifier (`ipc-sidebar-layout.ts`).
pub const SIDEBAR_PEOPLE_SECTION_ID: &str = "people";
/// Canonical Unassigned section identifier (`ipc-sidebar-layout.ts`).
pub const SIDEBAR_UNASSIGNED_SECTION_ID: &str = "unassigned";
/// Maximum nested folder depth levels (`0, 1, 2` -> `3` levels).
pub const SIDEBAR_FOLDER_LEVELS: usize = 3;
/// Maximum sidebar sections (`INPUT_LIMITS.sidebar_sections == 100`).
pub const SIDEBAR_MAX_SECTIONS: usize = INPUT_LIMITS.sidebar_sections;
/// Maximum sidebar section name characters (`INPUT_LIMITS.sidebar_section_name == 40`).
pub const SIDEBAR_MAX_SECTION_NAME: usize = INPUT_LIMITS.sidebar_section_name;
/// Maximum sidebar pins allowed (`SIDEBAR_PIN_LIMIT == 1000`).
pub const SIDEBAR_MAX_PINS: usize = 1_000;
/// Maximum mini server previews shown inside the 2x2 `HiddenServerFolder` tile.
pub const HIDDEN_FOLDER_PREVIEW_COUNT: usize = 4;
/// Minimum width of `AgentSettingsPanel` in pixels.
pub const AGENT_SETTINGS_MIN_PX: f32 = 360.0;
/// Pinned agent/channel tile size in `SidebarPinnedGroup`.
pub const SIDEBAR_PINNED_TILE_PX: f32 = 56.0;
/// Duration of the `beam()` server-switch radial wave (`1400ms`, `0ms` reduced motion).
pub const SERVER_BEAM_DURATION_MS: u32 = BEAM_MS;
/// Atomic sidebar preferences JSON file name.
pub const SIDEBAR_PREF_FILE: &str = "openbot-sidebar-v1.json";
/// Atomic page/server rail preferences JSON file name.
pub const RAIL_PREF_FILE: &str = "openbot-rail-v1.json";
/// Exact vertical group order rendered in `SidebarNav`.
pub const SIDEBAR_GROUP_ORDER: [&str; 5] = ["pinned", "waiting", "unread", "sections", "hidden"];

/// Domain validation error for OpenBot shell, sidebar layout, pins, and agent studio forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellValidationError {
    pub code: &'static str,
    pub message: String,
}

impl ShellValidationError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ShellValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ShellValidationError {}

/// Exact 11 `PageRail` page destinations (`PageRail.tsx:L440-L452`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenBotPage {
    #[default]
    Agents,
    Workflows,
    Tasks,
    Schedule,
    Usage,
    Library,
    Providers,
    Crm,
    Marketplace,
    Credits,
    Settings,
}

impl OpenBotPage {
    pub const ALL: [Self; 11] = [
        Self::Agents,
        Self::Workflows,
        Self::Tasks,
        Self::Schedule,
        Self::Usage,
        Self::Library,
        Self::Providers,
        Self::Crm,
        Self::Marketplace,
        Self::Credits,
        Self::Settings,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Workflows => "workflows",
            Self::Tasks => "tasks",
            Self::Schedule => "schedule",
            Self::Usage => "usage",
            Self::Library => "library",
            Self::Providers => "providers",
            Self::Crm => "crm",
            Self::Marketplace => "marketplace",
            Self::Credits => "credits",
            Self::Settings => "settings",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Agents => "Agents",
            Self::Workflows => "Workflows",
            Self::Tasks => "Tasks",
            Self::Schedule => "Schedule",
            Self::Usage => "Usage",
            Self::Library => "Library",
            Self::Providers => "Providers",
            Self::Crm => "CRM",
            Self::Marketplace => "Marketplace",
            Self::Credits => "Credits",
            Self::Settings => "Settings",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|p| p.as_str() == lower)
    }

    pub fn is_pinnable(self) -> bool {
        PAGE_RAIL_PINNABLE_PAGES.contains(&self)
    }
}

/// Exact 7 pinnable pages in `PageRail` (`PageRail.tsx:L454-L462`).
pub const PAGE_RAIL_PINNABLE_PAGES: [OpenBotPage; 7] = [
    OpenBotPage::Workflows,
    OpenBotPage::Tasks,
    OpenBotPage::Schedule,
    OpenBotPage::Usage,
    OpenBotPage::Library,
    OpenBotPage::Providers,
    OpenBotPage::Crm,
];

/// Default 5 pinned pages on initial workspace load (`TEST-T1-OB-F14-01`).
pub const PAGE_RAIL_DEFAULT_PINNED_PAGES: [OpenBotPage; 5] = [
    OpenBotPage::Tasks,
    OpenBotPage::Workflows,
    OpenBotPage::Usage,
    OpenBotPage::Library,
    OpenBotPage::Crm,
];

/// Exact 3 hideable bottom `PageRail` items (`PageRail.tsx:L465`).
pub const PAGE_RAIL_HIDEABLE_ITEMS: [&str; 3] = ["credits", "marketplace", "help"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageRailHideableItem {
    Credits,
    Marketplace,
    Help,
}

impl PageRailHideableItem {
    pub const ALL: [Self; 3] = [Self::Credits, Self::Marketplace, Self::Help];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Credits => "credits",
            Self::Marketplace => "marketplace",
            Self::Help => "help",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Credits => "Credits",
            Self::Marketplace => "Marketplace",
            Self::Help => "Help",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|item| item.as_str() == lower)
    }
}

/// Dual concentric provider quota ring dimensions (`PageRail.tsx:L160-L237`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UsageRingSpec {
    pub size_px: f32,
    pub stroke_px: f32,
    pub spacing_px: f32,
}

impl UsageRingSpec {
    pub const DEFAULT: Self = Self {
        size_px: 34.0,
        stroke_px: 2.25,
        spacing_px: 4.5,
    };
}

impl Default for UsageRingSpec {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Live quota sample rendered inside a `PageRail` `UsageRings` button and hover `UsageCard`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRingMetric {
    pub provider: AgentProviderId,
    pub label: String,
    /// Outer arc utilization fraction in `0.0..=1.0` (e.g. 5-hour session quota).
    pub primary_ratio: Option<f32>,
    /// Inner arc utilization fraction in `0.0..=1.0` (e.g. weekly quota).
    pub secondary_ratio: Option<f32>,
    /// When `true`, renders dashed `4 6` fallback rings.
    pub unavailable: bool,
    pub reset_label: Option<String>,
    pub detail_summary: Option<String>,
}

impl UsageRingMetric {
    pub fn new(
        provider: AgentProviderId,
        primary_ratio: Option<f32>,
        secondary_ratio: Option<f32>,
        unavailable: bool,
    ) -> Self {
        Self {
            provider,
            label: provider.descriptor().display_name.to_string(),
            primary_ratio: primary_ratio.map(|v| v.clamp(0.0, 1.0)),
            secondary_ratio: secondary_ratio.map(|v| v.clamp(0.0, 1.0)),
            unavailable,
            reset_label: None,
            detail_summary: None,
        }
    }
}

/// State model for the far-left `PageRail` (`PageRail.tsx:L160-L868`).
#[derive(Debug, Clone, PartialEq)]
pub struct PageRailState {
    pub active_page: OpenBotPage,
    pub pinned_pages: Vec<OpenBotPage>,
    pub hidden_items: Vec<PageRailHideableItem>,
    pub more_menu_open: bool,
    pub help_menu_open: bool,
    pub context_menu_open: bool,
    pub active_usage_hover_provider: Option<AgentProviderId>,
    pub usage_ring_spec: UsageRingSpec,
    pub update_notice: bool,
    pub credits_balance_label: Option<String>,
    pub media_credits_label: Option<String>,
    pub show_server_slot: bool,
    pub show_app_logo: bool,
    pub app_logo: AppLogoState,
    pub preloaded_workflows: bool,
}

impl Default for PageRailState {
    fn default() -> Self {
        Self::new()
    }
}

impl PageRailState {
    pub fn new() -> Self {
        Self {
            active_page: OpenBotPage::Agents,
            pinned_pages: PAGE_RAIL_DEFAULT_PINNED_PAGES.to_vec(),
            hidden_items: Vec::new(),
            more_menu_open: false,
            help_menu_open: false,
            context_menu_open: false,
            active_usage_hover_provider: None,
            usage_ring_spec: UsageRingSpec::DEFAULT,
            update_notice: false,
            credits_balance_label: Some("100 cr".to_string()),
            media_credits_label: None,
            show_server_slot: false,
            show_app_logo: true,
            app_logo: AppLogoState::new(AppLogoVariant::Production),
            preloaded_workflows: false,
        }
    }

    pub fn is_pinned(&self, page: OpenBotPage) -> bool {
        self.pinned_pages.contains(&page)
    }

    pub fn toggle_pin(&mut self, page: OpenBotPage) -> bool {
        if !page.is_pinnable() {
            return false;
        }
        if let Some(pos) = self.pinned_pages.iter().position(|p| *p == page) {
            self.pinned_pages.remove(pos);
            false
        } else {
            self.pinned_pages.push(page);
            true
        }
    }

    pub fn set_pinned_pages(&mut self, pages: Vec<OpenBotPage>) {
        let mut unique = Vec::new();
        for page in pages {
            if page.is_pinnable() && !unique.contains(&page) {
                unique.push(page);
            }
        }
        self.pinned_pages = unique;
    }

    pub fn unpinned_pinnable_pages(&self) -> Vec<OpenBotPage> {
        PAGE_RAIL_PINNABLE_PAGES
            .into_iter()
            .filter(|page| !self.pinned_pages.contains(page))
            .collect()
    }

    pub fn is_hidden(&self, item: PageRailHideableItem) -> bool {
        self.hidden_items.contains(&item)
    }

    pub fn hide_item(&mut self, item: PageRailHideableItem) {
        if !self.hidden_items.contains(&item) {
            self.hidden_items.push(item);
        }
    }

    pub fn show_item(&mut self, item: PageRailHideableItem) {
        self.hidden_items.retain(|existing| *existing != item);
    }

    pub fn set_hidden_items(&mut self, items: Vec<PageRailHideableItem>) {
        let mut unique = Vec::new();
        for item in items {
            if !unique.contains(&item) {
                unique.push(item);
            }
        }
        self.hidden_items = unique;
    }

    pub fn hover_workflows(&mut self) {
        self.preloaded_workflows = true;
    }

    /// Splits provider `UsageRings` into directly visible rings and overflow rings based on vertical budget.
    pub fn partition_usage_rings<'a>(
        &self,
        rings: &'a [UsageRingMetric],
        available_height_px: f32,
    ) -> (&'a [UsageRingMetric], &'a [UsageRingMetric]) {
        let slot_h = self.usage_ring_spec.size_px + 6.0;
        let max_slots = ((available_height_px.max(slot_h)) / slot_h).floor() as usize;
        if rings.len() <= max_slots {
            (rings, &[])
        } else {
            let visible_count = max_slots.saturating_sub(1).max(1).min(rings.len());
            rings.split_at(visible_count)
        }
    }
}

/// Live connection state of a local or remote OpenBot server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerConnectionState {
    #[default]
    Online,
    Connecting,
    Offline,
    Error,
    Incompatible,
}

impl ServerConnectionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Connecting => "connecting",
            Self::Offline => "offline",
            Self::Error => "error",
            Self::Incompatible => "incompatible",
        }
    }
}

/// Hosted/remote server sleep indicator (`ServerSleepDot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerSleepState {
    #[default]
    Awake,
    Sleeping,
    Waking,
}

impl ServerSleepState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Awake => "awake",
            Self::Sleeping => "sleeping",
            Self::Waking => "waking",
        }
    }
}

/// Server selector placement (`openbot:server-view`: rail vs topbar dropdown menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerViewMode {
    #[default]
    Rail,
    Menu,
}

impl ServerViewMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rail => "rail",
            Self::Menu => "menu",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rail" => Some(Self::Rail),
            "menu" | "dropdown" => Some(Self::Menu),
            _ => None,
        }
    }
}

/// Target desktop platform used for responsive `ServerRail` & titlebar inset sizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerPlatform {
    #[serde(rename = "darwin")]
    MacOs,
    #[default]
    #[serde(rename = "linux")]
    Linux,
    #[serde(rename = "win32")]
    Windows,
}

impl ServerPlatform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "darwin",
            Self::Linux => "linux",
            Self::Windows => "win32",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "darwin" | "macos" | "mac" => Some(Self::MacOs),
            "linux" => Some(Self::Linux),
            "win32" | "windows" | "win" => Some(Self::Windows),
            _ => None,
        }
    }

    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Linux
        }
    }
}

/// Local or remote OpenBot server shown in `ServerRail` or `ServerMenu`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    pub id: String,
    pub name: String,
    pub is_local: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub unread_count: u64,
    #[serde(default)]
    pub waiting_count: u64,
    #[serde(default)]
    pub connection: ServerConnectionState,
    #[serde(default)]
    pub sleep_state: ServerSleepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<ServerAppearance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_label: Option<String>,
    #[serde(default)]
    pub update_available: bool,
}

impl ServerEntry {
    pub fn local_default() -> Self {
        Self {
            id: "local".to_string(),
            name: "Local OpenBot".to_string(),
            is_local: true,
            hidden: false,
            muted: false,
            unread_count: 0,
            waiting_count: 0,
            connection: ServerConnectionState::Online,
            sleep_state: ServerSleepState::Awake,
            appearance: None,
            host_label: Some("127.0.0.1".to_string()),
            update_available: false,
        }
    }
}

/// State model for the `ServerRail` (`ServerRail.tsx:L1-L250`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRailState {
    pub view_mode: ServerViewMode,
    pub active_server_id: String,
    pub servers: Vec<ServerEntry>,
    pub hidden_folder_open: bool,
    pub dragging_server_id: Option<String>,
    pub drag_target_index: Option<usize>,
    pub last_beam_origin_server_id: Option<String>,
}

impl Default for ServerRailState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerRailState {
    pub fn new() -> Self {
        Self {
            view_mode: ServerViewMode::Rail,
            active_server_id: "local".to_string(),
            servers: vec![ServerEntry::local_default()],
            hidden_folder_open: false,
            dragging_server_id: None,
            drag_target_index: None,
            last_beam_origin_server_id: None,
        }
    }

    /// Exact responsive `ServerRail` width (`0`, `56`, `64`, or `72` px).
    pub const fn effective_width_px(&self, platform: ServerPlatform, window_width_px: u32) -> f32 {
        match self.view_mode {
            ServerViewMode::Menu => 0.0,
            ServerViewMode::Rail => {
                if window_width_px <= NARROW_WINDOW_BREAKPOINT_PX {
                    NARROW_SERVER_RAIL_WIDTH
                } else {
                    match platform {
                        ServerPlatform::MacOs => MAC_SERVER_RAIL_WIDTH,
                        ServerPlatform::Linux | ServerPlatform::Windows => SERVER_RAIL_WIDTH,
                    }
                }
            }
        }
    }

    /// Server-switch `beam()` duration (`1400ms` normal, `0ms` under `reduced_motion`).
    pub const fn beam_duration_ms(reduced_motion: bool) -> u32 {
        if reduced_motion {
            0
        } else {
            SERVER_BEAM_DURATION_MS
        }
    }

    pub fn local_servers(&self) -> Vec<&ServerEntry> {
        self.servers
            .iter()
            .filter(|s| s.is_local && !s.hidden)
            .collect()
    }

    pub fn visible_remote_servers(&self) -> Vec<&ServerEntry> {
        self.servers
            .iter()
            .filter(|s| !s.is_local && !s.hidden)
            .collect()
    }

    pub fn hidden_servers(&self) -> Vec<&ServerEntry> {
        self.servers.iter().filter(|s| s.hidden).collect()
    }

    pub fn hidden_folder_preview(&self) -> Vec<&ServerEntry> {
        self.servers
            .iter()
            .filter(|s| s.hidden)
            .take(HIDDEN_FOLDER_PREVIEW_COUNT)
            .collect()
    }

    pub fn active_server(&self) -> Option<&ServerEntry> {
        self.servers
            .iter()
            .find(|s| s.id == self.active_server_id)
            .or_else(|| self.servers.first())
    }

    /// Reorders a visible remote server while keeping local servers strictly separated.
    pub fn reorder_remote_server(&mut self, server_id: &str, target_remote_index: usize) -> bool {
        let remote_positions: Vec<usize> = self
            .servers
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.is_local && !s.hidden)
            .map(|(idx, _)| idx)
            .collect();
        let Some(current_remote_pos) = remote_positions
            .iter()
            .position(|&idx| self.servers[idx].id == server_id)
        else {
            return false;
        };
        let clamped_target = target_remote_index.min(remote_positions.len().saturating_sub(1));
        if current_remote_pos == clamped_target {
            return false;
        }
        let mut remotes: Vec<ServerEntry> = Vec::with_capacity(remote_positions.len());
        for &idx in &remote_positions {
            remotes.push(self.servers[idx].clone());
        }
        let moved = remotes.remove(current_remote_pos);
        remotes.insert(clamped_target, moved);
        for (slot, entry) in remote_positions.into_iter().zip(remotes.into_iter()) {
            self.servers[slot] = entry;
        }
        true
    }

    /// Switches active server and produces the `beam()` transition spec (`1400ms` or `0ms`).
    pub fn select_server(
        &mut self,
        server_id: &str,
        origin_y_px: f32,
        reduced_motion: bool,
    ) -> ThemeTransitionSpec {
        self.last_beam_origin_server_id = Some(self.active_server_id.clone());
        if self.servers.iter().any(|s| s.id == server_id) {
            self.active_server_id = server_id.to_string();
        }
        ThemeTransitionSpec::beam(point(px(32.0), px(origin_y_px)), true, reduced_motion)
    }

    pub fn toggle_hidden(&mut self, server_id: &str) -> bool {
        if let Some(srv) = self.servers.iter_mut().find(|s| s.id == server_id) {
            srv.hidden = !srv.hidden;
            return true;
        }
        false
    }

    pub fn toggle_mute(&mut self, server_id: &str) -> bool {
        if let Some(srv) = self.servers.iter_mut().find(|s| s.id == server_id) {
            srv.muted = !srv.muted;
            return true;
        }
        false
    }
}

/// Nested sidebar folder section (`ipc-sidebar-layout.ts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarSection {
    pub id: String,
    pub name: String,
    #[serde(
        rename = "parentId",
        alias = "parent_id",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub collapsed: bool,
}

/// Full reorderable sidebar layout snapshot (`ipc-sidebar-layout.ts:L1-L138`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarLayoutSnapshot {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub sections: Vec<SidebarSection>,
    pub order: Vec<String>,
    #[serde(
        rename = "agentAssignments",
        alias = "agent_assignments",
        default
    )]
    pub agent_assignments: BTreeMap<String, String>,
    #[serde(rename = "agentOrder", alias = "agent_order", default)]
    pub agent_order: Vec<String>,
}

impl Default for SidebarLayoutSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            sections: Vec::new(),
            order: vec![
                SIDEBAR_UNASSIGNED_SECTION_ID.to_string(),
                SIDEBAR_PEOPLE_SECTION_ID.to_string(),
            ],
            agent_assignments: BTreeMap::new(),
            agent_order: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarMoveDirection {
    Up,
    Down,
}

/// All 7 `SidebarLayoutAction`s (`ipc-sidebar-layout.ts:L58-L65`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum SidebarLayoutAction {
    Create {
        id: String,
        name: String,
        #[serde(rename = "parentId", alias = "parent_id", default)]
        parent_id: Option<String>,
    },
    Rename {
        #[serde(rename = "sectionId", alias = "section_id")]
        section_id: String,
        name: String,
    },
    Delete {
        #[serde(rename = "sectionId", alias = "section_id")]
        section_id: String,
    },
    Move {
        #[serde(rename = "sectionId", alias = "section_id")]
        section_id: String,
        direction: SidebarMoveDirection,
        #[serde(default)]
        steps: Option<usize>,
    },
    Assign {
        #[serde(rename = "agentId", alias = "agent_id")]
        agent_id: String,
        #[serde(rename = "sectionId", alias = "section_id", default)]
        section_id: Option<String>,
    },
    MoveAgent {
        #[serde(rename = "agentId", alias = "agent_id")]
        agent_id: String,
        #[serde(rename = "sectionId", alias = "section_id", default)]
        section_id: Option<String>,
        #[serde(rename = "beforeAgentId", alias = "before_agent_id", default)]
        before_agent_id: Option<String>,
    },
    Nest {
        #[serde(rename = "sectionId", alias = "section_id")]
        section_id: String,
        #[serde(rename = "parentId", alias = "parent_id", default)]
        parent_id: Option<String>,
        #[serde(rename = "beforeSectionId", alias = "before_section_id", default)]
        before_section_id: Option<String>,
    },
}

/// Validates `SidebarLayoutSnapshot` invariants (`ipc-sidebar-layout.ts:L1-L138`):
/// - `sections.len() <= 100`
/// - every section `name` non-empty and `<= 40` characters
/// - unique section IDs, excluding reserved `"people"` / `"unassigned"`
/// - `order` contains exact unique set `{ "people", "unassigned", ...sectionIds }`
/// - `agent_order` has unique agent IDs
pub fn validate_sidebar_layout(layout: &SidebarLayoutSnapshot) -> Result<(), ShellValidationError> {
    if layout.sections.len() > SIDEBAR_MAX_SECTIONS {
        return Err(ShellValidationError::new(
            "sidebar_sections_limit",
            format!(
                "sidebar layout has too many sections ({}): maximum limit is {}",
                layout.sections.len(),
                SIDEBAR_MAX_SECTIONS
            ),
        ));
    }

    let mut section_ids: HashSet<&str> = HashSet::with_capacity(layout.sections.len());
    for section in &layout.sections {
        let id = section.id.trim();
        if id.is_empty()
            || id == SIDEBAR_PEOPLE_SECTION_ID
            || id == SIDEBAR_UNASSIGNED_SECTION_ID
        {
            return Err(ShellValidationError::new(
                "invalid_section_id",
                format!("invalid sidebar section id '{}'", section.id),
            ));
        }
        if !section_ids.insert(section.id.as_str()) {
            return Err(ShellValidationError::new(
                "duplicate_section_id",
                format!("duplicate sidebar section id '{}'", section.id),
            ));
        }
        let name_len = section.name.trim().chars().count();
        if name_len == 0 {
            return Err(ShellValidationError::new(
                "empty_section_name",
                format!("sidebar section '{}' name cannot be empty", section.id),
            ));
        }
        if section.name.chars().count() > SIDEBAR_MAX_SECTION_NAME {
            return Err(ShellValidationError::new(
                "section_name_too_long",
                format!(
                    "sidebar section '{}' name length ({}) exceeds limit of {} characters",
                    section.id,
                    section.name.chars().count(),
                    SIDEBAR_MAX_SECTION_NAME
                ),
            ));
        }
    }

    let mut seen_order: HashSet<&str> = HashSet::with_capacity(layout.order.len());
    for entry in &layout.order {
        if !seen_order.insert(entry.as_str()) {
            return Err(ShellValidationError::new(
                "duplicate_order_entry",
                format!("sidebar layout order contains duplicate entry '{entry}'"),
            ));
        }
        if entry != SIDEBAR_PEOPLE_SECTION_ID
            && entry != SIDEBAR_UNASSIGNED_SECTION_ID
            && !section_ids.contains(entry.as_str())
        {
            return Err(ShellValidationError::new(
                "unknown_order_entry",
                format!("sidebar layout order references non-existent section '{entry}'"),
            ));
        }
    }

    if !seen_order.contains(SIDEBAR_UNASSIGNED_SECTION_ID) {
        return Err(ShellValidationError::new(
            "missing_unassigned_section",
            "sidebar layout order is incomplete: missing required 'unassigned' section",
        ));
    }
    if !seen_order.contains(SIDEBAR_PEOPLE_SECTION_ID) {
        return Err(ShellValidationError::new(
            "missing_people_section",
            "sidebar layout order is incomplete: missing required 'people' section",
        ));
    }
    for sec_id in &section_ids {
        if !seen_order.contains(sec_id) {
            return Err(ShellValidationError::new(
                "missing_section_in_order",
                format!("sidebar layout order is incomplete: missing section '{sec_id}'"),
            ));
        }
    }

    let mut seen_agents: HashSet<&str> = HashSet::with_capacity(layout.agent_order.len());
    for agent_id in &layout.agent_order {
        if !seen_agents.insert(agent_id.as_str()) {
            return Err(ShellValidationError::new(
                "duplicate_agent_order",
                format!("sidebar layout agentOrder contains duplicate agent '{agent_id}'"),
            ));
        }
    }

    Ok(())
}

/// Resolves cycle-safe nested folder depths clamped to `0..=(SIDEBAR_FOLDER_LEVELS - 1)` (`0..=2`).
pub fn resolve_folder_depths(sections: &[SidebarSection]) -> BTreeMap<String, usize> {
    let max_depth = SIDEBAR_FOLDER_LEVELS.saturating_sub(1);
    let parent_by_id: BTreeMap<&str, Option<&str>> = sections
        .iter()
        .map(|s| (s.id.as_str(), s.parent_id.as_deref()))
        .collect();

    let mut depths = BTreeMap::new();
    for section in sections {
        let mut depth = 0usize;
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(section.id.as_str());
        let mut current = section.parent_id.as_deref();
        let mut broke_cycle = false;

        while let Some(parent_id) = current {
            if !parent_by_id.contains_key(parent_id) {
                break;
            }
            if !visited.insert(parent_id) {
                broke_cycle = true;
                break;
            }
            depth += 1;
            current = parent_by_id.get(parent_id).copied().flatten();
        }

        let resolved = if broke_cycle { 0 } else { depth.min(max_depth) };
        depths.insert(section.id.clone(), resolved);
    }
    depths
}

fn would_create_folder_cycle(
    sections: &[SidebarSection],
    section_id: &str,
    candidate_parent: Option<&str>,
) -> bool {
    let mut visited = HashSet::new();
    visited.insert(section_id);
    let mut cur = candidate_parent;
    while let Some(pid) = cur {
        if !visited.insert(pid) {
            return true;
        }
        cur = sections
            .iter()
            .find(|s| s.id == pid)
            .and_then(|s| s.parent_id.as_deref());
    }
    false
}

/// Applies one of the 7 `SidebarLayoutAction`s cleanly, preserving all layout invariants.
pub fn apply_sidebar_layout_action(
    layout: &mut SidebarLayoutSnapshot,
    action: SidebarLayoutAction,
) -> Result<(), ShellValidationError> {
    match action {
        SidebarLayoutAction::Create {
            id,
            name,
            parent_id,
        } => {
            if layout.sections.len() >= SIDEBAR_MAX_SECTIONS {
                return Err(ShellValidationError::new(
                    "sidebar_sections_limit",
                    format!("cannot create section: limit of {SIDEBAR_MAX_SECTIONS} reached"),
                ));
            }
            let valid_parent = parent_id.filter(|pid| {
                layout.sections.iter().any(|s| s.id == *pid)
                    && resolve_folder_depths(&layout.sections)
                        .get(pid)
                        .copied()
                        .unwrap_or(0)
                        < SIDEBAR_FOLDER_LEVELS.saturating_sub(1)
            });
            let mut next = layout.clone();
            next.sections.push(SidebarSection {
                id: id.clone(),
                name,
                parent_id: valid_parent,
                collapsed: false,
            });
            let insert_at = next
                .order
                .iter()
                .position(|item| item == SIDEBAR_UNASSIGNED_SECTION_ID)
                .unwrap_or(next.order.len());
            next.order.insert(insert_at, id);
            validate_sidebar_layout(&next)?;
            next.revision = next.revision.saturating_add(1);
            *layout = next;
        }
        SidebarLayoutAction::Rename { section_id, name } => {
            let trimmed_len = name.trim().chars().count();
            if trimmed_len == 0 || name.chars().count() > SIDEBAR_MAX_SECTION_NAME {
                return Err(ShellValidationError::new(
                    "section_name_too_long",
                    format!(
                        "sidebar section name length must be 1..={SIDEBAR_MAX_SECTION_NAME} characters"
                    ),
                ));
            }
            let Some(sec) = layout.sections.iter_mut().find(|s| s.id == section_id) else {
                return Err(ShellValidationError::new(
                    "section_not_found",
                    format!("sidebar section '{section_id}' not found"),
                ));
            };
            sec.name = name;
            layout.revision = layout.revision.saturating_add(1);
        }
        SidebarLayoutAction::Delete { section_id } => {
            let Some(pos) = layout.sections.iter().position(|s| s.id == section_id) else {
                return Err(ShellValidationError::new(
                    "section_not_found",
                    format!("sidebar section '{section_id}' not found"),
                ));
            };
            let deleted_parent = layout.sections[pos].parent_id.clone();
            layout.sections.remove(pos);
            for child in &mut layout.sections {
                if child.parent_id.as_deref() == Some(section_id.as_str()) {
                    child.parent_id = deleted_parent.clone();
                }
            }
            layout.order.retain(|id| id != &section_id);
            layout
                .agent_assignments
                .retain(|_, assigned| assigned != &section_id);
            layout.revision = layout.revision.saturating_add(1);
        }
        SidebarLayoutAction::Move {
            section_id,
            direction,
            steps,
        } => {
            let Some(idx) = layout.order.iter().position(|id| id == &section_id) else {
                return Err(ShellValidationError::new(
                    "section_not_found",
                    format!("sidebar section '{section_id}' not found in order"),
                ));
            };
            let delta = steps.unwrap_or(1).max(1);
            let target = match direction {
                SidebarMoveDirection::Up => idx.saturating_sub(delta),
                SidebarMoveDirection::Down => {
                    (idx + delta).min(layout.order.len().saturating_sub(1))
                }
            };
            if target != idx {
                let item = layout.order.remove(idx);
                layout.order.insert(target, item);
                layout.revision = layout.revision.saturating_add(1);
            }
        }
        SidebarLayoutAction::Assign {
            agent_id,
            section_id,
        } => {
            if let Some(sec_id) = section_id {
                if !layout.sections.iter().any(|s| s.id == sec_id) {
                    return Err(ShellValidationError::new(
                        "section_not_found",
                        format!("cannot assign agent to missing section '{sec_id}'"),
                    ));
                }
                layout.agent_assignments.insert(agent_id, sec_id);
            } else {
                layout.agent_assignments.remove(&agent_id);
            }
            layout.revision = layout.revision.saturating_add(1);
        }
        SidebarLayoutAction::MoveAgent {
            agent_id,
            section_id,
            before_agent_id,
        } => {
            if let Some(sec_id) = section_id {
                if !layout.sections.iter().any(|s| s.id == sec_id) {
                    return Err(ShellValidationError::new(
                        "section_not_found",
                        format!("cannot move agent to missing section '{sec_id}'"),
                    ));
                }
                layout.agent_assignments.insert(agent_id.clone(), sec_id);
            } else {
                layout.agent_assignments.remove(&agent_id);
            }
            layout.agent_order.retain(|id| id != &agent_id);
            if let Some(before_id) = before_agent_id
                && let Some(pos) = layout.agent_order.iter().position(|id| id == &before_id)
            {
                layout.agent_order.insert(pos, agent_id);
            } else {
                layout.agent_order.push(agent_id);
            }
            layout.revision = layout.revision.saturating_add(1);
        }
        SidebarLayoutAction::Nest {
            section_id,
            parent_id,
            before_section_id,
        } => {
            if !layout.sections.iter().any(|s| s.id == section_id) {
                return Err(ShellValidationError::new(
                    "section_not_found",
                    format!("sidebar section '{section_id}' not found"),
                ));
            }
            let next_parent = parent_id.filter(|pid| {
                pid != &section_id
                    && layout.sections.iter().any(|s| s.id == *pid)
                    && !would_create_folder_cycle(&layout.sections, &section_id, Some(pid))
            });
            if let Some(sec) = layout.sections.iter_mut().find(|s| s.id == section_id) {
                sec.parent_id = next_parent;
            }
            if let Some(before_id) = before_section_id
                && before_id != section_id
                && let Some(old_pos) = layout.order.iter().position(|id| id == &section_id)
            {
                let item = layout.order.remove(old_pos);
                let new_pos = layout
                    .order
                    .iter()
                    .position(|id| id == &before_id)
                    .unwrap_or(layout.order.len());
                layout.order.insert(new_pos, item);
            }
            layout.revision = layout.revision.saturating_add(1);
        }
    }
    Ok(())
}

/// Supported pinned entry kinds in `SidebarPinnedGroup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarPinKind {
    Agent,
    Channel,
    Dm,
}

impl SidebarPinKind {
    pub const ALL: [Self; 3] = [Self::Agent, Self::Channel, Self::Dm];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Channel => "channel",
            Self::Dm => "dm",
        }
    }

    pub fn parse_strict(raw: &str) -> Result<Self, ShellValidationError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "agent" => Ok(Self::Agent),
            "channel" => Ok(Self::Channel),
            "dm" => Ok(Self::Dm),
            other => Err(ShellValidationError::new(
                "invalid_kind",
                format!("unsupported sidebar pin kind '{other}': expected agent, channel, or dm"),
            )),
        }
    }
}

/// Validated pinned item in the `56px` `SidebarPinnedGroup`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SidebarPinEntry {
    pub kind: SidebarPinKind,
    pub id: String,
}

/// Untrusted wire pin entry validated strictly before converting to [`SidebarPinEntry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSidebarPinEntry {
    pub kind: String,
    pub id: String,
}

/// Validates strongly-typed `SidebarPinEntry` entries: unique `(kind, id)`, non-empty ID, `<= 1000`.
pub fn validate_sidebar_pins(pins: &[SidebarPinEntry]) -> Result<(), ShellValidationError> {
    if pins.len() > SIDEBAR_MAX_PINS {
        return Err(ShellValidationError::new(
            "sidebar_pins_limit",
            format!(
                "sidebar pins count ({}) exceeds maximum limit of {}",
                pins.len(),
                SIDEBAR_MAX_PINS
            ),
        ));
    }
    let mut seen: HashSet<(SidebarPinKind, &str)> = HashSet::with_capacity(pins.len());
    for pin in pins {
        if pin.id.trim().is_empty() {
            return Err(ShellValidationError::new(
                "empty_pin_id",
                "sidebar pin id must not be empty",
            ));
        }
        if !seen.insert((pin.kind, pin.id.as_str())) {
            return Err(ShellValidationError::new(
                "duplicate_sidebar_pin",
                format!(
                    "duplicate sidebar pin ({}, '{}'): entries must be unique",
                    pin.kind.as_str(),
                    pin.id
                ),
            ));
        }
    }
    Ok(())
}

/// Parses and validates untrusted raw pin entries (rejects unsupported kinds like `"folder"`).
pub fn validate_raw_sidebar_pins(
    raw_pins: &[RawSidebarPinEntry],
) -> Result<Vec<SidebarPinEntry>, ShellValidationError> {
    if raw_pins.len() > SIDEBAR_MAX_PINS {
        return Err(ShellValidationError::new(
            "sidebar_pins_limit",
            format!(
                "sidebar pins count ({}) exceeds maximum limit of {}",
                raw_pins.len(),
                SIDEBAR_MAX_PINS
            ),
        ));
    }
    let mut parsed = Vec::with_capacity(raw_pins.len());
    for raw in raw_pins {
        let kind = SidebarPinKind::parse_strict(&raw.kind)?;
        parsed.push(SidebarPinEntry {
            kind,
            id: raw.id.clone(),
        });
    }
    validate_sidebar_pins(&parsed)?;
    Ok(parsed)
}

/// Active sidebar list filter (`SidebarFilterMenu`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SidebarFilter {
    #[default]
    All,
    Unread,
    Waiting,
    Working,
    Muted,
    BySection {
        section_id: String,
    },
}

impl SidebarFilter {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unread => "Unread",
            Self::Waiting => "Waiting",
            Self::Working => "Working",
            Self::Muted => "Muted",
            Self::BySection { .. } => "By section",
        }
    }
}

/// Dismissible cards rendered at the bottom of `WorkspaceSidebar`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarBottomCardKind {
    UpdateIsland,
    StorageCard,
    MachineCard,
    MobileAppCard,
    CommunityCard,
}

impl SidebarBottomCardKind {
    pub const ALL: [Self; 5] = [
        Self::UpdateIsland,
        Self::StorageCard,
        Self::MachineCard,
        Self::MobileAppCard,
        Self::CommunityCard,
    ];
}

/// Interactive state for the reorderable `WorkspaceSidebar`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidebarState {
    pub width_px: f32,
    pub compact: bool,
    pub closed: bool,
    pub search_query: String,
    pub filter: SidebarFilter,
    pub filter_menu_open: bool,
    pub new_menu_open: bool,
    pub server_menu_open: bool,
    pub selected_agent_id: Option<String>,
    pub selected_channel_id: Option<String>,
    pub selected_thread_id: Option<String>,
    pub hovered_item_id: Option<String>,
    pub dragging_item_id: Option<String>,
    pub drop_target_section_id: Option<String>,
    pub show_hidden_group: bool,
    pub show_archived_channels: bool,
    pub dismissed_bottom_cards: Vec<SidebarBottomCardKind>,
    pub layout: SidebarLayoutSnapshot,
    pub pins: Vec<SidebarPinEntry>,
    pub history_back_available: bool,
    pub history_forward_available: bool,
    pub server_update_available: bool,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self::new()
    }
}

impl SidebarState {
    pub fn new() -> Self {
        Self {
            width_px: LEFT_PANEL_DEFAULT,
            compact: false,
            closed: false,
            search_query: String::new(),
            filter: SidebarFilter::All,
            filter_menu_open: false,
            new_menu_open: false,
            server_menu_open: false,
            selected_agent_id: None,
            selected_channel_id: None,
            selected_thread_id: None,
            hovered_item_id: None,
            dragging_item_id: None,
            drop_target_section_id: None,
            show_hidden_group: false,
            show_archived_channels: false,
            dismissed_bottom_cards: Vec::new(),
            layout: SidebarLayoutSnapshot::default(),
            pins: Vec::new(),
            history_back_available: false,
            history_forward_available: false,
            server_update_available: false,
        }
    }

    /// Interactive pointer drag-resize hysteresis:
    /// - When expanded: `< 110px` snaps to compact (`88px`); `>= 110px` clamps into `[128, 400]`.
    /// - When compact: stays compact until `> 118px`, then expands clamped into `[128, 400]`.
    pub fn apply_drag_width(&mut self, drag_px: f32) {
        if !drag_px.is_finite() {
            return;
        }
        self.closed = false;
        if self.compact {
            if drag_px > LEFT_PANEL_EXPAND_THRESHOLD {
                self.compact = false;
                self.width_px = drag_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX);
            }
        } else if drag_px < LEFT_PANEL_COLLAPSE_THRESHOLD {
            self.compact = true;
        } else {
            self.width_px = drag_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX);
        }
    }

    /// Stateless threshold snap (`< 110px` -> compact `88px`; `>= 110px` -> expanded `[128, 400]`).
    pub fn snap_drag_width(&mut self, drag_px: f32) {
        if !drag_px.is_finite() {
            return;
        }
        self.closed = false;
        if drag_px < LEFT_PANEL_COLLAPSE_THRESHOLD {
            self.compact = true;
        } else {
            self.compact = false;
            self.width_px = drag_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX);
        }
    }

    /// Auto-compacts when remaining conversation width drops below `CONVERSATION_MIN_WIDTH` (`424px`).
    pub fn apply_window_width_constraint(&mut self, window_width_px: u32, server_rail_px: f32) {
        if self.closed {
            return;
        }
        let expanded_width = self.width_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX);
        let remaining = (window_width_px as f32) - server_rail_px - expanded_width;
        if remaining < CONVERSATION_MIN_WIDTH {
            self.compact = true;
        }
    }

    /// `Cmd/Ctrl+B` closes the sidebar to `0px` or restores the previous open/compact mode.
    pub fn toggle_closed(&mut self) -> bool {
        self.closed = !self.closed;
        self.closed
    }

    /// Effective rendered sidebar width (`0px` closed, `88px` compact, or clamped `[128, 400]`).
    pub fn effective_width_px(&self) -> f32 {
        if self.closed {
            0.0
        } else if self.compact {
            LEFT_PANEL_COMPACT
        } else {
            self.width_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX)
        }
    }

    pub fn is_pinned(&self, kind: SidebarPinKind, id: &str) -> bool {
        self.pins.iter().any(|p| p.kind == kind && p.id == id)
    }

    pub fn toggle_pin(
        &mut self,
        kind: SidebarPinKind,
        id: impl Into<String>,
    ) -> Result<bool, ShellValidationError> {
        let id_str = id.into();
        if let Some(pos) = self
            .pins
            .iter()
            .position(|p| p.kind == kind && p.id == id_str)
        {
            self.pins.remove(pos);
            return Ok(false);
        }
        let mut next = self.pins.clone();
        next.push(SidebarPinEntry { kind, id: id_str });
        validate_sidebar_pins(&next)?;
        self.pins = next;
        Ok(true)
    }

    pub fn dismiss_bottom_card(&mut self, card: SidebarBottomCardKind) {
        if !self.dismissed_bottom_cards.contains(&card) {
            self.dismissed_bottom_cards.push(card);
        }
    }

    pub fn is_bottom_card_dismissed(&self, card: SidebarBottomCardKind) -> bool {
        self.dismissed_bottom_cards.contains(&card)
    }

    pub fn toggle_section_collapsed(&mut self, section_id: &str) -> bool {
        if let Some(sec) = self
            .layout
            .sections
            .iter_mut()
            .find(|s| s.id == section_id)
        {
            sec.collapsed = !sec.collapsed;
            return sec.collapsed;
        }
        false
    }

    pub fn resolved_folder_depths(&self) -> BTreeMap<String, usize> {
        resolve_folder_depths(&self.layout.sections)
    }

    pub fn to_persisted(&self) -> SidebarPersistedState {
        SidebarPersistedState {
            version: 1,
            width_px: self.width_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX),
            compact: self.compact,
            closed: self.closed,
            layout: self.layout.clone(),
            pins: self.pins.clone(),
            dismissed_bottom_cards: self.dismissed_bottom_cards.clone(),
        }
    }

    pub fn apply_persisted(&mut self, persisted: SidebarPersistedState) {
        let width = if persisted.width_px.is_finite() {
            persisted.width_px.clamp(LEFT_PANEL_MIN, LEFT_PANEL_MAX)
        } else {
            LEFT_PANEL_DEFAULT
        };
        self.width_px = width;
        self.compact = persisted.compact;
        self.closed = persisted.closed;
        if validate_sidebar_layout(&persisted.layout).is_ok() {
            self.layout = persisted.layout;
        } else {
            self.layout = SidebarLayoutSnapshot::default();
        }
        if validate_sidebar_pins(&persisted.pins).is_ok() {
            self.pins = persisted.pins;
        } else {
            self.pins = Vec::new();
        }
        self.dismissed_bottom_cards = persisted.dismissed_bottom_cards;
    }
}

/// Persisted schema stored atomically in `<data_dir>/openbot-sidebar-v1.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidebarPersistedState {
    pub version: u32,
    pub width_px: f32,
    pub compact: bool,
    pub closed: bool,
    pub layout: SidebarLayoutSnapshot,
    pub pins: Vec<SidebarPinEntry>,
    #[serde(default)]
    pub dismissed_bottom_cards: Vec<SidebarBottomCardKind>,
}

impl Default for SidebarPersistedState {
    fn default() -> Self {
        Self {
            version: 1,
            width_px: LEFT_PANEL_DEFAULT,
            compact: false,
            closed: false,
            layout: SidebarLayoutSnapshot::default(),
            pins: Vec::new(),
            dismissed_bottom_cards: Vec::new(),
        }
    }
}

/// Persisted schema stored atomically in `<data_dir>/openbot-rail-v1.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailPersistedState {
    pub version: u32,
    pub active_page: OpenBotPage,
    pub pinned_pages: Vec<OpenBotPage>,
    pub hidden_items: Vec<PageRailHideableItem>,
    pub server_view_mode: ServerViewMode,
    pub active_server_id: String,
}

impl Default for RailPersistedState {
    fn default() -> Self {
        Self {
            version: 1,
            active_page: OpenBotPage::Agents,
            pinned_pages: PAGE_RAIL_DEFAULT_PINNED_PAGES.to_vec(),
            hidden_items: Vec::new(),
            server_view_mode: ServerViewMode::Rail,
            active_server_id: "local".to_string(),
        }
    }
}

static ATOMIC_WRITE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Atomically persists JSON by writing to a unique sibling `.tmp` file and renaming into place.
/// Cleans up the `.tmp` file on failure so zero `.tmp` leftovers remain.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_vec_pretty(value)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let seq = ATOMIC_WRITE_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("openbot-pref.json");
    let tmp_path = path.with_file_name(format!(".{file_name}.{}.{seq}.tmp", std::process::id()));
    let write_res = (|| -> std::io::Result<()> {
        fs::write(&tmp_path, &payload)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if write_res.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_res
}

/// Loads a JSON preference file, silently falling back to `T::default()` if missing or corrupt.
pub fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(bytes) = fs::read(path) else {
        return T::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Unified shell state combining `PageRail`, `ServerRail`, `Sidebar`, `GlobalSearch`,
/// `FirstAgentSetup`, and `AgentSettingsPanel` (`OB-F14`).
#[derive(Debug, Clone)]
pub struct OpenBotShellState {
    pub platform: ServerPlatform,
    pub window_width_px: u32,
    pub reduced_motion: bool,
    pub page_rail: PageRailState,
    pub server_rail: ServerRailState,
    pub sidebar: SidebarState,
    pub global_search: GlobalSearchState,
    pub first_agent_setup: FirstAgentSetupState,
    pub agent_settings: AgentSettingsState,
}

impl Default for OpenBotShellState {
    fn default() -> Self {
        Self::new(ServerPlatform::current(), 1280, false)
    }
}

impl OpenBotShellState {
    pub fn new(platform: ServerPlatform, window_width_px: u32, reduced_motion: bool) -> Self {
        let mut page_rail = PageRailState::new();
        page_rail.app_logo.set_reduced_motion(reduced_motion);
        page_rail.show_app_logo = platform != ServerPlatform::MacOs;
        Self {
            platform,
            window_width_px,
            reduced_motion,
            page_rail,
            server_rail: ServerRailState::new(),
            sidebar: SidebarState::new(),
            global_search: GlobalSearchState::new(reduced_motion),
            first_agent_setup: FirstAgentSetupState::first_run(),
            agent_settings: AgentSettingsState::default(),
        }
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        self.page_rail.app_logo.set_reduced_motion(reduced_motion);
        self.global_search.set_reduced_motion(reduced_motion);
    }

    pub fn effective_server_rail_px(&self) -> f32 {
        self.server_rail
            .effective_width_px(self.platform, self.window_width_px)
    }

    pub fn effective_sidebar_px(&self) -> f32 {
        self.sidebar.effective_width_px()
    }

    pub fn beam_duration_ms(&self) -> u32 {
        ServerRailState::beam_duration_ms(self.reduced_motion)
    }

    /// Re-evaluates responsive constraints (`CONVERSATION_MIN_WIDTH = 424px`) for `window_width_px`.
    pub fn update_window_width(&mut self, window_width_px: u32) {
        self.window_width_px = window_width_px;
        let server_px = self.effective_server_rail_px();
        self.sidebar
            .apply_window_width_constraint(window_width_px, server_px);
    }

    /// Loads `openbot-sidebar-v1.json` and `openbot-rail-v1.json`, silently recovering defaults on corruption.
    pub fn load_from_data_dir(&mut self, data_dir: &Path) {
        let sidebar_path = data_dir.join(SIDEBAR_PREF_FILE);
        let sidebar_pref: SidebarPersistedState = load_json_or_default(&sidebar_path);
        self.sidebar.apply_persisted(sidebar_pref);

        let rail_path = data_dir.join(RAIL_PREF_FILE);
        let rail_pref: RailPersistedState = load_json_or_default(&rail_path);
        self.page_rail.active_page = rail_pref.active_page;
        self.page_rail.set_pinned_pages(rail_pref.pinned_pages);
        self.page_rail.set_hidden_items(rail_pref.hidden_items);
        self.server_rail.view_mode = rail_pref.server_view_mode;
        if self
            .server_rail
            .servers
            .iter()
            .any(|s| s.id == rail_pref.active_server_id)
        {
            self.server_rail.active_server_id = rail_pref.active_server_id;
        }
    }

    /// Atomically persists both `openbot-sidebar-v1.json` and `openbot-rail-v1.json` with zero `.tmp` leftovers.
    pub fn save_to_data_dir(&self, data_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let sidebar_path = data_dir.join(SIDEBAR_PREF_FILE);
        write_json_atomic(&sidebar_path, &self.sidebar.to_persisted())?;

        let rail_path = data_dir.join(RAIL_PREF_FILE);
        let rail_pref = RailPersistedState {
            version: 1,
            active_page: self.page_rail.active_page,
            pinned_pages: self.page_rail.pinned_pages.clone(),
            hidden_items: self.page_rail.hidden_items.clone(),
            server_view_mode: self.server_rail.view_mode,
            active_server_id: self.server_rail.active_server_id.clone(),
        };
        write_json_atomic(&rail_path, &rail_pref)?;

        Ok(vec![sidebar_path, rail_path])
    }
}
