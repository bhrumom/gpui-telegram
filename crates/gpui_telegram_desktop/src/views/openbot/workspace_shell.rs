//! Root `OpenBotWorkspaceShell` GPUI `Render` entity and dual-surface fusion invariants (`OB-F14`..`OB-F20`).
//!
//! Composes the complete 1:1 OpenBot desktop workspace inside `gpui_telegram_desktop`:
//! - Left chrome: `PageRail` (11 pages, `UsageRings`, `RailMoreMenu`, `CreditsRailChip`)
//!   + `ServerRail` (`56/64/72px` responsive + `HiddenServerFolder` + `1400ms` `beam()`)
//!   + reorderable `WorkspaceSidebar` (`280px` default, `88px` compact, `[128, 400]` clamp, `Cmd+B` close)
//! - Center workspace: `FirstAgentSetup` (zero starter bots), 1:1 virtualized `ConversationView`
//!   (with `AgentSteps`, `SubagentDock`, `ask_ui` `UiBlockCard`, `RuntimeTurnCard`, `QueuePanel`,
//!   `VoicePill`, `@`/`$` `Composer`), multi-bot `ChannelConversation` + `ChannelSettingsPane`,
//!   `WorkflowCanvas`, `RoutineCalendar`, `TasksPanel`, `LibraryPanel`, `MarketplacePage`,
//!   `CrmPage`, `UsageAndCreditsPage`, `Providers` matrix, and `AppSettings` / `ServerSettings`
//! - Right workspace: vertically-splittable `SidePanel` (`360..1600px`, `MAX_SIDE_SPLITS = 2`)
//! - Deferred modals & 7 Desktop OS overlays (`GlobalSearch`, `ChannelCreateDialog`, `TraceWaterfall`,
//!   `ShortcutsOverlay`, `DynamicIsland`, `AgentDock`, `BrowserDock`, `CuaOverlay`, `CloseGuard`,
//!   `RecoverySurface`, `ThemeSweepOverlay`, and `BeamWaveOverlay`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::*;
use openbot_contracts::avatar::bloub_avatar_profile;
use openbot_contracts::channels::{ChannelMessageAuthor, ChannelMessageAuthorKind};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};
use openbot_contracts::providers::AGENT_PROVIDER_DESCRIPTORS;
use openbot_contracts::queue::{QueueDeliveryStatus, QueueSender};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{AgentAvatarEditorState, ChannelAgentBrief};
use crate::theme::openbot_palette::{
    BeamWaveState, OpenBotColorScheme, OpenBotPalette, ThemeSweepState, render_beam_overlay,
    render_theme_sweep_overlay,
};

use super::agent_studio::{
    AgentSettingsState, FirstAgentSetupState, GlobalSearchState, render_agent_settings_panel,
    render_first_agent_setup, render_global_search_modal,
};
use super::ask_ui::{RuntimeTurnCard, UiBlockCardState, render_runtime_turn_card, render_ui_block_card};
use super::channel_editor::{
    ChannelEditorAgentOption, ChannelEditorState, ChannelSettingsPaneState,
    render_channel_create_dialog, render_channel_settings_pane,
};
use super::channel_view::{
    ChannelConversationState, ChannelTimelineMessage, render_channel_conversation,
};
use super::composer::{ComposerState, ComposerTrigger, render_composer};
use super::conversation::{ChatTimelineRow, ConversationViewState, render_conversation_view};
use super::os_surfaces::{
    DesktopOsSurfacesState, render_agent_dock, render_browser_dock, render_close_guard_dialog,
    render_cua_overlay, render_dynamic_island, render_recovery_surface,
};
use super::page_rail::render_page_rail;
use super::queue_and_voice::{QueuePanelState, VoicePillState, render_queue_panel, render_voice_pill};
use super::secondary_pages::{
    CrmPageState, LibraryPanelState, MarketplacePageState, RoutineCalendarState, TasksPanelState,
    UsageAndCreditsState, render_crm_page, render_library_panel, render_marketplace_page,
    render_routine_calendar, render_tasks_panel, render_usage_and_credits,
};
use super::server_rail::render_server_rail;
use super::settings_and_shortcuts::{
    AppSettingsState, ServerSettingsState, ShortcutChord, ShortcutCommand, ShortcutRegistry,
    render_app_settings_modal, render_server_settings_modal, render_shortcuts_overlay,
};
use super::side_panel::{SidePanelPaneKind, SidePanelState, render_side_panel};
use super::sidebar::{SidebarAgentItem, SidebarChannelItem, render_workspace_sidebar};
use super::shell_state::{
    LEFT_PANEL_COMPACT, LEFT_PANEL_DEFAULT, LEFT_PANEL_MAX, LEFT_PANEL_MIN, OpenBotPage,
    OpenBotShellState, SERVER_RAIL_WIDTH, ServerPlatform, UsageRingMetric,
};
use super::steps_and_trace::{
    AgentStepsState, SubagentDockState, TraceWaterfallState, render_agent_steps,
    render_subagent_dock, render_trace_waterfall_dialog,
};
use super::workflow_canvas::{WorkflowCanvasState, render_workflow_canvas};

/// Exact canonical Telegram `Cmd+1..9` navigation destination labels preserved across dual-surface fusion.
pub const TELEGRAM_CMD_1_TO_9_DESTINATIONS: [&str; 9] = [
    "Chats",
    "Folders",
    "Calls",
    "TaskCenter",
    "McpTools",
    "TalkMode",
    "SavedMessages",
    "Settings",
    "Browser",
];

/// Exact canonical TDesktop theme cycle preserved untouched alongside `--openbot-*` tokens.
pub const TELEGRAM_TDESKTOP_THEME_CYCLE: [&str; 4] = ["Night", "Tinted", "Day", "Classic"];

/// Legacy 8 GrokBot avatar shapes preserved alongside `classic`, `bloub:*`, and `plush:*`.
pub const LEGACY_GROK_AVATAR_SHAPES: [&str; 8] = [
    "blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop",
];

/// Active target selected inside the OpenBot `Agents` primary view.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ActiveOpenBotTarget {
    #[default]
    None,
    Agent(String),
    Channel(String),
}

/// Immutable baseline Telegram 4-pane geometry, shortcut, and palette invariants (`OB-F20`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramBaselineInvariants {
    pub nav_rail_px: u32,
    pub chat_sidebar_px: u32,
    pub sidebar_row_px: u32,
    pub bubble_min_max_px: [u32; 2],
    pub inspector_px: u32,
    pub composer_min_max_px: [u32; 2],
    pub shortcuts_preserved: bool,
    pub palette_untouched: bool,
    pub cmd_1_to_9: Vec<String>,
    pub tdesktop_cycle: Vec<String>,
}

