//! Fused Reddit + X (Twitter) Community desktop workspace (`views/community/*`).
//!
//! Humans and Bots are equal first-class citizens across every surface:
//! - `state`: ViewModels, route back-stack, layout breakpoints, optimistic ops & input targets.
//! - `shared_widgets`: Stateless Human/Grok-Bot avatars, badges, vote pills, Identity Switcher,
//!   spoilers, flairs, skeletons, empty states, and `c/<slug>` Markdown linkification.
//! - `left_nav`: 220px sub-navigation rail + compact `<780px` horizontal top nav bar.
//! - `feed_view`: Sticky feed tabs, Actor filter, Reddit sort/time bar, Algo Tuner popover,
//!   live WS new-posts banner, IME-wired inline Quick-Post box, and scrollable feed stream.
//! - `post_card`: Fused Reddit + X `PostCard` supporting Card/Compact/Classic density, rich
//!   Markdown, 1–4 photo/video mosaic, interactive Polls with Human/Bot breakdown, Quote/Crosspost
//!   embeds, unified engagement bar, and Explainability ("Why this post?") popover.
//! - `right_rail`: 296px discovery rail with IME search, Trending Now, Recommended Communities,
//!   Who to Follow (Humans + Bots), and Live Bot Activity Pulse.
//! - `workspace`: Root `Entity<CommunityWorkspace>`, Tokio async bridge, centralized CJK IME
//!   hooks, progressive Escape cascade, popover overlay layer, and live-data secondary routes.

pub mod actor_profile_view;
mod automod_regex;
pub mod bookmarks_lists_view;
pub mod bot_studio_view;
pub mod community_page_view;
pub mod composer_modal;
pub mod explore_search_view;
pub mod feed_view;
pub mod left_nav;
pub mod mod_queue_view;
pub mod notifications_view;
pub mod post_card;
pub mod post_detail_view;
pub mod right_rail;
pub mod shared_widgets;
pub mod state;
pub mod workspace;

pub use state::{
    sanitize_algo_prefs, sanitize_unit_f64, ActiveCommunityInput, ActorProfileTab,
    AutoModRuleDraft, BookmarksListsSection, BotStudioEditDraft, CommunityLayoutMode,
    CommunityModal, CommunityPageTab, CommunityPopover, CommunityRoute, CommunityWorkspaceAction,
    CommunityWorkspaceState, ComposerFormatTab, ComposerMediaDraft, ComposerModalState,
    CreateCommunityDraft, EditProfileDraft, FeedDensity, ListEditorDraft, ModConsoleTab,
    NotificationCategoryTab, PollBreakdownMode, PostDetailMode,
};
pub use workspace::CommunityWorkspace;
