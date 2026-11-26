//! Native GPUI `0.2.2` Orca desktop surfaces:
//! - `runtime`: Dual-mode `OrcaDesktopRuntime` (`OrcaBotsFacade` + `OrcaIdeFacade` + `OrcaClient`)
//! - `security_surface`: Non-themable host-controlled `SecuritySurfaceModal`
//! - `bots_*`: Orca Bot Operating System UI (Sidebar, Chat Tree, 4-tab Bot Home, Needs You, 10 Cards, 24 Templates, Group Rooms, Packages, E2EE Team Share, Cross-App Import, Hand-Over)

pub mod bots_cards;
pub mod bots_dialogs;
pub mod bots_home;
pub mod bots_state;
pub mod bots_workspace;
pub mod runtime;
pub mod security_surface;

pub use bots_cards::{
    card_kind_badge_label, placement_badge_label, render_bot_item_ask_card,
    render_native_bot_card, render_streaming_reply_preview, render_usage_limit_banner,
};
pub use bots_dialogs::render_active_bot_dialog;
pub use bots_home::{render_bot_home_panel, render_needs_you_inbox};
pub use bots_state::{
    BotChatMessage, BotDialogState, BotHomeTab, BotsInputTarget, OrcaBotsAction,
    OrcaBotsSurfaceState,
};
pub use bots_workspace::OrcaBotsWorkspace;
pub use runtime::{OrcaDesktopRuntime, OrcaRuntimeBindingMode, run_on_orca_tokio};
pub use security_surface::{
    SECURITY_SURFACE_APPROVE_BG, SECURITY_SURFACE_BACKDROP_RGBA, SECURITY_SURFACE_BADGE_BG,
    SECURITY_SURFACE_BADGE_FG, SECURITY_SURFACE_BG, SECURITY_SURFACE_BORDER,
    SECURITY_SURFACE_CARD_BG, SECURITY_SURFACE_DENY_BG, SECURITY_SURFACE_FG_MUTED,
    SECURITY_SURFACE_FG_PRIMARY, SECURITY_SURFACE_HEADER_BG, SECURITY_SURFACE_PALETTE,
    SecurityModalKind, SecuritySurfaceDecision, SecuritySurfaceModal, SecuritySurfaceModalState,
    SecuritySurfacePalette,
};