impl Default for TelegramBaselineInvariants {
    fn default() -> Self {
        Self {
            nav_rail_px: 52,
            chat_sidebar_px: 288,
            sidebar_row_px: 62,
            bubble_min_max_px: [160, 430],
            inspector_px: 320,
            composer_min_max_px: [44, 192],
            shortcuts_preserved: true,
            palette_untouched: true,
            cmd_1_to_9: TELEGRAM_CMD_1_TO_9_DESTINATIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            tdesktop_cycle: TELEGRAM_TDESKTOP_THEME_CYCLE
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

/// Compact OpenBot shell layout summary exposed inside [`DualSurfaceFusionSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenBotShellSummarySnapshot {
    pub page_rail_count: usize,
    pub page_rail_pages: Vec<String>,
    pub server_rail_px: u32,
    pub sidebar_width_px: u32,
    pub sidebar_compact_px: u32,
    pub sidebar_clamp_px: [u32; 2],
    pub side_panel_splits: Vec<String>,
}

impl Default for OpenBotShellSummarySnapshot {
    fn default() -> Self {
        Self {
            page_rail_count: OpenBotPage::ALL.len(),
            page_rail_pages: OpenBotPage::ALL
                .iter()
                .map(|p| p.as_str().to_string())
                .collect(),
            server_rail_px: SERVER_RAIL_WIDTH as u32,
            sidebar_width_px: LEFT_PANEL_DEFAULT as u32,
            sidebar_compact_px: LEFT_PANEL_COMPACT as u32,
            sidebar_clamp_px: [LEFT_PANEL_MIN as u32, LEFT_PANEL_MAX as u32],
            side_panel_splits: vec!["browser".to_string(), "files".to_string()],
        }
    }
}

/// Deterministic dual-surface routing and baseline preservation snapshot (`OB-F20`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DualSurfaceFusionSnapshot {
    pub active_panel: String,
    pub nav_destination: String,
    pub tdesktop_mode: String,
    pub openbot_shell: OpenBotShellSummarySnapshot,
    pub telegram_baseline: TelegramBaselineInvariants,
}

impl Default for DualSurfaceFusionSnapshot {
    fn default() -> Self {
        Self {
            active_panel: "OpenBot".to_string(),
            nav_destination: "OpenBot".to_string(),
            tdesktop_mode: "Night".to_string(),
            openbot_shell: OpenBotShellSummarySnapshot::default(),
            telegram_baseline: TelegramBaselineInvariants::default(),
        }
    }
}

/// Maps dual-surface workspace shortcuts (`Cmd/Ctrl+0`, `Cmd/Ctrl+Shift+O`, `Cmd/Ctrl+1..9`)
/// to `(active_panel, nav_destination)`.
pub fn resolve_workspace_shortcut(shortcut: &str) -> Option<(&'static str, &'static str)> {
    let norm = shortcut.trim().to_ascii_lowercase().replace(' ', "");
    match norm.as_str() {
        "cmd+0" | "ctrl+0" | "meta+0" | "cmd+shift+o" | "ctrl+shift+o" | "meta+shift+o" => {
            Some(("OpenBot", "OpenBot"))
        }
        "cmd+1" | "ctrl+1" | "meta+1" => Some(("Chats", TELEGRAM_CMD_1_TO_9_DESTINATIONS[0])),
        "cmd+2" | "ctrl+2" | "meta+2" => Some(("Chats", TELEGRAM_CMD_1_TO_9_DESTINATIONS[1])),
        "cmd+3" | "ctrl+3" | "meta+3" => Some(("Calls", TELEGRAM_CMD_1_TO_9_DESTINATIONS[2])),
        "cmd+4" | "ctrl+4" | "meta+4" => Some(("TaskCenter", TELEGRAM_CMD_1_TO_9_DESTINATIONS[3])),
        "cmd+5" | "ctrl+5" | "meta+5" => Some(("McpTools", TELEGRAM_CMD_1_TO_9_DESTINATIONS[4])),
        "cmd+6" | "ctrl+6" | "meta+6" => Some(("TalkMode", TELEGRAM_CMD_1_TO_9_DESTINATIONS[5])),
        "cmd+7" | "ctrl+7" | "meta+7" => Some(("Chats", TELEGRAM_CMD_1_TO_9_DESTINATIONS[6])),
        "cmd+8" | "ctrl+8" | "meta+8" => Some(("Settings", TELEGRAM_CMD_1_TO_9_DESTINATIONS[7])),
        "cmd+9" | "ctrl+9" | "meta+9" => Some(("Browser", TELEGRAM_CMD_1_TO_9_DESTINATIONS[8])),
        _ => None,
    }
}

/// Classifies a unified Telegram/OpenBot avatar shape descriptor into its rendering engine
/// (`"initials"`, `"grok_bot"`, `"bloub_flat"`, `"bloub_plush"`), matching `components/avatar.rs`.
pub fn classify_unified_avatar_spec(raw_spec: &str) -> &'static str {
    let lower = raw_spec.trim().to_ascii_lowercase();
    if lower == "classic" || lower.is_empty() {
        return "initials";
    }
    if lower == "bloub" || lower.starts_with("bloub:") {
        return "bloub_flat";
    }
    if lower == "plush" || lower.starts_with("plush:") {
        return "bloub_plush";
    }
    if LEGACY_GROK_AVATAR_SHAPES.contains(&lower.as_str()) {
        return "grok_bot";
    }
    "initials"
}

/// Evaluates the fallback shape resolved for `"bloub:goutte"` (`goutte` is strictly excluded).
pub fn resolve_goutte_fallback_shape() -> &'static str {
    bloub_avatar_profile("goutte", None, None).shape.as_str()
}

impl DualSurfaceFusionSnapshot {
    /// Evaluates routing across an optional `shortcut`, optional `switch_sequence`, and `tdesktop_mode`.
    pub fn evaluate(
        shortcut: Option<&str>,
        switch_sequence: Option<&[String]>,
        tdesktop_mode: Option<&str>,
    ) -> Self {
        let mut snap = Self::default();
        if let Some(mode) = tdesktop_mode.map(str::trim).filter(|m| !m.is_empty()) {
            snap.tdesktop_mode = mode.to_string();
        }
        if let Some(sc) = shortcut {
            if let Some((panel, dest)) = resolve_workspace_shortcut(sc) {
                snap.active_panel = panel.to_string();
                snap.nav_destination = dest.to_string();
            }
        }
        if let Some(seq) = switch_sequence {
            for target in seq {
                let trimmed = target.trim();
                if trimmed.is_empty() {
                    continue;
                }
                snap.active_panel = trimmed.to_string();
                snap.nav_destination = trimmed.to_string();
            }
        }
        snap
    }
}

