//! 14-Tab App Settings Modal, 18-Section Server Settings Modal, and Keyboard Shortcuts Registry (`OB-F18`).
//!
//! Mirrors `SettingsModal.tsx`, `settings-tabs.ts`, `ServerSettingsModal.tsx`, and `keyboard-shortcuts.ts`:
//! - Exact 14 visible App Settings tabs (`12` in detached `chat_window`, omitting `dynamic-island` & `agent-dock`;
//!   legacy `"hosted-servers"` redirects to `"billing"`; unknown tab IDs return an explicit error).
//! - Exact 18 Server Settings sections across 4 groups (`Top`, `Agents`, `People`, `Server`).
//! - Exact 11 configurable `ShortcutCommand`s + 4 immutable `FixedShortcutGroup`s with macOS/Linux/Windows
//!   defaults, OS-reserved & bare-key guards, fixed-override refusal, and duplicate-collision rejection.

use std::collections::{BTreeMap, HashMap};

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::providers::AGENT_PROVIDER_DESCRIPTORS;
use serde::{Deserialize, Serialize};

use crate::theme::openbot_palette::OpenBotPalette;
use crate::theme::{IconName, tg_icon_rgba};

fn pal_rgba(palette: &OpenBotPalette, key: &str, fallback_hex: u32) -> Rgba {
    palette.rgba(key).unwrap_or_else(|| rgb(fallback_hex))
}

// ============================================================================
// 1. 14-Tab App Settings (`SettingsModal.tsx` / `settings-tabs.ts`)
// ============================================================================

/// Legacy app settings tab alias redirected to `"billing"`.
pub const LEGACY_HOSTED_SERVERS_TAB: &str = "hosted-servers";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppSettingsGroup {
    Primary,
    Account,
    ThisComputer,
}

impl AppSettingsGroup {
    pub const ALL: [Self; 3] = [Self::Primary, Self::Account, Self::ThisComputer];

    pub const fn heading(self) -> &'static str {
        match self {
            Self::Primary => "APP",
            Self::Account => "ACCOUNT",
            Self::ThisComputer => "THIS COMPUTER",
        }
    }
}

/// All 14 canonical visible App Settings tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppSettingsTab {
    #[default]
    General,
    Appearance,
    Notifications,
    Voice,
    Keyboard,
    Profile,
    Billing,
    Referral,
    MobileConnect,
    AppBehavior,
    DynamicIsland,
    AgentDock,
    ComputerUse,
    Updates,
}

impl AppSettingsTab {
    pub const ALL: [Self; 14] = [
        Self::General,
        Self::Appearance,
        Self::Notifications,
        Self::Voice,
        Self::Keyboard,
        Self::Profile,
        Self::Billing,
        Self::Referral,
        Self::MobileConnect,
        Self::AppBehavior,
        Self::DynamicIsland,
        Self::AgentDock,
        Self::ComputerUse,
        Self::Updates,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Notifications => "notifications",
            Self::Voice => "voice",
            Self::Keyboard => "keyboard",
            Self::Profile => "profile",
            Self::Billing => "billing",
            Self::Referral => "referral",
            Self::MobileConnect => "mobile-connect",
            Self::AppBehavior => "app-behavior",
            Self::DynamicIsland => "dynamic-island",
            Self::AgentDock => "agent-dock",
            Self::ComputerUse => "computer-use",
            Self::Updates => "updates",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Notifications => "Notifications",
            Self::Voice => "Voice",
            Self::Keyboard => "Keyboard",
            Self::Profile => "Profile",
            Self::Billing => "Billing",
            Self::Referral => "Referral",
            Self::MobileConnect => "Mobile Connect",
            Self::AppBehavior => "App Behavior",
            Self::DynamicIsland => "Dynamic Island",
            Self::AgentDock => "Agent Dock",
            Self::ComputerUse => "Computer Use",
            Self::Updates => "Updates",
        }
    }

    pub const fn group(self) -> AppSettingsGroup {
        match self {
            Self::General
            | Self::Appearance
            | Self::Notifications
            | Self::Voice
            | Self::Keyboard => AppSettingsGroup::Primary,
            Self::Profile | Self::Billing | Self::Referral | Self::MobileConnect => {
                AppSettingsGroup::Account
            }
            Self::AppBehavior
            | Self::DynamicIsland
            | Self::AgentDock
            | Self::ComputerUse
            | Self::Updates => AppSettingsGroup::ThisComputer,
        }
    }

    pub const fn hidden_in_chat_window(self) -> bool {
        matches!(self, Self::DynamicIsland | Self::AgentDock)
    }

    pub const fn icon(self) -> IconName {
        match self {
            Self::General => IconName::SettingsGear,
            Self::Appearance => IconName::NightMoon,
            Self::Notifications => IconName::ChannelMegaphone,
            Self::Voice => IconName::Mic,
            Self::Keyboard => IconName::Edit,
            Self::Profile => IconName::ContactsUsers,
            Self::Billing => IconName::WalletStars,
            Self::Referral => IconName::PremiumStar,
            Self::MobileConnect => IconName::PhoneCall,
            Self::AppBehavior => IconName::ComputerDesktop,
            Self::DynamicIsland => IconName::Pin,
            Self::AgentDock => IconName::BotRobot,
            Self::ComputerUse => IconName::ScreenShare,
            Self::Updates => IconName::ArrowUp,
        }
    }

    pub fn from_id(raw: &str) -> Result<Self, String> {
        let canonical = resolve_app_settings_tab(raw)?;
        Self::ALL
            .iter()
            .copied()
            .find(|t| t.id() == canonical)
            .ok_or_else(|| format!("unknown app settings tab '{raw}'"))
    }
}

