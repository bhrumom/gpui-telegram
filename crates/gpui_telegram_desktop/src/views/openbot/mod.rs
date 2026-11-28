//! Complete 1:1 OpenBot Desktop Workspace Views (`OB-F14`..`OB-F20`).
//!
//! Assembles all 19 OpenBot GPUI `0.2.2` UI modules, the root [`OpenBotWorkspaceShell`]
//! `Render` entity, and the deterministic 15-scenario headless [`evaluate_ui_probe`] engine:
//! - `OB-F14`: `shell_state`, `page_rail`, `server_rail`, `sidebar`, `agent_studio`
//! - `OB-F15`: `conversation`, `steps_and_trace`, `ask_ui`
//! - `OB-F16`: `composer`, `queue_and_voice`, `side_panel`
//! - `OB-F17`: `channel_view`, `channel_editor`
//! - `OB-F18`: `workflow_canvas`, `secondary_pages`, `settings_and_shortcuts`
//! - `OB-F19`: `os_surfaces` (all 7 Desktop OS surfaces)
//! - `OB-F20`: `workspace_shell`, `ui_probe` (dual-surface fusion & 15-scenario headless probe)

use std::path::Path;

pub mod agent_studio;
pub mod ask_ui;
pub mod channel_editor;
pub mod channel_view;
pub mod composer;
pub mod conversation;
pub mod os_surfaces;
pub mod page_rail;
pub mod queue_and_voice;
pub mod runtime_bridge;
pub mod secondary_pages;
pub mod server_rail;
pub mod settings_and_shortcuts;
pub mod shell_state;
pub mod side_panel;
pub mod sidebar;
pub mod steps_and_trace;
pub mod ui_probe;
pub mod workflow_canvas;
pub mod workflow_interactive;
pub mod workspace_shell;

pub use agent_studio::{AgentSettingsState, FirstAgentSetupState, GlobalSearchState};
pub use ask_ui::{RuntimeTurnCard, UiBlockCardState, UiBlockEvaluationResult, UiBlockValidationError};
pub use channel_editor::{ChannelEditorAgentOption, ChannelEditorState, ChannelSettingsPaneState};
pub use channel_view::{ChannelConversationState, ChannelRoutingEvent, ChannelStoppedTasksState};
pub use composer::{ComposerAttachmentDraft, ComposerState, ComposerTrigger};
pub use conversation::{ConversationViewState, StreamingBodyPacer, VoicePlayerState};
pub use os_surfaces::{
    DesktopOsSurfaceKind, DesktopOsSurfacesState, OsSurfaceValidationError, SURFACE_NAMES,
    evaluate_desktop_os_surfaces_probe,
};
pub use queue_and_voice::{QueuePanelState, VoicePillPhase, VoicePillState, VoiceTalkMode};
pub use runtime_bridge::{
    OPENBOT_SYNTHETIC_DIALOG_PREFIX, OpenBotRuntimeBridge, ProjectedTelegramAvatarProfile,
    ProjectedTelegramPeer, TelegramBidirectionalProjection, load_live_fusion_snapshot,
    load_openbot_host_analytics, load_telegram_bidirectional_projection, synthetic_telegram_peer_id,
};
pub use secondary_pages::{
    CalendarViewMode, CreditLedgerEntry, CrmPageState, LibraryPanelState, MarketplacePageState,
    ProviderUsageSeries, RoutineCalendarState, TasksPanelState, UsageAndCreditsState,
    UsageAndCreditsUiAction,
};
pub use settings_and_shortcuts::{
    AppSettingsState, AppSettingsTab, ServerSettingsSection, ServerSettingsState, ShortcutChord,
    ShortcutCommand, ShortcutRegistry,
};
pub use shell_state::{
    OpenBotPage, OpenBotShellState, PageRailState, ServerPlatform, ServerRailState,
    SidebarLayoutSnapshot, SidebarPinEntry, SidebarState, UsageRingMetric,
};
pub use side_panel::{SidePanelPaneKind, SidePanelState};
pub use sidebar::{SidebarAgentItem, SidebarChannelItem};
pub use steps_and_trace::{AgentStepsState, SubagentDockState, TraceWaterfallState};
pub use ui_probe::{
    ALL_UI_PROBE_SCENARIOS, LiveFusionSnapshot, UiProbeError, evaluate_ui_probe,
};
pub use workflow_canvas::WorkflowCanvasState;
pub use workflow_interactive::{
    OPENBOT_WORKFLOW_TEMPLATES, WorkflowUiAction, build_openbot_template_graph,
    hydrate_workflows_from_runtime,
};
pub use workspace_shell::{
    ActiveOpenBotTarget, DualSurfaceFusionSnapshot, LEGACY_GROK_AVATAR_SHAPES,
    OpenBotShellSummarySnapshot, OpenBotWorkspaceEvent, OpenBotWorkspaceShell,
    TELEGRAM_CMD_1_TO_9_DESTINATIONS, TELEGRAM_TDESKTOP_THEME_CYCLE, TelegramBaselineInvariants,
    classify_unified_avatar_spec, resolve_goutte_fallback_shape, resolve_workspace_shortcut,
};

/// Returns the canonical count of OpenBot Desktop OS surfaces (`7`: `Main`, `ChatWindow`,
/// `DynamicIsland`, `AgentDock`, `BrowserDock`, `CUA`, `Recovery`).
pub fn openbot_workspace_surface_count() -> usize {
    SURFACE_NAMES.len()
}

/// Constructs a fresh zero-starter-bot [`OpenBotWorkspaceShell`] for mounting inside `AppShell`.
pub fn new_openbot_workspace_shell() -> OpenBotWorkspaceShell {
    OpenBotWorkspaceShell::new()
}

/// Constructs an [`OpenBotWorkspaceShell`] hydrated from `<data_dir>/openbot-*-v1.json`.
pub fn new_openbot_workspace_shell_with_data_dir(data_dir: &Path) -> OpenBotWorkspaceShell {
    OpenBotWorkspaceShell::with_data_dir(data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openbot_facade_exposes_seven_surfaces_and_zero_starter_bots() {
        assert_eq!(openbot_workspace_surface_count(), 7);
        assert_eq!(ALL_UI_PROBE_SCENARIOS.len(), 15);
        let shell = new_openbot_workspace_shell();
        assert!(shell.agents.is_empty());
        assert!(shell.channels.is_empty());
        assert_eq!(shell.active_page, OpenBotPage::Agents);
        assert_eq!(shell.active_target, ActiveOpenBotTarget::None);
    }
}