/// Root stateful GPUI `Render` entity assembling the entire 1:1 OpenBot desktop workspace.
pub struct OpenBotWorkspaceShell {
    pub shell: OpenBotShellState,
    pub palette: OpenBotPalette,
    pub color_scheme: OpenBotColorScheme,
    pub lang_id: String,
    pub reduced_motion: bool,
    pub active_page: OpenBotPage,
    pub active_target: ActiveOpenBotTarget,
    pub platform: ServerPlatform,
    pub window_width_px: u32,
    pub now_sec: f64,
    pub now_ms: u64,
    pub agents: Vec<SidebarAgentItem>,
    pub channels: Vec<SidebarChannelItem>,
    pub usage_rings: Vec<UsageRingMetric>,
    pub conversation: ConversationViewState,
    pub agent_steps: AgentStepsState,
    pub trace_waterfall: TraceWaterfallState,
    pub subagent_dock: SubagentDockState,
    pub active_ui_block: Option<UiBlockCardState>,
    pub runtime_turn_cards: Vec<RuntimeTurnCard>,
    pub composer: ComposerState,
    pub queue_panel: QueuePanelState,
    pub voice_pill: VoicePillState,
    pub side_panel: SidePanelState,
    pub channel_conversation: ChannelConversationState,
    pub channel_editor: ChannelEditorState,
    pub channel_settings: ChannelSettingsPaneState,
    pub show_channel_create_dialog: bool,
    pub workflow_canvas: WorkflowCanvasState,
    pub routine_calendar: RoutineCalendarState,
    pub tasks_panel: TasksPanelState,
    pub library_panel: LibraryPanelState,
    pub marketplace: MarketplacePageState,
    pub crm_page: CrmPageState,
    pub usage_credits: UsageAndCreditsState,
    pub app_settings: AppSettingsState,
    pub server_settings: ServerSettingsState,
    pub shortcuts: ShortcutRegistry,
    pub show_shortcuts_overlay: bool,
    pub show_server_settings: bool,
    pub global_search: GlobalSearchState,
    pub first_agent_setup: FirstAgentSetupState,
    pub agent_settings: AgentSettingsState,
    pub show_agent_settings: bool,
    pub show_trace_dialog: bool,
    pub show_channel_settings: bool,
    pub active_voice_call: bool,
    pub os_surfaces: DesktopOsSurfacesState,
    pub theme_sweep: Option<ThemeSweepState>,
    pub beam_wave: Option<BeamWaveState>,
    pub data_dir: Option<PathBuf>,
    /// When `true`, `AppShell` already provides the outer `52px` `PageRail` and workspace card frame,
    /// so `render_workspace_root()` suppresses its internal `page_rail` and `server_rail`.
    pub embedded_in_app_shell: bool,
    pending_page_select: Arc<Mutex<Option<OpenBotPage>>>,
}

impl Default for OpenBotWorkspaceShell {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenBotWorkspaceShell {
    /// Creates a fresh zero-starter-bot `OpenBotWorkspaceShell` matching upstream first-run state.
    pub fn new() -> Self {
        let platform = ServerPlatform::current();
        let window_width_px = 1280;
        let color_scheme = OpenBotColorScheme::Dark;
        let shell = OpenBotShellState::new(platform, window_width_px, false);
        let side_panel = SidePanelState {
            open: false,
            ..SidePanelState::default()
        };
        Self {
            shell,
            palette: OpenBotPalette::for_scheme(color_scheme),
            color_scheme,
            lang_id: "en".to_string(),
            reduced_motion: false,
            active_page: OpenBotPage::Agents,
            active_target: ActiveOpenBotTarget::None,
            platform,
            window_width_px,
            now_sec: 0.0,
            now_ms: 0,
            agents: Vec::new(),
            channels: Vec::new(),
            usage_rings: Vec::new(),
            conversation: ConversationViewState::new("", "OpenBot", "openbot"),
            agent_steps: AgentStepsState::default(),
            trace_waterfall: TraceWaterfallState::default(),
            subagent_dock: SubagentDockState::default(),
            active_ui_block: None,
            runtime_turn_cards: Vec::new(),
            composer: ComposerState::new(false),
            queue_panel: QueuePanelState::new(),
            voice_pill: VoicePillState::default(),
            side_panel,
            channel_conversation: ChannelConversationState::new("", "", Vec::new(), None),
            channel_editor: ChannelEditorState::new_create(),
            channel_settings: ChannelSettingsPaneState::default(),
            show_channel_create_dialog: false,
            workflow_canvas: WorkflowCanvasState::with_sample_pipeline(),
            routine_calendar: RoutineCalendarState::default(),
            tasks_panel: TasksPanelState::default(),
            library_panel: LibraryPanelState::default(),
            marketplace: MarketplacePageState::new(false),
            crm_page: CrmPageState::default(),
            usage_credits: UsageAndCreditsState::default(),
            app_settings: AppSettingsState::default(),
            server_settings: ServerSettingsState::default(),
            shortcuts: ShortcutRegistry::new(platform.as_str()),
            show_shortcuts_overlay: false,
            show_server_settings: false,
            global_search: GlobalSearchState::new(false),
            first_agent_setup: FirstAgentSetupState::first_run(),
            agent_settings: AgentSettingsState::default(),
            show_agent_settings: false,
            show_trace_dialog: false,
            show_channel_settings: false,
            active_voice_call: false,
            os_surfaces: DesktopOsSurfacesState::new(),
            theme_sweep: None,
            beam_wave: None,
            data_dir: None,
            embedded_in_app_shell: false,
            pending_page_select: Arc::new(Mutex::new(None)),
        }
    }

    /// Configures whether this workspace shell is embedded inside `AppShell`'s workspace card
    /// (suppressing duplicate internal `PageRail` and `ServerRail`).
    pub fn set_embedded_in_app_shell(&mut self, embedded: bool) {
        self.embedded_in_app_shell = embedded;
    }

    /// Creates an `OpenBotWorkspaceShell` hydrated from `<data_dir>/openbot-*-v1.json` preference stores.
    pub fn with_data_dir(data_dir: &Path) -> Self {
        let mut this = Self::new();
        this.data_dir = Some(data_dir.to_path_buf());
        this.shell.load_from_data_dir(data_dir);
        this.active_page = this.shell.page_rail.active_page;
        this
    }

    /// Synchronizes the OpenBot color scheme from host theme switches and starts a `sweepTheme`
    /// wave (`1200ms`, or `0ms` instant under `reduced_motion`).
    pub fn sync_color_scheme(&mut self, scheme: OpenBotColorScheme) {
        if self.color_scheme != scheme {
            let before = self.color_scheme;
            self.color_scheme = scheme;
            self.palette = OpenBotPalette::for_scheme(scheme);
            self.app_settings.theme_mode = match scheme {
                OpenBotColorScheme::Dark => "dark".to_string(),
                OpenBotColorScheme::Light => "light".to_string(),
            };
            if !self.reduced_motion {
                self.theme_sweep = Some(ThemeSweepState::new(before, scheme, false));
            } else {
                self.theme_sweep = None;
            }
        }
    }

    /// Synchronizes the host UI locale tag (e.g. `"en"`, `"zh-CN"`, `"pl-PL"`).
    pub fn sync_locale(&mut self, lang: &str) {
        let trimmed = lang.trim();
        if !trimmed.is_empty() {
            self.lang_id = trimmed.to_string();
        }
    }

    /// Updates viewport width and recomputes responsive `ServerRail` and `Sidebar` auto-compact rules.
    pub fn set_window_width(&mut self, width_px: u32) {
        self.window_width_px = width_px.max(320);
        self.shell.update_window_width(self.window_width_px);
        self.os_surfaces.main_window.width_px = self.window_width_px;
    }