/// Returns all 14 visible App Settings tab IDs on main window (`chat_window == false`),
/// or 12 tab IDs when `chat_window == true` (omitting `"dynamic-island"` and `"agent-dock"`).
pub fn visible_app_settings_tabs(chat_window: bool) -> Vec<&'static str> {
    AppSettingsTab::ALL
        .iter()
        .copied()
        .filter(|tab| !chat_window || !tab.hidden_in_chat_window())
        .map(AppSettingsTab::id)
        .collect()
}

/// Resolves an App Settings tab identifier, redirecting legacy `"hosted-servers"` -> `"billing"`
/// and rejecting any unknown tab ID.
pub fn resolve_app_settings_tab(raw: &str) -> Result<&'static str, String> {
    let trimmed = raw.trim();
    if trimmed == LEGACY_HOSTED_SERVERS_TAB {
        return Ok(AppSettingsTab::Billing.id());
    }
    AppSettingsTab::ALL
        .iter()
        .map(|tab| tab.id())
        .find(|&id| id == trimmed)
        .ok_or_else(|| {
            format!("unknown app settings tab '{trimmed}': unrecognized settings destination")
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsState {
    pub active_tab: AppSettingsTab,
    pub chat_window: bool,
    pub send_on_enter: bool,
    pub show_agent_steps: bool,
    pub compact_actions: bool,
    pub theme_mode: String,
    pub backdrop_scene: String,
    pub moving_faces: bool,
    pub reduced_motion: bool,
    pub desktop_notifications: bool,
    pub task_sound: bool,
    pub dictation_model: String,
    pub tts_speed: f32,
    pub profile_email: String,
    pub billing_plan: String,
    pub referral_code: String,
    pub mobile_paired_devices: usize,
    pub launch_at_login: bool,
    pub quit_guard_enabled: bool,
    pub keep_awake: bool,
    pub dynamic_island_enabled: bool,
    pub dynamic_island_width_pct: u32,
    pub dynamic_island_height_pct: u32,
    pub agent_dock_enabled: bool,
    pub agent_dock_side: String,
    pub computer_use_granted: bool,
    pub update_track: String,
}

impl Default for AppSettingsState {
    fn default() -> Self {
        Self {
            active_tab: AppSettingsTab::General,
            chat_window: false,
            send_on_enter: true,
            show_agent_steps: true,
            compact_actions: false,
            theme_mode: "dark".to_string(),
            backdrop_scene: "pond".to_string(),
            moving_faces: true,
            reduced_motion: false,
            desktop_notifications: true,
            task_sound: true,
            dictation_model: "parakeet-tdt-0.6b-v3".to_string(),
            tts_speed: 1.0,
            profile_email: "operator@openbot.local".to_string(),
            billing_plan: "Pro".to_string(),
            referral_code: "OPENBOT-2026".to_string(),
            mobile_paired_devices: 1,
            launch_at_login: true,
            quit_guard_enabled: true,
            keep_awake: false,
            dynamic_island_enabled: true,
            dynamic_island_width_pct: 100,
            dynamic_island_height_pct: 100,
            agent_dock_enabled: true,
            agent_dock_side: "right".to_string(),
            computer_use_granted: false,
            update_track: "stable".to_string(),
        }
    }
}

impl AppSettingsState {
    pub fn select_tab(&mut self, raw: &str) -> Result<AppSettingsTab, String> {
        let tab = AppSettingsTab::from_id(raw)?;
        if self.chat_window && tab.hidden_in_chat_window() {
            return Err(format!(
                "app settings tab '{}' is hidden in detached chat window",
                tab.id()
            ));
        }
        self.active_tab = tab;
        Ok(tab)
    }

    pub fn visible_tabs(&self) -> Vec<&'static str> {
        visible_app_settings_tabs(self.chat_window)
    }
}

// ============================================================================
// 2. 18-Section Server Settings (`ServerSettingsModal.tsx`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerSettingsGroup {
    Top,
    Agents,
    People,
    Server,
}

impl ServerSettingsGroup {
    pub const ALL: [Self; 4] = [Self::Top, Self::Agents, Self::People, Self::Server];

    pub const fn heading(self) -> &'static str {
        match self {
            Self::Top => "WORKSPACE",
            Self::Agents => "AGENTS",
            Self::People => "PEOPLE",
            Self::Server => "SERVER",
        }
    }
}

/// All 18 canonical Server Settings sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerSettingsSection {
    #[default]
    General,
    Providers,
    Personality,
    Agents,
    Memory,
    Vault,
    BrowserProfiles,
    Routines,
    Sites,
    Members,
    Roles,
    Audit,
    Storage,
    Updates,
    Desktop,
    Import,
    Transfer,
    Experimental,
}

impl ServerSettingsSection {
    pub const ALL: [Self; 18] = [
        Self::General,
        Self::Providers,
        Self::Personality,
        Self::Agents,
        Self::Memory,
        Self::Vault,
        Self::BrowserProfiles,
        Self::Routines,
        Self::Sites,
        Self::Members,
        Self::Roles,
        Self::Audit,
        Self::Storage,
        Self::Updates,
        Self::Desktop,
        Self::Import,
        Self::Transfer,
        Self::Experimental,
    ];