    /// Propagates OS reduced-motion preference across all animated surfaces and avatars.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        self.shell.set_reduced_motion(reduced_motion);
        self.global_search.set_reduced_motion(reduced_motion);
        self.conversation.reduced_motion = reduced_motion;
        self.conversation.streaming_pacer.reduced_motion = reduced_motion;
        self.agent_steps.reduced_motion = reduced_motion;
        self.subagent_dock.reduced_motion = reduced_motion;
        self.marketplace.no_match_plush.set_reduced_motion(reduced_motion);
        self.app_settings.reduced_motion = reduced_motion;
        self.os_surfaces.dynamic_island.reduced_motion = reduced_motion;
        self.os_surfaces.agent_dock.reduced_motion = reduced_motion;
        self.os_surfaces.cua.reduced_motion = reduced_motion;
        if reduced_motion {
            self.theme_sweep = None;
            self.beam_wave = None;
        }
    }

    /// Navigates the left `PageRail` to one of the 11 canonical OpenBot pages.
    pub fn select_page(&mut self, page: OpenBotPage) {
        self.active_page = page;
        self.shell.page_rail.active_page = page;
        if let Some(dir) = self.data_dir.as_deref() {
            let _ = self.shell.save_to_data_dir(dir);
        }
    }

    /// Selects a 1-on-1 agent conversation in the `Agents` workspace and synchronizes composer/settings.
    pub fn select_agent(&mut self, agent_id: &str) {
        self.active_page = OpenBotPage::Agents;
        self.shell.page_rail.active_page = OpenBotPage::Agents;
        self.active_target = ActiveOpenBotTarget::Agent(agent_id.to_string());
        self.shell.sidebar.selected_agent_id = Some(agent_id.to_string());
        self.shell.sidebar.selected_channel_id = None;
        self.composer.in_channel = false;
        self.composer.channel_archived_read_only = false;
        self.show_channel_settings = false;

        if let Some(item) = self.agents.iter().find(|a| a.id == agent_id) {
            if self.conversation.agent_id != item.id {
                self.conversation = ConversationViewState::new(
                    item.id.clone(),
                    item.name.clone(),
                    item.avatar_seed.clone(),
                );
                self.conversation.agent_title = (!item.title.is_empty()).then(|| item.title.clone());
                self.conversation.agent_avatar_hue = item.avatar_hue;
                self.conversation.agent_avatar_color = item.avatar_color.clone();
                self.conversation.agent_mood = item.mood;
                self.conversation.reduced_motion = self.reduced_motion;
            }
            self.agent_settings = AgentSettingsState::new(item.id.clone(), item.name.clone());
            self.agent_settings.title = item.title.clone();
            self.agent_settings.avatar_editor = AgentAvatarEditorState::new(
                &item.id,
                Some(item.avatar_seed.clone()),
                item.avatar_hue,
                item.avatar_color.clone(),
                item.avatar_url.clone(),
            );
        } else if self.conversation.agent_id != agent_id {
            self.conversation = ConversationViewState::new(agent_id, agent_id, agent_id);
            self.conversation.reduced_motion = self.reduced_motion;
        }
        self.sync_composer_available_agents();
    }

    /// Selects a multi-bot shared channel conversation and synchronizes channel composer/settings.
    pub fn select_channel(&mut self, channel_id: &str) {
        self.active_page = OpenBotPage::Agents;
        self.shell.page_rail.active_page = OpenBotPage::Agents;
        self.active_target = ActiveOpenBotTarget::Channel(channel_id.to_string());
        self.shell.sidebar.selected_channel_id = Some(channel_id.to_string());
        self.shell.sidebar.selected_agent_id = None;
        self.composer.in_channel = true;
        self.show_agent_settings = false;

        if let Some(ch) = self.channels.iter().find(|c| c.id == channel_id) {
            let member_ids: Vec<String> = ch.members.iter().map(|m| m.agent_id.clone()).collect();
            let lead_id = member_ids.first().cloned();
            if self.channel_conversation.channel_id != ch.id {
                let mut conv =
                    ChannelConversationState::new(ch.id.clone(), ch.name.clone(), member_ids, lead_id);
                conv.icon = ch.icon.clone();
                conv.image_url = ch.image_url.clone();
                conv.image_failed = ch.image_failed;
                conv.set_archived(ch.archived);
                for agent in &self.agents {
                    conv.agent_names.insert(agent.id.clone(), agent.name.clone());
                }
                self.channel_conversation = conv;
            }
            self.composer.channel_archived_read_only = ch.archived;
        } else if self.channel_conversation.channel_id != channel_id {
            self.channel_conversation =
                ChannelConversationState::new(channel_id, channel_id, Vec::new(), None);
            self.composer.channel_archived_read_only = false;
        }
        self.sync_composer_available_agents();
    }

    /// Switches the active OpenBot server and fires the `1400ms` radial `beam()` overlay (`0ms` reduced).
    pub fn switch_server(&mut self, server_id: &str, origin_y_px: f32) {
        let _spec = self
            .shell
            .server_rail
            .select_server(server_id, origin_y_px, self.reduced_motion);
        if !self.reduced_motion {
            self.beam_wave = Some(BeamWaveState::new(
                point(px(32.0), px(origin_y_px)),
                size(px(self.window_width_px as f32), px(800.0)),
                true,
                false,
            ));
        } else {
            self.beam_wave = None;
        }
    }

    fn sync_composer_available_agents(&mut self) {
        self.composer.available_agents = self
            .agents
            .iter()
            .map(|a| (a.id.clone(), a.name.clone()))
            .collect();
    }

    fn channel_agent_briefs(&self) -> Vec<ChannelAgentBrief> {
        self.agents.iter().map(SidebarAgentItem::to_channel_brief).collect()
    }

    fn channel_editor_options(&self) -> Vec<ChannelEditorAgentOption> {
        self.agents
            .iter()
            .map(|a| ChannelEditorAgentOption {
                id: a.id.clone(),
                name: a.name.clone(),
                title: a.title.clone(),
                description: a.preview.clone(),
                avatar_seed: Some(a.avatar_seed.clone()),
                avatar_hue: a.avatar_hue,
                avatar_color: a.avatar_color.clone(),
                avatar_url: a.avatar_url.clone(),
            })
            .collect()
    }

    /// Routes committed text / IME input into whichever OpenBot text surface currently holds focus.
    pub fn handle_text_input(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.global_search.open {
            let mut next = self.global_search.query.clone();
            next.push_str(text);
            let caret_x = (next.chars().count() as f64) * 7.5;
            self.global_search.set_query(next, caret_x, self.now_ms);
            return;
        }
        if self.show_channel_create_dialog {
            let mut next = self.channel_editor.name.clone();
            next.push_str(text);
            self.channel_editor.set_name(next);
            return;
        }
        if self.conversation.search.open {
            let mut next = self.conversation.search.query.clone();
            next.push_str(text);
            self.conversation.run_search(&next);
            return;
        }
        if self.active_page == OpenBotPage::Agents
            && self.agents.is_empty()
            && self.active_target == ActiveOpenBotTarget::None
        {
            let mut next = self.first_agent_setup.purpose.clone();
            next.push_str(text);
            let _ = self.first_agent_setup.set_purpose(next);
            return;
        }
        if self.composer.channel_archived_read_only {
            return;
        }
        if text.chars().count() > super::composer::COMPOSER_PASTED_TEXT_LIMIT {
            self.composer.paste_text(text);
        } else {
            self.composer.text.push_str(text);
            self.sync_composer_picker_from_text();
        }
    }

    /// Handles Backspace in whichever OpenBot text surface currently holds focus.
    pub fn handle_backspace(&mut self) {
        if self.global_search.open {
            let mut next = self.global_search.query.clone();
            next.pop();
            let caret_x = (next.chars().count() as f64) * 7.5;
            self.global_search.set_query(next, caret_x, self.now_ms);
            return;
        }
        if self.show_channel_create_dialog {
            let mut next = self.channel_editor.name.clone();
            next.pop();
            self.channel_editor.set_name(next);
            return;
        }
        if self.conversation.search.open {
            let mut next = self.conversation.search.query.clone();
            next.pop();
            self.conversation.run_search(&next);
            return;
        }
        if self.active_page == OpenBotPage::Agents
            && self.agents.is_empty()
            && self.active_target == ActiveOpenBotTarget::None
        {
            let mut next = self.first_agent_setup.purpose.clone();
            next.pop();
            let _ = self.first_agent_setup.set_purpose(next);
            return;
        }
        self.composer.text.pop();
        self.sync_composer_picker_from_text();
    }

    /// Handles Enter / Cmd+Enter across search, dialogs, and the main/channel composer.
    pub fn handle_enter(&mut self, cmd_or_ctrl: bool) {
        if self.global_search.open {
            self.global_search.flush_debounce();
            self.global_search.open = false;
            return;
        }
        if self.conversation.search.open {
            self.conversation.next_search_match();
            return;
        }
        if self.composer.active_trigger.is_some() {
            self.composer.close_picker();
            return;
        }
        let should_send = if self.composer.send_on_enter {
            !cmd_or_ctrl || cmd_or_ctrl
        } else {
            cmd_or_ctrl
        };
        if should_send && self.composer.can_send() {
            self.submit_composer_message();
        } else if !should_send {
            self.composer.text.push('\n');
        }
    }

    /// Handles Tab: steers queued turn (enforcing `steerRefused` on routine turns) or cycles search tabs.
    pub fn handle_tab(&mut self) {
        if self.global_search.open {
            let tabs = super::agent_studio::GlobalSearchTab::ALL;
            let idx = tabs
                .iter()
                .position(|&t| t == self.global_search.active_tab)
                .unwrap_or(0);
            self.global_search.active_tab = tabs[(idx + 1) % tabs.len()];
            return;
        }
        if self.composer.active_turn_running || !self.queue_panel.items.is_empty() {
            let eval = self.queue_panel.evaluate_steer_for_state(true);
            if let Some(first) = self.queue_panel.items.first_mut() {
                if eval.steered_on_tab {
                    first.status = QueueDeliveryStatus::Running;
                    first.routine_steer_refused = false;
                } else if eval.steer_refused {
                    first.routine_steer_refused = true;
                }
            }
        }
    }

    /// Handles Escape by dismissing the topmost modal, popover, search bar, or maximized panel.
    pub fn handle_escape(&mut self) {
        if self.show_shortcuts_overlay {
            self.show_shortcuts_overlay = false;
        } else if self.global_search.open {
            self.global_search.open = false;
        } else if self.show_channel_create_dialog {
            self.show_channel_create_dialog = false;
        } else if self.show_server_settings {
            self.show_server_settings = false;
        } else if self.show_trace_dialog {
            self.show_trace_dialog = false;
        } else if self.composer.active_trigger.is_some() {
            self.composer.close_picker();
        } else if self.conversation.search.open {
            self.conversation.search.open = false;
        } else if self.side_panel.maximized {
            self.side_panel.maximized = false;
        } else if self.show_agent_settings {
            self.show_agent_settings = false;
        } else if self.show_channel_settings {
            self.show_channel_settings = false;
        }
    }

    /// Resolves and dispatches a keyboard chord through the active platform `ShortcutRegistry`.
    pub fn handle_shortcut(&mut self, chord: &ShortcutChord) -> bool {
        let primary_mod = if self.platform == ServerPlatform::MacOs {
            chord.meta && !chord.ctrl
        } else {
            chord.ctrl && !chord.meta
        };
        if primary_mod && !chord.alt && !chord.shift && chord.code.eq_ignore_ascii_case("Comma") {
            self.select_page(OpenBotPage::Settings);
            return true;
        }
        if primary_mod && !chord.alt && !chord.shift && chord.code.eq_ignore_ascii_case("Period") {
            self.composer.active_turn_running = false;
            self.queue_panel.pause();
            return true;
        }
        if primary_mod && !chord.alt && chord.shift && chord.code.eq_ignore_ascii_case("KeyE") {
            self.side_panel.open = !self.side_panel.open;
            self.conversation.side_panel_open = self.side_panel.open;
            return true;
        }
        let Ok(resolved) = self.shortcuts.resolve() else {
            return false;
        };
        let target_key = chord.normalized_key();
        let matched = resolved
            .iter()
            .find(|(_, bound)| bound.normalized_key() == target_key)
            .and_then(|(cmd_id, _)| ShortcutCommand::from_id(cmd_id));
        let Some(cmd) = matched else {
            return false;
        };
        match cmd {
            ShortcutCommand::ToggleSidebar => {
                self.shell.sidebar.toggle_closed();
            }
            ShortcutCommand::GlobalSearch => {
                self.global_search.open = !self.global_search.open;
            }
            ShortcutCommand::SearchChat => {
                self.conversation.search.open = !self.conversation.search.open;
            }
            ShortcutCommand::ShowShortcuts => {
                self.show_shortcuts_overlay = !self.show_shortcuts_overlay;
            }
            ShortcutCommand::NewAgent => {
                self.active_page = OpenBotPage::Agents;
                self.active_target = ActiveOpenBotTarget::None;
                self.first_agent_setup = FirstAgentSetupState::first_run();
            }
            ShortcutCommand::NewThread => {
                let idx = self.conversation.threads.len() + 1;
                for tab in &mut self.conversation.threads {
                    tab.active = false;
                }
                self.conversation
                    .threads
                    .push(super::conversation::ThreadTabItem {
                        id: format!("thread-{idx}"),
                        title: format!("Thread {idx}"),
                        active: true,
                        unread_count: 0,
                    });
            }
            ShortcutCommand::PreviousThread | ShortcutCommand::NextThread => {
                let len = self.conversation.threads.len();
                if len > 1 {
                    let cur = self
                        .conversation
                        .threads
                        .iter()
                        .position(|t| t.active)
                        .unwrap_or(0);
                    let next = if cmd == ShortcutCommand::NextThread {
                        (cur + 1) % len
                    } else {
                        (cur + len - 1) % len
                    };
                    for (i, tab) in self.conversation.threads.iter_mut().enumerate() {
                        tab.active = i == next;
                    }
                }
            }
            ShortcutCommand::FocusComposer => {
                self.global_search.open = false;
                self.show_shortcuts_overlay = false;
            }
            ShortcutCommand::Back => {
                self.shell.sidebar.history_forward_available = true;
            }
            ShortcutCommand::Forward => {
                self.shell.sidebar.history_back_available = true;
            }
        }
        true
    }