    pub const ALL_IDS: [&'static str; 18] = [
        "general",
        "providers",
        "personality",
        "agents",
        "memory",
        "vault",
        "browser-profiles",
        "routines",
        "sites",
        "members",
        "roles",
        "audit",
        "storage",
        "updates",
        "desktop",
        "import",
        "transfer",
        "experimental",
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Providers => "providers",
            Self::Personality => "personality",
            Self::Agents => "agents",
            Self::Memory => "memory",
            Self::Vault => "vault",
            Self::BrowserProfiles => "browser-profiles",
            Self::Routines => "routines",
            Self::Sites => "sites",
            Self::Members => "members",
            Self::Roles => "roles",
            Self::Audit => "audit",
            Self::Storage => "storage",
            Self::Updates => "updates",
            Self::Desktop => "desktop",
            Self::Import => "import",
            Self::Transfer => "transfer",
            Self::Experimental => "experimental",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Providers => "Providers",
            Self::Personality => "Personality",
            Self::Agents => "Agents",
            Self::Memory => "Memory",
            Self::Vault => "Vault",
            Self::BrowserProfiles => "Browser Profiles",
            Self::Routines => "Routines",
            Self::Sites => "Sites",
            Self::Members => "Members",
            Self::Roles => "Roles",
            Self::Audit => "Audit Log",
            Self::Storage => "Storage",
            Self::Updates => "Updates",
            Self::Desktop => "Remote Desktop",
            Self::Import => "Import",
            Self::Transfer => "Transfer",
            Self::Experimental => "Experimental",
        }
    }

    pub const fn group(self) -> ServerSettingsGroup {
        match self {
            Self::General | Self::Providers | Self::Personality => ServerSettingsGroup::Top,
            Self::Agents
            | Self::Memory
            | Self::Vault
            | Self::BrowserProfiles
            | Self::Routines
            | Self::Sites => ServerSettingsGroup::Agents,
            Self::Members | Self::Roles | Self::Audit => ServerSettingsGroup::People,
            Self::Storage
            | Self::Updates
            | Self::Desktop
            | Self::Import
            | Self::Transfer
            | Self::Experimental => ServerSettingsGroup::Server,
        }
    }

    pub fn from_id(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|s| s.id() == trimmed)
            .ok_or_else(|| format!("unknown server settings section '{trimmed}'"))
    }
}

/// Returns all 18 canonical Server Settings section IDs.
pub fn server_settings_section_ids() -> Vec<&'static str> {
    ServerSettingsSection::ALL_IDS.to_vec()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettingsState {
    pub active_section: ServerSettingsSection,
    pub server_name: String,
    pub host_key_fingerprint: String,
    pub discover_ollama: bool,
    pub discover_lm_studio: bool,
    pub personality_note: String,
    pub timezone: String,
    pub default_task_limit: u32,
    pub memory_limit: usize,
    pub vault_external_sync: String,
    pub browser_profile_count: usize,
    pub routines_enabled: bool,
    pub static_sites_count: usize,
    pub members_count: usize,
    pub roles_count: usize,
    pub audit_retention_days: u32,
    pub storage_quota_gib: u32,
    pub update_channel: String,
    pub remote_desktop_enabled: bool,
    pub crm_experimental_enabled: bool,
}

impl Default for ServerSettingsState {
    fn default() -> Self {
        Self {
            active_section: ServerSettingsSection::General,
            server_name: "Local OpenBot Server".to_string(),
            host_key_fingerprint: "ed25519:openbot-local".to_string(),
            discover_ollama: true,
            discover_lm_studio: true,
            personality_note: "Concise, reliable engineering assistant.".to_string(),
            timezone: "UTC".to_string(),
            default_task_limit: 4,
            memory_limit: 64,
            vault_external_sync: "Built-in encrypted vault".to_string(),
            browser_profile_count: 1,
            routines_enabled: true,
            static_sites_count: 0,
            members_count: 1,
            roles_count: 3,
            audit_retention_days: 90,
            storage_quota_gib: 20,
            update_channel: "stable".to_string(),
            remote_desktop_enabled: false,
            crm_experimental_enabled: true,
        }
    }
}

// ============================================================================
// 3. Keyboard Shortcuts Registry (`keyboard-shortcuts.ts`)
// ============================================================================

/// Exact 11 configurable `ShortcutCommand`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShortcutCommand {
    NewThread,
    PreviousThread,
    NextThread,
    NewAgent,
    GlobalSearch,
    SearchChat,
    FocusComposer,
    Back,
    Forward,
    ToggleSidebar,
    ShowShortcuts,
}

impl ShortcutCommand {
    pub const ALL: [Self; 11] = [
        Self::NewThread,
        Self::PreviousThread,
        Self::NextThread,
        Self::NewAgent,
        Self::GlobalSearch,
        Self::SearchChat,
        Self::FocusComposer,
        Self::Back,
        Self::Forward,
        Self::ToggleSidebar,
        Self::ShowShortcuts,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::NewThread => "newThread",
            Self::PreviousThread => "previousThread",
            Self::NextThread => "nextThread",
            Self::NewAgent => "newAgent",
            Self::GlobalSearch => "globalSearch",
            Self::SearchChat => "searchChat",
            Self::FocusComposer => "focusComposer",
            Self::Back => "back",
            Self::Forward => "forward",
            Self::ToggleSidebar => "toggleSidebar",
            Self::ShowShortcuts => "showShortcuts",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::NewThread => "New Thread",
            Self::PreviousThread => "Previous Thread",
            Self::NextThread => "Next Thread",
            Self::NewAgent => "New Agent",
            Self::GlobalSearch => "Global Search",
            Self::SearchChat => "Search Chat",
            Self::FocusComposer => "Focus Composer",
            Self::Back => "Navigate Back",
            Self::Forward => "Navigate Forward",
            Self::ToggleSidebar => "Toggle Sidebar",
            Self::ShowShortcuts => "Show Shortcuts",
        }
    }

    pub fn from_id(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.id() == raw.trim())
    }
}