    fn sync_composer_picker_from_text(&mut self) {
        let last_token = self
            .composer
            .text
            .rsplit([' ', '\n', '\t'])
            .next()
            .unwrap_or("")
            .to_string();
        if let Some(q) = last_token.strip_prefix('@') {
            self.composer.open_picker(ComposerTrigger::At, q);
        } else if let Some(q) = last_token.strip_prefix('$') {
            self.composer.open_picker(ComposerTrigger::Dollar, q);
        } else {
            self.composer.close_picker();
        }
    }

    fn submit_composer_message(&mut self) {
        let text = self.composer.text.trim().to_string();
        if text.is_empty() && self.composer.attachments.is_empty() {
            return;
        }
        let seq = (self.conversation.rows.len() + self.channel_conversation.messages.len() + 1) as u64;
        if self.composer.active_turn_running {
            let _ = self
                .queue_panel
                .push_item(format!("deliv-{seq}"), text, QueueSender::User);
        } else {
            match &self.active_target {
                ActiveOpenBotTarget::Channel(_) => {
                    if !self.channel_conversation.read_only {
                        self.channel_conversation.messages.push(ChannelTimelineMessage {
                            id: format!("cmsg-{seq}"),
                            sequence: seq,
                            author: ChannelMessageAuthor {
                                kind: ChannelMessageAuthorKind::Member,
                                id: "local".to_string(),
                                name: "You".to_string(),
                            },
                            task_id: None,
                            turn_id: None,
                            item_type: None,
                            reply_to_message_id: self.channel_conversation.reply_to_message_id.take(),
                            reply_preview: None,
                            superseded: false,
                            streaming: false,
                            text,
                            created_at: "just now".to_string(),
                        });
                    }
                }
                ActiveOpenBotTarget::Agent(_) | ActiveOpenBotTarget::None => {
                    let msg = ConversationMessage::new_simple(
                        format!("msg-{seq}"),
                        ConversationMessageAuthor::User,
                        text,
                        "just now",
                        ConversationMessageStatus::Completed,
                    );
                    self.conversation.rows.push(ChatTimelineRow {
                        message: msg,
                        timestamp_ms: self.now_ms as i64,
                        corrupt_override: None,
                        steps: None,
                        ui_block_card: None,
                        runtime_cards: Vec::new(),
                        voice_player: None,
                    });
                }
            }
        }
        self.composer.text.clear();
        self.composer.attachments.clear();
        self.composer.close_picker();
    }

    /// Flushes any page click recorded by the `PageRail` callback into `self.active_page`.
    pub fn flush_pending_page_select(&mut self) {
        let next_page = self.pending_page_select.lock().take();
        if let Some(page) = next_page {
            self.select_page(page);
        }
    }

    /// Renders the complete 1:1 OpenBot desktop shell (`PageRail` + `ServerRail` + `WorkspaceSidebar`
    /// + active center surface + splittable `SidePanel` + deferred modals & 7 Desktop OS overlays).
    pub fn render_workspace_root(&self) -> Div {
        let bg_frame = self
            .palette
            .rgba("--openbot-bg-frame")
            .unwrap_or(rgb(0x0d0d0d));
        let bg_canvas = self
            .palette
            .rgba("--openbot-bg-canvas")
            .unwrap_or(rgb(0x1a1a1a));
        let border_col = self
            .palette
            .rgba("--openbot-border")
            .unwrap_or(rgb(0x222222));

        let show_sidebar = self.active_page == OpenBotPage::Agents && !self.shell.sidebar.closed;
        let show_side_panel =
            self.active_page == OpenBotPage::Agents && self.side_panel.open && !self.side_panel.active_splits.is_empty();

        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(if self.embedded_in_app_shell {
                bg_canvas
            } else {
                bg_frame
            })
            .overflow_hidden();

        if !self.embedded_in_app_shell {
            let pending_sink = Arc::clone(&self.pending_page_select);
            let page_rail = render_page_rail(
                &self.shell.page_rail,
                &self.palette,
                &self.usage_rings,
                self.active_page,
                self.reduced_motion,
                move |page| {
                    *pending_sink.lock() = Some(page);
                },
            );
            let server_rail = render_server_rail(
                &self.shell.server_rail,
                &self.palette,
                &self.shell.server_rail.active_server_id,
                self.platform,
                self.window_width_px,
            );
            root = root.child(page_rail).child(server_rail);
        }

        if show_sidebar {
            root = root.child(render_workspace_sidebar(
                &self.shell.sidebar,
                &self.agents,
                &self.channels,
                &self.palette,
                self.now_sec,
                self.now_ms,
            ));
        }

        if !show_side_panel || !self.side_panel.maximized {
            root = root.child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(bg_canvas)
                    .child(self.render_main_surface()),
            );
        }

        if show_side_panel {
            let mut panel_wrap = div()
                .h_full()
                .min_h(px(0.0))
                .border_l_1()
                .border_color(border_col)
                .child(render_side_panel(&self.side_panel, &self.palette, self.now_sec));
            if self.side_panel.maximized {
                panel_wrap = panel_wrap.flex_1().min_w(px(0.0));
            }
            root = root.child(panel_wrap);
        }

        self.attach_deferred_overlays(root)
    }

    fn render_main_surface(&self) -> Div {
        match self.active_page {
            OpenBotPage::Agents => match &self.active_target {
                ActiveOpenBotTarget::None if self.agents.is_empty() && self.channels.is_empty() => {
                    render_first_agent_setup(&self.first_agent_setup, &self.palette, self.now_sec)
                }
                ActiveOpenBotTarget::Channel(_) => self.render_channel_surface(),
                ActiveOpenBotTarget::Agent(_) | ActiveOpenBotTarget::None => {
                    self.render_agent_conversation_surface()
                }
            },
            OpenBotPage::Workflows => {
                render_workflow_canvas(&self.workflow_canvas, &self.palette, self.now_sec)
            }
            OpenBotPage::Tasks => {
                render_tasks_panel(&self.tasks_panel, &self.palette, self.now_sec)
            }
            OpenBotPage::Schedule => {
                render_routine_calendar(&self.routine_calendar, &self.palette, self.now_sec)
            }
            OpenBotPage::Usage | OpenBotPage::Credits => {
                render_usage_and_credits(&self.usage_credits, &self.palette)
            }
            OpenBotPage::Library => render_library_panel(&self.library_panel, &self.palette),
            OpenBotPage::Providers => self.render_providers_surface(),
            OpenBotPage::Crm => render_crm_page(&self.crm_page, &self.palette),
            OpenBotPage::Marketplace => {
                render_marketplace_page(&self.marketplace, &self.palette)
            }
            OpenBotPage::Settings => {
                render_app_settings_modal(&self.app_settings, &self.palette)
            }
        }
    }

    fn render_agent_conversation_surface(&self) -> Div {
        let mut column = div()
            .flex_1()
            .min_h(px(0.0))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(render_conversation_view(
                        &self.conversation,
                        &self.palette,
                        self.now_sec,
                    )),
            );

        if !self.agent_steps.steps.is_empty() {
            column = column.child(
                div()
                    .px(px(16.0))
                    .pb(px(6.0))
                    .child(render_agent_steps(&self.agent_steps, &self.palette)),
            );
        }

        if !self.subagent_dock.items.is_empty() {
            column = column.child(
                div()
                    .px(px(16.0))
                    .pb(px(6.0))
                    .child(render_subagent_dock(&self.subagent_dock, &self.palette)),
            );
        }

        if let Some(ui_block) = &self.active_ui_block {
            column = column.child(
                div()
                    .px(px(16.0))
                    .pb(px(8.0))
                    .child(render_ui_block_card(ui_block, &self.palette)),
            );
        }

        for card in &self.runtime_turn_cards {
            column = column.child(
                div()
                    .px(px(16.0))
                    .pb(px(6.0))
                    .child(render_runtime_turn_card(card, &self.palette)),
            );
        }

        if !self.queue_panel.items.is_empty() || self.queue_panel.paused {
            column = column.child(
                div()
                    .px(px(16.0))
                    .pb(px(6.0))
                    .child(render_queue_panel(&self.queue_panel, &self.palette)),
            );
        }

        if self.active_voice_call || self.voice_pill.is_live() {
            column = column.child(div().px(px(16.0)).pb(px(6.0)).child(render_voice_pill(
                &self.voice_pill,
                &self.conversation.agent_avatar_seed,
                &self.palette,
                self.now_sec,
            )));
        }

        column = column.child(
            div()
                .px(px(12.0))
                .pb(px(12.0))
                .child(render_composer(&self.composer, &self.palette)),
        );

        if self.show_agent_settings {
            let border = self
                .palette
                .rgba("--openbot-border")
                .unwrap_or(rgb(0x222222));
            div()
                .flex_1()
                .min_h(px(0.0))
                .size_full()
                .flex()
                .flex_row()
                .child(column)
                .child(
                    div()
                        .h_full()
                        .min_h(px(0.0))
                        .border_l_1()
                        .border_color(border)
                        .child(render_agent_settings_panel(
                            &self.agent_settings,
                            &self.palette,
                            self.now_sec,
                        )),
                )
        } else {
            column
        }
    }

    fn render_channel_surface(&self) -> Div {
        let briefs = self.channel_agent_briefs();
        let mut column = div()
            .flex_1()
            .min_h(px(0.0))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(render_channel_conversation(
                        &self.channel_conversation,
                        &briefs,
                        &self.palette,
                        self.now_sec,
                    )),
            );

        if !self.channel_conversation.read_only {
            column = column.child(
                div()
                    .px(px(12.0))
                    .pb(px(12.0))
                    .child(render_composer(&self.composer, &self.palette)),
            );
        }

        if self.show_channel_settings {
            let options = self.channel_editor_options();
            let border = self
                .palette
                .rgba("--openbot-border")
                .unwrap_or(rgb(0x222222));
            div()
                .flex_1()
                .min_h(px(0.0))
                .size_full()
                .flex()
                .flex_row()
                .child(column)
                .child(
                    div()
                        .h_full()
                        .min_h(px(0.0))
                        .border_l_1()
                        .border_color(border)
                        .child(render_channel_settings_pane(
                            &self.channel_settings,
                            &options,
                            &self.palette,
                            self.now_sec,
                        )),
                )
        } else {
            column
        }
    }

    /// Opens or focuses a specific right `SidePanel` pane (`browser`, `files`, `threads`,
    /// `settings`, `question`, `subagent`).
    pub fn toggle_side_panel_pane(&mut self, pane: SidePanelPaneKind) {
        if self.side_panel.open && self.side_panel.active_splits.contains(&pane) {
            self.side_panel.close_pane(pane);
        } else {
            self.side_panel.open_pane(pane);
        }
        self.conversation.side_panel_open = self.side_panel.open;
    }

    fn render_providers_surface(&self) -> Div {
        let bg_canvas = self
            .palette
            .rgba("--openbot-bg-canvas")
            .unwrap_or(rgb(0x1a1a1a));
        let bg_surface = self
            .palette
            .rgba("--openbot-bg-surface")
            .unwrap_or(rgb(0x212121));
        let border = self
            .palette
            .rgba("--openbot-border-grouped")
            .unwrap_or(rgb(0x333335));
        let text_primary = self
            .palette
            .rgba("--openbot-text-primary")
            .unwrap_or(rgb(0xffffff));
        let text_muted = self
            .palette
            .rgba("--openbot-text-muted")
            .unwrap_or(rgb(0x979797));
        let accent = self
            .palette
            .rgba("--openbot-accent")
            .unwrap_or(rgb(0x007cf7));

        let mut cards = div()
            .id("openbot-providers-scroll")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .gap(px(12.0))
            .p(px(20.0));

        for desc in &AGENT_PROVIDER_DESCRIPTORS {
            cards = cards.child(
                div()
                    .w(px(260.0))
                    .p(px(14.0))
                    .rounded(px(12.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_primary)
                                    .child(desc.display_name.to_string()),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(accent)
                                    .text_size(px(10.0))
                                    .text_color(accent)
                                    .child(desc.auth_kind.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted)
                            .child(format!("Default: {}", desc.default_model)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted)
                            .child(desc.onboarding_description.to_string()),
                    ),
            );
        }

        div()
            .flex_1()
            .min_h(px(0.0))
            .size_full()
            .flex()
            .flex_col()
            .bg(bg_canvas)
            .child(
                div()
                    .px(px(20.0))
                    .py(px(14.0))
                    .border_b_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_primary)
                            .child(format!(
                                "Providers ({} built-in)",
                                AGENT_PROVIDER_DESCRIPTORS.len()
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_muted)
                            .child(format!("Locale: {}", self.lang_id)),
                    ),
            )
            .child(cards)
    }

    fn attach_deferred_overlays(&self, mut root: Div) -> Div {
        if self.global_search.open {
            let mut search_clone = self.global_search.clone();
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_start()
                        .justify_center()
                        .pt(px(72.0))
                        .occlude()
                        .child(render_global_search_modal(
                            &mut search_clone,
                            &self.palette,
                            self.now_ms as f64,
                        )),
                )
                .with_priority(50),
            );
        }

        if self.show_channel_create_dialog {
            let options = self.channel_editor_options();
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(render_channel_create_dialog(
                            &self.channel_editor,
                            &options,
                            &self.palette,
                            self.now_sec,
                        )),
                )
                .with_priority(52),
            );
        }

        if self.show_server_settings {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(render_server_settings_modal(
                            &self.server_settings,
                            &self.palette,
                        )),
                )
                .with_priority(54),
            );
        }

        if self.show_trace_dialog {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(render_trace_waterfall_dialog(
                            &self.trace_waterfall,
                            &self.palette,
                        )),
                )
                .with_priority(55),
            );
        }

        if self.show_shortcuts_overlay {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(render_shortcuts_overlay(
                            &self.shortcuts,
                            self.platform.as_str(),
                            &self.palette,
                        )),
                )
                .with_priority(56),
            );
        }

        if self.os_surfaces.dynamic_island.preference.enabled
            && (self.os_surfaces.dynamic_island.interacting
                || self.os_surfaces.dynamic_island.critical_lock
                || self.os_surfaces.dynamic_island.current.is_critical())
        {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .top(px(8.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .occlude()
                        .child(render_dynamic_island(
                            &self.os_surfaces.dynamic_island,
                            &self.palette,
                            self.now_sec,
                        )),
                )
                .with_priority(57),
            );
        }

        if !self.os_surfaces.agent_dock.seats.is_empty() {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .right(px(6.0))
                        .top(px(96.0))
                        .occlude()
                        .child(render_agent_dock(
                            &self.os_surfaces.agent_dock,
                            &self.palette,
                            self.now_sec,
                        )),
                )
                .with_priority(57),
            );
        }

        if self.os_surfaces.browser_dock.is_open() {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .bottom(px(16.0))
                        .right(px(16.0))
                        .occlude()
                        .child(render_browser_dock(
                            &self.os_surfaces.browser_dock,
                            &self.palette,
                        )),
                )
                .with_priority(58),
            );
        }

        if self.os_surfaces.cua.is_visible() {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .child(render_cua_overlay(&self.os_surfaces.cua, &self.palette)),
                )
                .with_priority(58),
            );
        }

        if self.os_surfaces.main_window.close_guard_prompted() {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(render_close_guard_dialog(
                            &self.os_surfaces.main_window,
                            &self.palette,
                        )),
                )
                .with_priority(59),
            );
        }

        if self.os_surfaces.recovery.is_triggered() {
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(render_recovery_surface(
                            &self.os_surfaces.recovery,
                            &self.palette,
                        )),
                )
                .with_priority(60),
            );
        }

        if let Some(sweep) = &self.theme_sweep {
            if !sweep.is_finished() {
                root = root.child(deferred(render_theme_sweep_overlay(sweep)).with_priority(60));
            }
        }

        if let Some(beam) = &self.beam_wave {
            if !beam.is_finished() {
                root = root.child(deferred(render_beam_overlay(beam)).with_priority(60));
            }
        }

        root
    }
}