/// Exact 4 immutable `FixedShortcutGroup`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FixedShortcutGroup {
    OpenSettings,
    StopAllAgents,
    SwitchServer,
    Zoom,
}

impl FixedShortcutGroup {
    pub const ALL: [Self; 4] = [
        Self::OpenSettings,
        Self::StopAllAgents,
        Self::SwitchServer,
        Self::Zoom,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::OpenSettings => "openSettings",
            Self::StopAllAgents => "stopAllAgents",
            Self::SwitchServer => "switchServer",
            Self::Zoom => "zoom",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::OpenSettings => "Open Settings",
            Self::StopAllAgents => "Stop All Agents",
            Self::SwitchServer => "Switch Server (1–9)",
            Self::Zoom => "Zoom In / Out / Reset",
        }
    }

    pub fn display_chord(self, platform: &str) -> String {
        let primary = if is_darwin(platform) { "⌘" } else { "Ctrl+" };
        match self {
            Self::OpenSettings => format!("{primary},"),
            Self::StopAllAgents => format!("{primary}."),
            Self::SwitchServer => format!("{primary}1–9"),
            Self::Zoom => format!("{primary}+ / − / 0"),
        }
    }

    pub fn from_id(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|g| g.id() == raw.trim())
    }
}

fn is_darwin(platform: &str) -> bool {
    matches!(
        platform.trim().to_ascii_lowercase().as_str(),
        "darwin" | "macos" | "mac"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutChord {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub meta: bool,
    pub code: String,
}

impl ShortcutChord {
    pub fn new(ctrl: bool, alt: bool, shift: bool, meta: bool, code: impl Into<String>) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            meta,
            code: code.into(),
        }
    }

    pub fn normalized_key(&self) -> (bool, bool, bool, bool, String) {
        (
            self.ctrl,
            self.alt,
            self.shift,
            self.meta,
            self.code.trim().to_ascii_lowercase(),
        )
    }

    /// Evaluates whether a keydown event matches this chord (always ignores IME composing events).
    pub fn matches(
        &self,
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
        code: &str,
        is_composing: bool,
    ) -> bool {
        if is_composing {
            return false;
        }
        self.ctrl == ctrl
            && self.alt == alt
            && self.shift == shift
            && self.meta == meta
            && self.code.eq_ignore_ascii_case(code.trim())
    }

    pub fn display_tokens(&self, platform: &str) -> Vec<String> {
        let mac = is_darwin(platform);
        let mut out = Vec::new();
        if self.ctrl {
            out.push(if mac { "⌃".to_string() } else { "Ctrl".to_string() });
        }
        if self.alt {
            out.push(if mac { "⌥".to_string() } else { "Alt".to_string() });
        }
        if self.shift {
            out.push(if mac { "⇧".to_string() } else { "Shift".to_string() });
        }
        if self.meta {
            out.push(if mac { "⌘".to_string() } else { "Meta".to_string() });
        }
        out.push(humanize_key_code(&self.code));
        out
    }
}

fn humanize_key_code(code: &str) -> String {
    if let Some(letter) = code.strip_prefix("Key") {
        return letter.to_ascii_uppercase();
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return digit.to_string();
    }
    match code {
        "ArrowUp" => "↑".to_string(),
        "ArrowDown" => "↓".to_string(),
        "ArrowLeft" => "←".to_string(),
        "ArrowRight" => "→".to_string(),
        "BracketLeft" => "[".to_string(),
        "BracketRight" => "]".to_string(),
        "Slash" => "/".to_string(),
        "Escape" => "Esc".to_string(),
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutRegistry {
    pub platform: String,
    pub overrides: BTreeMap<String, ShortcutChord>,
}

impl Default for ShortcutRegistry {
    fn default() -> Self {
        Self::new("linux")
    }
}

impl ShortcutRegistry {
    pub fn new(platform: impl Into<String>) -> Self {
        Self {
            platform: platform.into(),
            overrides: BTreeMap::new(),
        }
    }

    /// Returns the 11 configurable platform defaults (`"darwin"` uses `meta: true, ctrl: false`;
    /// `"linux"` / `"win32"` use `ctrl: true, meta: false`).
    pub fn platform_defaults(platform: &str) -> BTreeMap<String, ShortcutChord> {
        let mac = is_darwin(platform);
        let primary = |shift: bool, code: &str| ShortcutChord::new(!mac, false, shift, mac, code);
        let mut map = BTreeMap::new();
        map.insert(ShortcutCommand::NewThread.id().to_string(), primary(true, "KeyO"));
        map.insert(
            ShortcutCommand::PreviousThread.id().to_string(),
            ShortcutChord::new(false, true, true, false, "ArrowUp"),
        );
        map.insert(
            ShortcutCommand::NextThread.id().to_string(),
            ShortcutChord::new(false, true, true, false, "ArrowDown"),
        );
        map.insert(ShortcutCommand::NewAgent.id().to_string(), primary(true, "KeyE"));
        map.insert(ShortcutCommand::GlobalSearch.id().to_string(), primary(false, "KeyK"));
        map.insert(ShortcutCommand::SearchChat.id().to_string(), primary(false, "KeyF"));
        map.insert(
            ShortcutCommand::FocusComposer.id().to_string(),
            ShortcutChord::new(false, false, true, false, "Escape"),
        );
        if mac {
            map.insert(ShortcutCommand::Back.id().to_string(), primary(false, "BracketLeft"));
            map.insert(ShortcutCommand::Forward.id().to_string(), primary(false, "BracketRight"));
        } else {
            map.insert(
                ShortcutCommand::Back.id().to_string(),
                ShortcutChord::new(false, true, false, false, "ArrowLeft"),
            );
            map.insert(
                ShortcutCommand::Forward.id().to_string(),
                ShortcutChord::new(false, true, false, false, "ArrowRight"),
            );
        }
        map.insert(ShortcutCommand::ToggleSidebar.id().to_string(), primary(false, "KeyB"));
        map.insert(ShortcutCommand::ShowShortcuts.id().to_string(), primary(false, "Slash"));
        map
    }

    /// Validates a single `ShortcutChord` against bare/invalid keys and OS-reserved primary chords.
    pub fn validate_chord(platform: &str, chord: &ShortcutChord) -> Result<(), String> {
        let code = chord.code.trim();
        if code.is_empty() {
            return Err("invalid shortcut chord: key code must not be empty".to_string());
        }
        let has_modifier = chord.ctrl || chord.alt || chord.shift || chord.meta;
        if !has_modifier {
            return Err(format!(
                "invalid bare shortcut chord '{code}': at least one modifier key is required"
            ));
        }
        if matches!(code, "Space" | "Enter" | "Tab") && !chord.ctrl && !chord.alt && !chord.meta {
            return Err(format!(
                "invalid shortcut chord '{code}': Space/Enter/Tab cannot be bound without Ctrl/Alt/Cmd"
            ));
        }
        let mac = is_darwin(platform);
        let primary_only = if mac {
            chord.meta && !chord.ctrl && !chord.alt && !chord.shift
        } else {
            chord.ctrl && !chord.meta && !chord.alt && !chord.shift
        };
        let primary_shift_only = if mac {
            chord.meta && !chord.ctrl && !chord.alt && chord.shift
        } else {
            chord.ctrl && !chord.meta && !chord.alt && chord.shift
        };
        const RESERVED_PRIMARY: [&str; 27] = [
            "KeyA", "KeyC", "KeyG", "KeyH", "KeyL", "KeyM", "KeyQ", "KeyR", "KeyT", "KeyV", "KeyW",
            "KeyX", "KeyZ", "Comma", "Period", "Equal", "Minus", "Digit0", "Digit1", "Digit2",
            "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9",
        ];
        if primary_only && RESERVED_PRIMARY.iter().any(|r| r.eq_ignore_ascii_case(code)) {
            return Err(format!(
                "reserved OS/built-in shortcut chord '{code}' cannot be bound"
            ));
        }
        if primary_shift_only && (code.eq_ignore_ascii_case("KeyZ") || code.eq_ignore_ascii_case("KeyG")) {
            return Err(format!(
                "reserved OS shortcut chord 'Shift+{code}' cannot be bound"
            ));
        }
        Ok(())
    }

    /// Resolves platform defaults with validated `overrides`, rejecting fixed-shortcut overrides,
    /// unknown command IDs, invalid/reserved chords, and duplicate chord collisions.
    pub fn apply_overrides(
        platform: &str,
        overrides: &BTreeMap<String, ShortcutChord>,
    ) -> Result<BTreeMap<String, ShortcutChord>, String> {
        let mut resolved = Self::platform_defaults(platform);
        for (cmd_id, chord) in overrides {
            if FixedShortcutGroup::from_id(cmd_id).is_some() {
                return Err(format!(
                    "cannot override fixed built-in shortcut '{cmd_id}': fixed shortcut groups are immutable"
                ));
            }
            if ShortcutCommand::from_id(cmd_id).is_none() {
                return Err(format!(
                    "unknown shortcut command '{cmd_id}': expected one of the 11 configurable commands"
                ));
            }
            Self::validate_chord(platform, chord)?;
            resolved.insert(cmd_id.clone(), chord.clone());
        }
        let mut seen: HashMap<(bool, bool, bool, bool, String), String> = HashMap::new();
        for (cmd_id, chord) in &resolved {
            let key = chord.normalized_key();
            if let Some(existing) = seen.insert(key, cmd_id.clone()) {
                return Err(format!(
                    "duplicate shortcut chord collision (conflict): '{existing}' and '{cmd_id}' are already bound to the same chord"
                ));
            }
        }
        Ok(resolved)
    }

    pub fn resolve(&self) -> Result<BTreeMap<String, ShortcutChord>, String> {
        Self::apply_overrides(&self.platform, &self.overrides)
    }

    pub fn probe_summary(
        platform: &str,
        overrides: &BTreeMap<String, ShortcutChord>,
    ) -> Result<serde_json::Value, String> {
        let resolved = Self::apply_overrides(platform, overrides)?;
        let configurable_ids: Vec<&'static str> =
            ShortcutCommand::ALL.iter().map(|c| c.id()).collect();
        let fixed_ids: Vec<&'static str> =
            FixedShortcutGroup::ALL.iter().map(|g| g.id()).collect();
        Ok(serde_json::json!({
            "configurable_count": ShortcutCommand::ALL.len(),
            "fixed_count": FixedShortcutGroup::ALL.len(),
            "platform": platform,
            "configurable_commands": configurable_ids,
            "fixed_groups": fixed_ids,
            "resolved": resolved,
        }))
    }
}

fn render_setting_row(
    label: &str,
    detail: &str,
    value_badge: String,
    palette: &OpenBotPalette,
) -> Div {
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    div()
        .p(px(10.0))
        .rounded(px(12.0))
        .bg(bg_control)
        .border_1()
        .border_color(border)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(text_primary).child(label.to_string()))
                .child(div().text_size(px(11.0)).text_color(text_muted).child(detail.to_string())),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(border)
                .text_size(px(11.0))
                .text_color(text_primary)
                .child(value_badge),
        )
}