impl Render for OpenBotWorkspaceShell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.flush_pending_page_select();
        if let Some(sweep) = &mut self.theme_sweep {
            sweep.tick_now();
        }
        if let Some(beam) = &mut self.beam_wave {
            beam.tick_now();
        }
        self.render_workspace_root()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn dual_surface_fusion_invariants_and_shortcut_routing() {
        let base = TelegramBaselineInvariants::default();
        assert_eq!(base.nav_rail_px, 52);
        assert_eq!(base.chat_sidebar_px, 288);
        assert_eq!(base.sidebar_row_px, 62);
        assert_eq!(base.bubble_min_max_px, [160, 430]);
        assert_eq!(base.inspector_px, 320);
        assert_eq!(base.composer_min_max_px, [44, 192]);
        assert_eq!(base.cmd_1_to_9.len(), 9);
        assert_eq!(base.tdesktop_cycle, vec!["Night", "Tinted", "Day", "Classic"]);

        let snap0 = DualSurfaceFusionSnapshot::evaluate(Some("Cmd+0"), None, Some("Night"));
        assert_eq!(snap0.active_panel, "OpenBot");
        assert_eq!(snap0.nav_destination, "OpenBot");

        let snap_o = DualSurfaceFusionSnapshot::evaluate(Some("Cmd+Shift+O"), None, None);
        assert_eq!(snap_o.active_panel, "OpenBot");
        assert_eq!(snap_o.nav_destination, "OpenBot");

        for (idx, dest) in TELEGRAM_CMD_1_TO_9_DESTINATIONS.iter().enumerate() {
            let sc = format!("Cmd+{}", idx + 1);
            let s = DualSurfaceFusionSnapshot::evaluate(Some(&sc), None, None);
            assert_eq!(s.nav_destination, *dest);
        }

        let seq = vec![
            "Chats".to_string(),
            "OpenBot".to_string(),
            "Chats".to_string(),
        ];
        let snap_seq = DualSurfaceFusionSnapshot::evaluate(None, Some(&seq), None);
        assert_eq!(snap_seq.active_panel, "Chats");
    }

    #[std::prelude::v1::test]
    fn unified_avatar_spec_classifier_preserves_legacy_and_excludes_goutte() {
        assert_eq!(classify_unified_avatar_spec("classic"), "initials");
        assert_eq!(classify_unified_avatar_spec("unknown:foo"), "initials");
        for shape in LEGACY_GROK_AVATAR_SHAPES {
            assert_eq!(classify_unified_avatar_spec(shape), "grok_bot");
        }
        assert_eq!(classify_unified_avatar_spec("bloub:agent-1"), "bloub_flat");
        assert_eq!(classify_unified_avatar_spec("bloub:goutte"), "bloub_flat");
        assert_eq!(classify_unified_avatar_spec("plush:agent-1"), "bloub_plush");
        assert_ne!(resolve_goutte_fallback_shape(), "goutte");
    }

    #[std::prelude::v1::test]
    fn workspace_shell_routing_shortcuts_and_input_lifecycle() {
        let mut shell = OpenBotWorkspaceShell::new();
        assert_eq!(shell.active_page, OpenBotPage::Agents);
        assert_eq!(shell.active_target, ActiveOpenBotTarget::None);

        shell.select_agent("agent-alpha");
        assert_eq!(
            shell.active_target,
            ActiveOpenBotTarget::Agent("agent-alpha".to_string())
        );

        shell.handle_text_input("Hello @alpha");
        assert_eq!(shell.composer.active_trigger, Some(ComposerTrigger::At));
        shell.handle_escape();
        assert_eq!(shell.composer.active_trigger, None);

        shell.handle_enter(false);
        assert_eq!(shell.conversation.rows.len(), 1);
        assert!(shell.composer.text.is_empty());

        let toggle_sidebar = ShortcutChord::new(
            shell.platform != ServerPlatform::MacOs,
            false,
            false,
            shell.platform == ServerPlatform::MacOs,
            "KeyB",
        );
        assert!(shell.handle_shortcut(&toggle_sidebar));
        assert!(shell.shell.sidebar.closed);

        shell.sync_color_scheme(OpenBotColorScheme::Light);
        assert_eq!(shell.color_scheme, OpenBotColorScheme::Light);
        assert!(shell.theme_sweep.is_some());

        shell.set_reduced_motion(true);
        assert!(shell.theme_sweep.is_none());
        shell.sync_color_scheme(OpenBotColorScheme::Dark);
        assert!(shell.theme_sweep.is_none());

        shell.select_channel("chan-1");
        assert_eq!(
            shell.active_target,
            ActiveOpenBotTarget::Channel("chan-1".to_string())
        );
        assert!(shell.composer.in_channel);
    }

    #[std::prelude::v1::test]
    fn embedded_in_app_shell_defaults_false_and_renders_all_pages_in_both_modes() {
        let mut shell = OpenBotWorkspaceShell::new();
        assert!(!shell.embedded_in_app_shell);

        for page in OpenBotPage::ALL {
            shell.select_page(page);
            let _standalone_root = shell.render_workspace_root();
        }

        shell.set_embedded_in_app_shell(true);
        assert!(shell.embedded_in_app_shell);

        for page in OpenBotPage::ALL {
            shell.select_page(page);
            let _embedded_root = shell.render_workspace_root();
        }

        shell.set_embedded_in_app_shell(false);
        assert!(!shell.embedded_in_app_shell);
    }
}