// ============================================================================
// 4. GPUI 0.2.2 Renderers (`AppSettingsModal`, `ServerSettingsModal`, `ShortcutsOverlay`)
// ============================================================================

/// Renders the 14-tab App Settings Modal (`12` tabs when `state.chat_window == true`).
pub fn render_app_settings_modal(state: &AppSettingsState, palette: &OpenBotPalette) -> Div {
    let bg_sheet = pal_rgba(palette, "--openbot-bg-sheet", 0x141414);
    let bg_sidebar = pal_rgba(palette, "--openbot-bg-sidebar", 0x1a1a1a);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);

    let mut sidebar = div()
        .w(px(220.0))
        .p(px(12.0))
        .border_r_1()
        .border_color(border)
        .bg(bg_sidebar)
        .flex()
        .flex_col()
        .gap(px(10.0));

    for group in AppSettingsGroup::ALL {
        let tabs: Vec<AppSettingsTab> = AppSettingsTab::ALL
            .iter()
            .copied()
            .filter(|t| t.group() == group && (!state.chat_window || !t.hidden_in_chat_window()))
            .collect();
        let mut group_col = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .px(px(8.0))
                .py(px(3.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_muted)
                .child(group.heading()),
        );
        for tab in tabs {
            let active = tab == state.active_tab;
            group_col = group_col.child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(if active { accent } else { bg_sidebar })
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(tg_icon_rgba(
                        tab.icon(),
                        14.0,
                        if active { rgb(0xffffff) } else { text_muted },
                    ))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(if active { rgb(0xffffff) } else { text_primary })
                            .child(tab.label()),
                    ),
            );
        }
        sidebar = sidebar.child(group_col);
    }

    let mut content = div()
        .flex_1()
        .p(px(18.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(state.active_tab.label()),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(999.0))
                        .bg(bg_control)
                        .text_size(px(10.0))
                        .text_color(text_muted)
                        .child(format!("{} visible tabs", state.visible_tabs().len())),
                ),
        );

    content = match state.active_tab {
        AppSettingsTab::General => content
            .child(render_setting_row("Send Shortcut", "Send message with Enter or Cmd/Ctrl+Enter", if state.send_on_enter { "Enter".into() } else { "Cmd/Ctrl+Enter".into() }, palette))
            .child(render_setting_row("Show Agent Steps", "Display collapsible reasoning & tool progress", if state.show_agent_steps { "On".into() } else { "Off".into() }, palette))
            .child(render_setting_row("Compact Actions", "Compact 1-hour timeline gap actions", if state.compact_actions { "On".into() } else { "Off".into() }, palette)),
        AppSettingsTab::Appearance => content
            .child(render_setting_row("Theme Palette", "1200ms spectral sweep theme switch", state.theme_mode.clone(), palette))
            .child(render_setting_row("Chat Backdrop", "Procedural pixel-art scene or custom image", state.backdrop_scene.clone(), palette))
            .child(render_setting_row("Moving Faces", "Ambient Bloub/Plush breathing & gaze tracking", if state.moving_faces && !state.reduced_motion { "Active".into() } else { "Reduced".into() }, palette)),
        AppSettingsTab::Notifications => content
            .child(render_setting_row("Desktop Notifications", "Show OS banners for agent replies & approvals", if state.desktop_notifications { "Enabled".into() } else { "Muted".into() }, palette))
            .child(render_setting_row("Task Completion Sound", "Audible cue on background task completion", if state.task_sound { "On".into() } else { "Off".into() }, palette)),
        AppSettingsTab::Voice => content
            .child(render_setting_row("Dictation Model", "Local Parakeet / Whisper / Cloud STT model", state.dictation_model.clone(), palette))
            .child(render_setting_row("Speech Playback Speed", "Cyclic TTS speed (0.75x – 2.0x)", format!("{:.2}x", state.tts_speed), palette)),
        AppSettingsTab::Keyboard => content
            .child(render_setting_row("Configurable Commands", "11 rebindable commands with collision guard", "11 commands".into(), palette))
            .child(render_setting_row("Fixed Built-In Shortcuts", "openSettings, stopAllAgents, switchServer, zoom", "4 immutable".into(), palette)),
        AppSettingsTab::Profile => content
            .child(render_setting_row("Operator Account", "Signed-in profile identity & sessions", state.profile_email.clone(), palette)),
        AppSettingsTab::Billing => content
            .child(render_setting_row("Active Plan", "Subscription, credits & hosted servers (legacy alias)", state.billing_plan.clone(), palette)),
        AppSettingsTab::Referral => content
            .child(render_setting_row("Referral Code", "Share invite code to earn compute credits", state.referral_code.clone(), palette)),
        AppSettingsTab::MobileConnect => content
            .child(render_setting_row("Paired Mobile Devices", "QR code pairing & encrypted remote relay", format!("{} device(s)", state.mobile_paired_devices), palette)),
        AppSettingsTab::AppBehavior => content
            .child(render_setting_row("Launch at Login", "Start OpenBot workspace automatically", if state.launch_at_login { "On".into() } else { "Off".into() }, palette))
            .child(render_setting_row("Quit Guard", "Prompt before closing while agents are working", if state.quit_guard_enabled { "Protected".into() } else { "Off".into() }, palette))
            .child(render_setting_row("Keep Awake", "Prevent system sleep during active agent runs", if state.keep_awake { "On".into() } else { "Off".into() }, palette)),
        AppSettingsTab::DynamicIsland => content
            .child(render_setting_row("Dynamic Island Overlay", "192x32 notch status & critical prompt lock", if state.dynamic_island_enabled { "Shown".into() } else { "Hidden".into() }, palette))
            .child(render_setting_row("Dimensions", "Width (20–130% step 5) × Height (75–125% step 5)", format!("{}% × {}%", state.dynamic_island_width_pct, state.dynamic_island_height_pct), palette)),
        AppSettingsTab::AgentDock => content
            .child(render_setting_row("Screen-Edge Agent Dock", "Up to 24 seats with 900ms edge-strip auto-tuck", if state.agent_dock_enabled { state.agent_dock_side.clone() } else { "Hidden".into() }, palette)),
        AppSettingsTab::ComputerUse => content
            .child(render_setting_row("OS Accessibility & Screen Recording", "Required for 33ms CUA window highlight & virtual cursor", if state.computer_use_granted { "Granted".into() } else { "Needs Permission".into() }, palette)),
        AppSettingsTab::Updates => content
            .child(render_setting_row("Release Track", "Auto-download and idle restart channel", state.update_track.clone(), palette)),
    };

    div()
        .w(px(760.0))
        .min_h(px(520.0))
        .rounded(px(20.0))
        .bg(bg_sheet)
        .border_1()
        .border_color(pal_rgba(palette, "--openbot-dialog-ring", 0x333335))
        .overflow_hidden()
        .flex()
        .child(sidebar)
        .child(content)
}

/// Renders the 18-section Server Settings Modal across `Top`, `Agents`, `People`, and `Server` groups.
pub fn render_server_settings_modal(state: &ServerSettingsState, palette: &OpenBotPalette) -> Div {
    let bg_sheet = pal_rgba(palette, "--openbot-bg-sheet", 0x141414);
    let bg_sidebar = pal_rgba(palette, "--openbot-bg-sidebar", 0x1a1a1a);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);

    let mut sidebar = div()
        .w(px(220.0))
        .p(px(12.0))
        .border_r_1()
        .border_color(border)
        .bg(bg_sidebar)
        .flex()
        .flex_col()
        .gap(px(8.0));

    for group in ServerSettingsGroup::ALL {
        let mut group_col = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .px(px(8.0))
                .py(px(2.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_muted)
                .child(group.heading()),
        );
        for sec in ServerSettingsSection::ALL.iter().copied().filter(|s| s.group() == group) {
            let active = sec == state.active_section;
            group_col = group_col.child(
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(8.0))
                    .bg(if active { accent } else { bg_sidebar })
                    .text_size(px(12.0))
                    .text_color(if active { rgb(0xffffff) } else { text_primary })
                    .child(sec.label()),
            );
        }
        sidebar = sidebar.child(group_col);
    }

    let mut content = div()
        .flex_1()
        .p(px(18.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child(state.active_section.label()))
                .child(div().px(px(8.0)).py(px(3.0)).rounded(px(999.0)).bg(bg_control).text_size(px(10.0)).text_color(text_muted).child("18 sections")),
        );

    content = match state.active_section {
        ServerSettingsSection::General => content
            .child(render_setting_row("Server Name", "Display label shown in ServerRail & invites", state.server_name.clone(), palette))
            .child(render_setting_row("Host Key", "Pinned cryptographic server identity", state.host_key_fingerprint.clone(), palette)),
        ServerSettingsSection::Providers => content
            .child(render_setting_row("Built-In Providers", "Codex, Claude, Grok, OpenCode, Antigravity & 13 more", format!("{} providers", AGENT_PROVIDER_DESCRIPTORS.len()), palette))
            .child(render_setting_row("Local Discovery", "Ollama (:11434) & LM Studio (:1234) detection", format!("Ollama:{} · LM:{}", state.discover_ollama, state.discover_lm_studio), palette)),
        ServerSettingsSection::Personality => content
            .child(render_setting_row("Workspace Personality", "Shared system tone for workspace agents", state.personality_note.clone(), palette)),
        ServerSettingsSection::Agents => content
            .child(render_setting_row("Server Timezone", "Default IANA timezone for agent routines", state.timezone.clone(), palette))
            .child(render_setting_row("Concurrent Task Limit", "Maximum parallel side tasks (1..8)", format!("{}", state.default_task_limit), palette)),
        ServerSettingsSection::Memory => content
            .child(render_setting_row("Per-Agent Memory Cap", "4,096 B essential budget + FTS5/CJK search", format!("{} memories", state.memory_limit), palette)),
        ServerSettingsSection::Vault => content
            .child(render_setting_row("Credential Vault", "Encrypted logins, cards, contacts & external sync", state.vault_external_sync.clone(), palette)),
        ServerSettingsSection::BrowserProfiles => content
            .child(render_setting_row("Browser Profiles", "Isolated Chromium profiles & proxy bindings", format!("{} profile(s)", state.browser_profile_count), palette)),
        ServerSettingsSection::Routines => content
            .child(render_setting_row("Routine Scheduler", "Minimum 3m (180s) interval & HMAC webhooks", if state.routines_enabled { "Active".into() } else { "Paused".into() }, palette)),
        ServerSettingsSection::Sites => content
            .child(render_setting_row("Hosted Static Sites", "30-day TTL static bundles (<= 20 files, 2 MiB)", format!("{} active site(s)", state.static_sites_count), palette)),
        ServerSettingsSection::Members => content
            .child(render_setting_row("Team Members", "SSO / SCIM provisioning and workspace ownership", format!("{} member(s)", state.members_count), palette)),
        ServerSettingsSection::Roles => content
            .child(render_setting_row("Access Roles", "Fine-grained agent & channel ACL policies", format!("{} role(s)", state.roles_count), palette)),
        ServerSettingsSection::Audit => content
            .child(render_setting_row("Audit Log Retention", "Immutable administrative & CRM activity log", format!("{} days", state.audit_retention_days), palette)),
        ServerSettingsSection::Storage => content
            .child(render_setting_row("Workspace Quota", "Attachments, media cache, and background processes", format!("{} GiB", state.storage_quota_gib), palette)),
        ServerSettingsSection::Updates => content
            .child(render_setting_row("Server Release Channel", "Automatic server daemon update channel", state.update_channel.clone(), palette)),
        ServerSettingsSection::Desktop => content
            .child(render_setting_row("Remote Desktop", "Interactive desktop bridge & screen stream", if state.remote_desktop_enabled { "Enabled".into() } else { "Disabled".into() }, palette)),
        ServerSettingsSection::Import => content
            .child(render_setting_row("Archive Import", "Supports .openbot bundles, Grok .zip, and Hermes", "Ready".into(), palette)),
        ServerSettingsSection::Transfer => content
            .child(render_setting_row("Instance Transfer", "Migrate SQLite database & agent workspaces cleanly", "Available".into(), palette)),
        ServerSettingsSection::Experimental => content
            .child(render_setting_row("Built-In CRM Module", "13 SQLite tables + Stripe sync + crm_propose queue", if state.crm_experimental_enabled { "Enabled".into() } else { "Disabled".into() }, palette)),
    };

    div()
        .w(px(780.0))
        .min_h(px(560.0))
        .rounded(px(20.0))
        .bg(bg_sheet)
        .border_1()
        .border_color(pal_rgba(palette, "--openbot-dialog-ring", 0x333335))
        .overflow_hidden()
        .flex()
        .child(sidebar)
        .child(content)
}

/// Renders the `ShortcutsOverlay` showing all 11 configurable commands, 4 immutable fixed groups,
/// and contextual composer/queue shortcuts for `platform`.
pub fn render_shortcuts_overlay(
    registry: &ShortcutRegistry,
    platform: &str,
    palette: &OpenBotPalette,
) -> Div {
    let bg_sheet = pal_rgba(palette, "--openbot-bg-sheet", 0x141414);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);

    let resolved = ShortcutRegistry::apply_overrides(platform, &registry.overrides)
        .unwrap_or_else(|_| ShortcutRegistry::platform_defaults(platform));

    let mut configurable_col = div().flex().flex_col().gap(px(6.0)).child(
        div()
            .text_size(px(11.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_muted)
            .child("CONFIGURABLE COMMANDS (11)"),
    );
    for cmd in ShortcutCommand::ALL {
        let is_overridden = registry.overrides.contains_key(cmd.id());
        let tokens = resolved
            .get(cmd.id())
            .map(|ch| ch.display_tokens(platform))
            .unwrap_or_default();
        let mut keycaps = div().flex().items_center().gap(px(4.0));
        for tok in tokens {
            keycaps = keycaps.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .bg(bg_control)
                    .border_1()
                    .border_color(if is_overridden { accent } else { border })
                    .text_size(px(11.0))
                    .text_color(text_primary)
                    .child(tok),
            );
        }
        configurable_col = configurable_col.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .py(px(3.0))
                .child(div().text_size(px(12.0)).text_color(text_primary).child(cmd.label()))
                .child(keycaps),
        );
    }

    let mut fixed_col = div().flex().flex_col().gap(px(6.0)).child(
        div()
            .text_size(px(11.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_muted)
            .child("FIXED BUILT-IN SHORTCUTS (4)"),
    );
    for grp in FixedShortcutGroup::ALL {
        fixed_col = fixed_col.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .py(px(3.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::Lock, 12.0, text_muted))
                        .child(div().text_size(px(12.0)).text_color(text_primary).child(grp.label())),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(border)
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(grp.display_chord(platform)),
                ),
        );
    }

    div()
        .w(px(620.0))
        .p(px(20.0))
        .rounded(px(20.0))
        .bg(bg_sheet)
        .border_1()
        .border_color(pal_rgba(palette, "--openbot-dialog-ring", 0x333335))
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Keyboard Shortcuts"))
                .child(div().px(px(8.0)).py(px(3.0)).rounded(px(999.0)).bg(bg_control).text_size(px(11.0)).text_color(text_muted).child(if is_darwin(platform) { "macOS".to_string() } else { platform.to_string() })),
        )
        .child(configurable_col)
        .child(div().h(px(1.0)).w_full().bg(border))
        .child(fixed_col)
        .when(!registry.overrides.is_empty(), |el| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .text_color(accent)
                    .child(format!("{} custom shortcut override(s) active", registry.overrides.len())),
            )
        })
}
