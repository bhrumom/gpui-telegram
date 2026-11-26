//! Community workspace ViewModels, route stack, responsive breakpoints, and optimistic mutations.

use std::collections::{HashMap, HashSet};

use gpui_telegram_client::CommunityRuntimeMode;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorProfileResponse, AlgoPreferencesDto, AutoModActionKind, AutoModRuleDto,
    AwardResponse,
    BookmarkResponse, BotAutonomyMode, CommentSort, CommunityActorDto, CommunityCommentDto,
    CommunityDetailResponse, CommunityListDto, CommunityListKind, CommunityNotificationDto,
    CommunityNotificationKind, CommunityPostDto, CommunitySearchResultDto, CommunitySearchSort,
    CommunitySearchTab, FeedItemDto, FeedSort, FeedTab, FollowActionResponse,
    JoinCommunityResponse, ModLogEntryDto, ModQueueItemDto, ModQueueStatus, PollDto,
    PostDetailResponse, RepostResponse, SubCommunityDto, TimeWindow, ToggleActionResponse,
    TrendingTopicDto, VoteResponse,
};

use crate::views::composer::formatting_bar::TextStyleKind;

/// Active route inside the Community workspace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CommunityRoute {
    #[default]
    Feed,
    PostDetail(String),
    CommunityPage(String),
    ActorProfile(String),
    Explore {
        initial_query: Option<String>,
    },
    Notifications,
    BookmarksLists,
    ModQueue(Option<String>),
    BotStudio,
}

/// Cross-app actions emitted by [`super::CommunityWorkspace`] and handled by `AppShell`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommunityWorkspaceAction {
    OpenMediaLightbox(String),
    OpenDmWithActor { linked_user_id: i64 },
    SharePostToChat { post_id: String, summary: String },
    ShowToast { text: String, is_error: bool },
    UnreadCountChanged(u32),
    OpenExternalUrl(String),
}

/// Focused text input field inside Community for centralized English + CJK IME routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveCommunityInput {
    #[default]
    None,
    QuickPostTitle,
    QuickPostBody,
    RightRailSearch,
    ExploreSearch,
    DetailReply,
    QuotePostNote,
    BotStudioName,
    BotStudioHandle,
    BotStudioPersona,
    // M5 F17 Post Detail inputs
    DetailEditComment,
    DetailSummonPrompt,
    // M5 F18 Rich Composer Modal inputs
    ComposerTitle,
    ComposerBody,
    ComposerLinkUrl,
    ComposerLinkTitle,
    ComposerLinkDesc,
    ComposerMediaUrl(usize),
    ComposerMediaAlt(usize),
    ComposerPollQuestion,
    ComposerPollOption(usize),
    ComposerThreadPart(usize),
    ComposerScheduleMinutes,
    ComposerBotPrompt,
    // M5 F19 Community Page & Create Community inputs
    CommunityWikiMarkdown,
    CommunityWikiSummary,
    CommunityNewRuleTitle,
    CommunityNewRuleDesc,
    CreateCommSlug,
    CreateCommName,
    CreateCommDescription,
    CreateCommIconEmoji,
    // M5 F20 Actor Profile Edit inputs
    ProfileEditDisplayName,
    ProfileEditBio,
    ProfileEditLocation,
    ProfileEditWebsite,
    ProfileEditBotBadge,
    ProfileEditBotModel,
    ProfileEditBotPersona,
    // M5 F20 Bookmarks & Lists inputs
    BookmarkNewFolder,
    ListEditorName,
    ListEditorDesc,
    // M5 F21 Moderation Console & AutoMod inputs
    ModReason,
    ModFlair,
    AutoModRuleName,
    AutoModRuleKeywords,
    AutoModRuleRegex,
    AutoModRuleMinKarma,
    AutoModRuleMaxLinks,
    AutoModRuleFlair,
    AutoModRuleReply,
    AutoModDryRunTitle,
    AutoModDryRunBody,
    // M5 F21 Bot Citizenship Studio inputs
    BotStudioBio,
    BotStudioBadge,
    BotStudioModel,
    BotStudioCustomCap,
    BotStudioCooldown,
    BotStudioMaxPostsPerHour,
    BotStudioEditName,
    BotStudioEditBio,
    BotStudioEditBadge,
    BotStudioEditModel,
    BotStudioEditCustomCap,
    BotStudioEditPersona,
    BotStudioTestPostBody,
}

impl ActiveCommunityInput {
    pub fn allows_newlines(self) -> bool {
        matches!(
            self,
            Self::QuickPostBody
                | Self::DetailReply
                | Self::QuotePostNote
                | Self::BotStudioPersona
                | Self::DetailEditComment
                | Self::ComposerBody
                | Self::ComposerThreadPart(_)
                | Self::CommunityWikiMarkdown
                | Self::CommunityNewRuleDesc
                | Self::CreateCommDescription
                | Self::ProfileEditBio
                | Self::ProfileEditBotPersona
                | Self::ListEditorDesc
                | Self::AutoModRuleReply
                | Self::AutoModDryRunBody
                | Self::BotStudioBio
                | Self::BotStudioEditBio
                | Self::BotStudioEditPersona
                | Self::BotStudioTestPostBody
        )
    }

    pub fn is_composer_input(self) -> bool {
        matches!(
            self,
            Self::ComposerTitle
                | Self::ComposerBody
                | Self::ComposerLinkUrl
                | Self::ComposerLinkTitle
                | Self::ComposerLinkDesc
                | Self::ComposerMediaUrl(_)
                | Self::ComposerMediaAlt(_)
                | Self::ComposerPollQuestion
                | Self::ComposerPollOption(_)
                | Self::ComposerThreadPart(_)
                | Self::ComposerScheduleMinutes
                | Self::ComposerBotPrompt
        )
    }

    pub fn is_create_comm_input(self) -> bool {
        matches!(
            self,
            Self::CreateCommSlug
                | Self::CreateCommName
                | Self::CreateCommDescription
                | Self::CreateCommIconEmoji
        )
    }

    pub fn is_profile_edit_input(self) -> bool {
        matches!(
            self,
            Self::ProfileEditDisplayName
                | Self::ProfileEditBio
                | Self::ProfileEditLocation
                | Self::ProfileEditWebsite
                | Self::ProfileEditBotBadge
                | Self::ProfileEditBotModel
                | Self::ProfileEditBotPersona
        )
    }

    pub fn is_list_editor_input(self) -> bool {
        matches!(self, Self::ListEditorName | Self::ListEditorDesc)
    }

    pub fn is_automod_input(self) -> bool {
        matches!(
            self,
            Self::AutoModRuleName
                | Self::AutoModRuleKeywords
                | Self::AutoModRuleRegex
                | Self::AutoModRuleMinKarma
                | Self::AutoModRuleMaxLinks
                | Self::AutoModRuleFlair
                | Self::AutoModRuleReply
        )
    }

    pub fn is_bot_studio_create_input(self) -> bool {
        matches!(
            self,
            Self::BotStudioName
                | Self::BotStudioHandle
                | Self::BotStudioPersona
                | Self::BotStudioBio
                | Self::BotStudioBadge
                | Self::BotStudioModel
                | Self::BotStudioCustomCap
                | Self::BotStudioCooldown
                | Self::BotStudioMaxPostsPerHour
        )
    }

    pub fn is_bot_studio_edit_input(self) -> bool {
        matches!(
            self,
            Self::BotStudioEditName
                | Self::BotStudioEditBio
                | Self::BotStudioEditBadge
                | Self::BotStudioEditModel
                | Self::BotStudioEditCustomCap
                | Self::BotStudioEditPersona
        )
    }
}

/// Visual density modes for feed post cards (Reddit Card / Compact / Classic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeedDensity {
    #[default]
    Card,
    Compact,
    Classic,
}

/// Poll vote breakdown display filter (`All | Humans | Bots`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PollBreakdownMode {
    #[default]
    All,
    Humans,
    Bots,
}

/// Adaptive 3/2/1-column layout mode resolved from window width & host sidebar width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommunityLayoutMode {
    #[default]
    ThreeColumn,
    TwoColumn,
    OneColumn,
}

impl CommunityLayoutMode {
    /// Resolves layout mode from standalone community pane width (`>=1080` 3-col, `>=780` 2-col, else 1-col).
    pub fn for_pane_width(pane_width_px: f32) -> Self {
        if pane_width_px >= 1080.0 {
            Self::ThreeColumn
        } else if pane_width_px >= 780.0 {
            Self::TwoColumn
        } else {
            Self::OneColumn
        }
    }

    /// Resolves layout mode inside `AppShell` accounting for expanded (`288px`) or collapsed (`64px`) sidebar.
    ///
    /// - Standalone (`host_sidebar_width_px <= 0.0`) delegates directly to [`Self::for_pane_width`].
    /// - `1440×900` (expanded or collapsed) -> `ThreeColumn`
    /// - `1040×800` (expanded or collapsed) -> `TwoColumn`
    /// - `820×720` / `640×480` -> `OneColumn`
    pub fn resolve(window_width_px: f32, host_sidebar_width_px: f32) -> Self {
        if host_sidebar_width_px <= 0.0 {
            return Self::for_pane_width(window_width_px);
        }
        let pane_w = (window_width_px - host_sidebar_width_px).max(0.0);
        if pane_w >= 1080.0 {
            Self::ThreeColumn
        } else if pane_w >= 720.0 && window_width_px >= 900.0 {
            Self::TwoColumn
        } else {
            Self::OneColumn
        }
    }
}

/// Clamps a unit-interval floating-point weight into `[0.0, 1.0]`, replacing `NaN`/`±Inf` with `default`.
pub fn sanitize_unit_f64(v: f64, default: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else if default.is_finite() {
        default.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

/// Returns a finite, `[0.0, 1.0]`-clamped copy of [`AlgoPreferencesDto`] using canonical defaults
/// for any `NaN` or infinite fields.
pub fn sanitize_algo_prefs(prefs: &AlgoPreferencesDto) -> AlgoPreferencesDto {
    let defs = AlgoPreferencesDto::default();
    AlgoPreferencesDto {
        preset: prefs.preset,
        recency_weight: sanitize_unit_f64(prefs.recency_weight, defs.recency_weight),
        affinity_weight: sanitize_unit_f64(prefs.affinity_weight, defs.affinity_weight),
        quality_weight: sanitize_unit_f64(prefs.quality_weight, defs.quality_weight),
        discovery_ratio: sanitize_unit_f64(prefs.discovery_ratio, defs.discovery_ratio),
        bot_content_ratio: sanitize_unit_f64(prefs.bot_content_ratio, defs.bot_content_ratio),
        show_explanations: prefs.show_explanations,
        allow_nsfw: prefs.allow_nsfw,
    }
}

/// Active floating popover overlay rendered on top of the Community scroll containers & modals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommunityPopover {
    IdentitySwitcher,
    AlgoTuner,
    ExplainPost(String),
    AwardPost(String),
    QuickPostCommunityPicker,
    QuotePost(String),
    AwardComment(String),
    SummonBotPicker(String),
}

/// Active centered `macos_modal_card` sheet overlay inside the Community workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityModal {
    Composer,
    CreateCommunity,
    EditProfile,
}

/// Dual-mode display toggle inside `PostDetail` (F17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PostDetailMode {
    #[default]
    RedditTree,
    XThread,
}

/// Multi-format post creation tab inside `ComposerModal` (F18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComposerFormatTab {
    #[default]
    Micro,
    Article,
    Media,
    Link,
    Poll,
    Thread,
    Crosspost,
    Quote,
}

/// Single media attachment draft entry inside `ComposerModal` (1..=4 items).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerMediaDraft {
    pub url_or_id: String,
    pub kind: String,
    pub alt_text: String,
}

impl Default for ComposerMediaDraft {
    fn default() -> Self {
        Self {
            url_or_id: String::new(),
            kind: "image".to_string(),
            alt_text: String::new(),
        }
    }
}

/// Complete ViewModel state for the full rich `ComposerModal` (F18).
#[derive(Debug, Clone)]
pub struct ComposerModalState {
    pub editing_post_id: Option<String>,
    pub tab: ComposerFormatTab,
    pub show_drafts_drawer: bool,
    pub show_markdown_preview: bool,
    pub community_slug: Option<String>,
    pub flair: Option<String>,
    pub title: String,
    pub body: String,
    pub media_items: Vec<ComposerMediaDraft>,
    pub link_url: String,
    pub link_title: String,
    pub link_description: String,
    pub poll_question: String,
    pub poll_options: Vec<String>,
    pub poll_multi_select: bool,
    pub poll_duration_secs: i64,
    pub thread_parts: Vec<String>,
    pub quoted_post_id: Option<String>,
    pub crosspost_of_id: Option<String>,
    pub origin_preview: Option<CommunityPostDto>,
    pub is_nsfw: bool,
    pub is_spoiler: bool,
    pub schedule_enabled: bool,
    pub schedule_offset_mins: i64,
    pub schedule_minutes_input: String,
    pub co_author_bot_id: Option<String>,
    pub bot_prompt: String,
    pub bot_coauthor_suggestion: Option<String>,
    pub autocomplete_dismissed: bool,
    pub active_formats: Vec<TextStyleKind>,
    pub submitting: bool,
    pub validation_error: Option<String>,
    pub drafts: Vec<CommunityPostDto>,
    pub drafts_loading: bool,
    pub drafts_gen: u64,
}

impl Default for ComposerModalState {
    fn default() -> Self {
        Self {
            editing_post_id: None,
            tab: ComposerFormatTab::Micro,
            show_drafts_drawer: false,
            show_markdown_preview: false,
            community_slug: None,
            flair: None,
            title: String::new(),
            body: String::new(),
            media_items: vec![ComposerMediaDraft::default()],
            link_url: String::new(),
            link_title: String::new(),
            link_description: String::new(),
            poll_question: String::new(),
            poll_options: vec![String::new(), String::new()],
            poll_multi_select: false,
            poll_duration_secs: 86_400,
            thread_parts: vec![String::new()],
            quoted_post_id: None,
            crosspost_of_id: None,
            origin_preview: None,
            is_nsfw: false,
            is_spoiler: false,
            schedule_enabled: false,
            schedule_offset_mins: 60,
            schedule_minutes_input: "60".to_string(),
            co_author_bot_id: None,
            bot_prompt: String::new(),
            bot_coauthor_suggestion: None,
            autocomplete_dismissed: false,
            active_formats: Vec::new(),
            submitting: false,
            validation_error: None,
            drafts: Vec::new(),
            drafts_loading: false,
            drafts_gen: 0,
        }
    }
}

/// Active sub-tab on a `c/<slug>` Community page (F19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommunityPageTab {
    #[default]
    Posts,
    Rules,
    Wiki,
    Mods,
}

/// Form draft state for the "Create Community" modal (F19).
#[derive(Debug, Clone)]
pub struct CreateCommunityDraft {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub icon_emoji: String,
    pub accent_color: String,
    pub allow_bots: bool,
    pub require_bot_disclosure: bool,
    pub is_nsfw: bool,
    pub submitting: bool,
    pub error: Option<String>,
}

impl Default for CreateCommunityDraft {
    fn default() -> Self {
        Self {
            slug: String::new(),
            name: String::new(),
            description: String::new(),
            category: "tech".to_string(),
            icon_emoji: "🚀".to_string(),
            accent_color: "#3B82F6".to_string(),
            allow_bots: true,
            require_bot_disclosure: true,
            is_nsfw: false,
            submitting: false,
            error: None,
        }
    }
}

/// Active sub-tab on a Human/Bot `@handle` Profile page (F20-Profile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActorProfileTab {
    #[default]
    Posts,
    Replies,
    Threads,
    Media,
    Likes,
    Upvoted,
    Communities,
    BotActivity,
}

/// Form draft state for editing an owned Human or Bot profile (F20-Profile).
#[derive(Debug, Clone, Default)]
pub struct EditProfileDraft {
    pub actor_id: String,
    pub is_bot: bool,
    pub display_name: String,
    pub bio: String,
    pub location: String,
    pub website: String,
    pub avatar_shape: String,
    pub avatar_color: String,
    pub bot_badge: String,
    pub bot_model: String,
    pub bot_autonomy_mode: BotAutonomyMode,
    pub bot_persona: String,
    pub submitting: bool,
    pub error: Option<String>,
}

/// Notification category filter tabs mapping exhaustively across all `CommunityNotificationKind` variants (F20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationCategoryTab {
    #[default]
    All,
    Mentions,
    Replies,
    VotesLikes,
    RepostsQuotes,
    Follows,
    BotActivity,
    Moderation,
}

impl NotificationCategoryTab {
    pub const ALL_TABS: [Self; 8] = [
        Self::All,
        Self::Mentions,
        Self::Replies,
        Self::VotesLikes,
        Self::RepostsQuotes,
        Self::Follows,
        Self::BotActivity,
        Self::Moderation,
    ];

    pub fn matches(self, kind: CommunityNotificationKind) -> bool {
        match self {
            Self::All => true,
            Self::Mentions => matches!(kind, CommunityNotificationKind::Mention),
            Self::Replies => matches!(kind, CommunityNotificationKind::Reply),
            Self::VotesLikes => matches!(
                kind,
                CommunityNotificationKind::Like
                    | CommunityNotificationKind::UpvoteMilestone
                    | CommunityNotificationKind::Award
            ),
            Self::RepostsQuotes => matches!(
                kind,
                CommunityNotificationKind::Repost | CommunityNotificationKind::Quote
            ),
            Self::Follows => matches!(kind, CommunityNotificationKind::Follow),
            Self::BotActivity => matches!(
                kind,
                CommunityNotificationKind::BotSummon | CommunityNotificationKind::BotReply
            ),
            Self::Moderation => matches!(
                kind,
                CommunityNotificationKind::ModAction
                    | CommunityNotificationKind::PollEnded
                    | CommunityNotificationKind::System
            ),
        }
    }
}

/// Top-level section switcher inside `BookmarksLists` (F20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BookmarksListsSection {
    #[default]
    Bookmarks,
    Lists,
}

/// Create/Edit draft state for X Lists & Reddit Custom Feeds (F20).
#[derive(Debug, Clone)]
pub struct ListEditorDraft {
    pub editing_list_id: Option<String>,
    pub name: String,
    pub description: String,
    pub kind: CommunityListKind,
    pub is_private: bool,
    pub is_pinned: bool,
    pub member_actor_ids: Vec<String>,
    pub community_slugs: Vec<String>,
    pub submitting: bool,
    pub error: Option<String>,
}

impl Default for ListEditorDraft {
    fn default() -> Self {
        Self {
            editing_list_id: None,
            name: String::new(),
            description: String::new(),
            kind: CommunityListKind::Mixed,
            is_private: false,
            is_pinned: false,
            member_actor_ids: Vec::new(),
            community_slugs: Vec::new(),
            submitting: false,
            error: None,
        }
    }
}

/// Sub-tab switcher inside the Community Moderation Console (F21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModConsoleTab {
    #[default]
    Queue,
    AutoModRules,
    ModLog,
}

/// Declarative AutoMod rule creation draft state (F21).
#[derive(Debug, Clone)]
pub struct AutoModRuleDraft {
    pub name: String,
    pub keywords: String,
    pub regex_pattern: String,
    pub min_karma: String,
    pub max_links: String,
    pub action: AutoModActionKind,
    pub assign_flair: String,
    pub auto_reply_template: String,
    pub apply_to_bots: bool,
    pub apply_to_humans: bool,
    pub saving: bool,
    pub error: Option<String>,
}

impl Default for AutoModRuleDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            keywords: String::new(),
            regex_pattern: String::new(),
            min_karma: String::new(),
            max_links: String::new(),
            action: AutoModActionKind::FlagForReview,
            assign_flair: String::new(),
            auto_reply_template: String::new(),
            apply_to_bots: true,
            apply_to_humans: true,
            saving: false,
            error: None,
        }
    }
}

/// Edit draft state for modifying an existing owned Bot inside Bot Citizenship Studio (F21).
#[derive(Debug, Clone, Default)]
pub struct BotStudioEditDraft {
    pub actor_id: String,
    pub display_name: String,
    pub bio: String,
    pub bot_badge: String,
    pub bot_model: String,
    pub bot_capabilities: Vec<String>,
    pub custom_cap_draft: String,
    pub bot_persona: String,
    pub bot_autonomy_mode: BotAutonomyMode,
    pub shape_idx: usize,
    pub color_idx: usize,
    pub shape_edited: bool,
    pub color_edited: bool,
    pub autonomy_edited: bool,
    pub saving: bool,
    pub error: Option<String>,
}

/// Normalizes bookmark folder names to match SQLite/server semantics
/// (`""`, `"Unsorted"`, and `"All"` all map to `"All"`).
pub fn normalize_bookmark_folder(folder: &str) -> String {
    let trimmed = folder.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("Unsorted")
        || trimmed.eq_ignore_ascii_case("All")
    {
        "All".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Last confirmed SQLite/server bookmark state per `post_id` for rollback on terminal error.
#[derive(Debug, Clone)]
pub struct BookmarkRollbackSnapshot {
    pub bookmarked: bool,
    pub folder: Option<String>,
    pub bookmarks_count: u64,
    pub saved_item: Option<FeedItemDto>,
}

/// Post-settlement override recorded ONLY while `saved_loading == true`, preventing a
/// stale `load_saved_posts` (whose `FeedTab::Saved` read ran before `bookmark_post`
/// committed, but whose `list_lists()` finished after `bookmark_in_flight` cleared)
/// from clobbering `saved_items`.
#[derive(Debug, Clone)]
pub struct BookmarkSavedOverride {
    pub epoch: u64,
    pub bookmarked: bool,
    pub folder: Option<String>,
    pub bookmarks_count: Option<u64>,
    pub item: Option<FeedItemDto>,
}

/// Complete client-side ViewModel for [`super::CommunityWorkspace`].
pub struct CommunityWorkspaceState {
    pub route: CommunityRoute,
    pub route_stack: Vec<CommunityRoute>,

    // Identities & runtime status
    pub identities: Option<gpui_telegram_proto::community_contracts::CommunityIdentitiesResponse>,
    pub active_actor_id: String,
    pub known_owned_actor_ids: HashSet<String>,
    pub runtime_mode: CommunityRuntimeMode,
    pub identities_gen: u64,
    pub actor_switch_gen: u64,

    // Feed query & stream
    pub active_tab: FeedTab,
    pub sort: FeedSort,
    pub time_window: TimeWindow,
    pub actor_filter: ActorFilter,
    pub active_flair: Option<String>,
    pub density: FeedDensity,
    pub algo_prefs: AlgoPreferencesDto,
    pub algo_draft: AlgoPreferencesDto,
    pub algo_prefs_gen: u64,
    pub feed_items: Vec<FeedItemDto>,
    pub next_cursor: Option<String>,
    pub feed_loading: bool,
    pub feed_loading_more: bool,
    pub feed_error: Option<String>,
    pub pending_ws_new_posts: u32,
    pub feed_gen: u64,

    // Quick-Post & IME input buffers
    pub quick_post_title: String,
    pub quick_post_body: String,
    pub quick_post_community: Option<String>,
    pub quick_post_expanded: bool,
    pub quick_post_submitting: bool,
    pub quick_post_validation_error: Option<String>,
    pub right_rail_search_draft: String,
    pub explore_search_draft: String,
    pub detail_reply_draft: String,
    pub detail_reply_parent_id: Option<String>,
    pub quote_post_draft: String,
    pub active_input: ActiveCommunityInput,
    pub ime_preedit: Option<String>,
    pub input_generation: u64,

    // Card interactive states & modal sheets
    pub active_popover: Option<CommunityPopover>,
    pub active_modal: Option<CommunityModal>,
    pub card_child_click_consumed: bool,
    pub revealed_spoilers: HashSet<(u64, usize)>,
    pub revealed_post_veils: HashSet<String>,
    pub expanded_quotes: HashSet<(u64, usize)>,
    pub copied_code_msg: Option<u64>,
    pub poll_breakdown_by_post: HashMap<String, PollBreakdownMode>,
    pub hidden_post_ids: HashSet<String>,
    pub post_vote_gen: HashMap<String, u64>,
    pub post_like_flight_seq: u64,
    pub post_like_in_flight: HashMap<String, u64>,
    pub post_like_pending_toggle: HashSet<String>,
    pub post_like_last_synced: HashMap<String, (bool, u64)>,
    pub comment_vote_gen: HashMap<String, u64>,
    pub comment_like_flight_seq: u64,
    pub comment_like_in_flight: HashMap<String, u64>,
    pub comment_like_pending_toggle: HashSet<String>,
    pub comment_like_last_synced: HashMap<String, (bool, u64)>,

    // Left Nav & Right Rail caches
    pub communities: Vec<SubCommunityDto>,
    pub communities_gen: u64,
    pub lists: Vec<CommunityListDto>,
    pub lists_gen: u64,
    pub trending: Vec<TrendingTopicDto>,
    pub recommended_actors: Vec<CommunityActorDto>,
    pub unread_notifications: u32,

    // Secondary route live caches & M5 interactive states
    pub detail_post: Option<PostDetailResponse>,
    pub detail_comment_sort: CommentSort,
    pub detail_mode: PostDetailMode,
    pub detail_collapsed_comments: HashSet<String>,
    pub detail_editing_comment_id: Option<String>,
    pub detail_edit_comment_draft: String,
    pub detail_loading_more_comment_ids: HashSet<String>,
    pub detail_comment_more_cursors: HashMap<String, String>,
    pub detail_summon_bot_id: Option<String>,
    pub detail_summon_prompt: String,
    pub detail_summon_pending: bool,
    pub detail_summon_feedback: Option<(String, bool)>,
    pub detail_focused_comment_id: Option<String>,
    pub detail_opened_from_notification: bool,
    pub detail_loading: bool,
    pub detail_error: Option<String>,
    pub detail_gen: u64,

    // F18 Rich Multi-Format Composer modal state
    pub composer: ComposerModalState,

    // F19 Community Page & Create Community modal state
    pub community_detail: Option<CommunityDetailResponse>,
    pub community_posts: Vec<FeedItemDto>,
    pub community_tab: CommunityPageTab,
    pub community_selected_flair: Option<String>,
    pub community_sort: FeedSort,
    pub community_time_window: TimeWindow,
    pub community_actor_filter: ActorFilter,
    pub community_next_cursor: Option<String>,
    pub community_loading_more: bool,
    pub community_bell_enabled: HashMap<String, bool>,
    pub community_wiki_editing: bool,
    pub community_wiki_draft: String,
    pub community_wiki_summary: String,
    pub community_wiki_preview_rev: Option<u32>,
    pub community_wiki_saving: bool,
    pub community_new_rule_title: String,
    pub community_new_rule_desc: String,
    pub community_rules_saving: bool,
    pub create_community: CreateCommunityDraft,
    pub community_loading: bool,
    pub community_error: Option<String>,
    pub community_gen: u64,

    // F20 Actor Profile & Edit Profile modal state
    pub actor_profile: Option<ActorProfileResponse>,
    pub profile_tab: ActorProfileTab,
    pub edit_profile: EditProfileDraft,
    pub profile_loading: bool,
    pub profile_error: Option<String>,
    pub profile_gen: u64,
    pub actor_mute_flight_seq: u64,
    pub actor_mute_in_flight: HashMap<String, u64>,
    pub actor_mute_dirty: HashSet<String>,
    pub actor_mute_pending_target: HashMap<String, bool>,
    pub actor_mute_last_synced: HashMap<String, bool>,
    pub actor_block_flight_seq: u64,
    pub actor_block_in_flight: HashMap<String, u64>,
    pub actor_block_dirty: HashSet<String>,
    pub actor_block_pending_target: HashMap<String, bool>,
    pub actor_block_last_synced: HashMap<String, (bool, bool)>,

    // F21 Explore & Search state
    pub search_result: Option<CommunitySearchResultDto>,
    pub search_tab: CommunitySearchTab,
    pub search_sort: CommunitySearchSort,
    pub search_category_filter: Option<String>,
    pub explore_show_discovery: bool,
    pub search_error: Option<String>,
    pub search_loading: bool,
    pub search_gen: u64,

    // F20 Notifications state
    pub notifications: Vec<CommunityNotificationDto>,
    pub notif_category: NotificationCategoryTab,
    pub notif_unread_only: bool,
    pub notifications_loading: bool,
    pub notifications_gen: u64,

    // F20 Bookmarks & Lists state
    pub saved_items: Vec<FeedItemDto>,
    pub bookmarks_section: BookmarksListsSection,
    pub bookmark_selected_folder: String,
    pub bookmark_custom_folders: Vec<String>,
    pub bookmark_new_folder_draft: String,
    pub bookmark_moving_post_id: Option<String>,
    pub bookmark_state_epoch: u64,
    pub bookmark_flight_seq: u64,
    pub bookmark_in_flight: HashMap<String, u64>,
    pub bookmark_dirty: HashSet<String>,
    pub bookmark_pending_target: HashMap<String, (bool, Option<String>)>,
    pub bookmark_last_synced: HashMap<String, BookmarkRollbackSnapshot>,
    pub bookmark_cached_items: HashMap<String, FeedItemDto>,
    pub bookmark_recent_overrides: HashMap<String, BookmarkSavedOverride>,
    pub selected_list_id: Option<String>,
    pub list_feed_items: Vec<FeedItemDto>,
    pub list_feed_sort: FeedSort,
    pub list_feed_actor_filter: ActorFilter,
    pub list_feed_loading: bool,
    pub list_feed_gen: u64,
    pub list_membership_gen: u64,
    pub list_membership_flight_seq: u64,
    pub list_membership_in_flight: HashMap<String, u64>,
    pub list_membership_dirty: HashSet<String>,
    pub list_membership_needs_feed_reload: HashSet<String>,
    pub list_membership_last_synced: HashMap<String, (bool, Vec<String>, Vec<String>)>,
    pub list_deleting: HashSet<String>,
    pub list_editor_open: bool,
    pub list_editor: ListEditorDraft,
    pub saved_loading: bool,
    pub saved_gen: u64,

    // F21 Moderation Console state
    pub mod_slug: Option<String>,
    pub mod_tab: ModConsoleTab,
    pub mod_status_filter: Option<ModQueueStatus>,
    pub mod_reason_draft: String,
    pub mod_flair_draft: String,
    pub mod_community: Option<SubCommunityDto>,
    pub mod_queue: Vec<ModQueueItemDto>,
    pub mod_logs: Vec<ModLogEntryDto>,
    pub mod_error: Option<String>,
    pub automod_draft: AutoModRuleDraft,
    pub automod_dry_run_title: String,
    pub automod_dry_run_body: String,
    pub automod_dry_run_is_bot: bool,
    pub automod_dry_run_result: Option<gpui_telegram_community::AutoModDecision>,
    pub automod_rules_gen: u64,
    pub automod_flight_seq: u64,
    pub automod_in_flight: HashMap<String, u64>,
    pub automod_dirty: HashSet<String>,
    pub automod_reset_draft_pending: HashSet<String>,
    pub automod_last_synced: HashMap<String, Vec<AutoModRuleDto>>,
    pub mod_loading: bool,
    pub mod_gen: u64,

    // F21 Bot Citizenship Studio state (preserving M4 fields)
    pub bot_studio_name: String,
    pub bot_studio_handle: String,
    pub bot_studio_persona: String,
    pub bot_studio_bio: String,
    pub bot_studio_badge: String,
    pub bot_studio_model: String,
    pub bot_studio_capabilities: Vec<String>,
    pub bot_studio_custom_cap_draft: String,
    pub bot_studio_cooldown_secs: String,
    pub bot_studio_max_posts_per_hour: String,
    pub bot_studio_shape_idx: usize,
    pub bot_studio_color_idx: usize,
    pub bot_studio_autonomy: BotAutonomyMode,
    pub bot_studio_submitting: bool,
    pub bot_studio_last_token: Option<String>,
    pub bot_studio_tokens_by_actor: HashMap<String, String>,
    pub bot_studio_token_revealed: bool,
    pub bot_studio_selected_bot_id: Option<String>,
    pub bot_studio_edit: BotStudioEditDraft,
    pub bot_studio_test_community: Option<String>,
    pub bot_studio_test_content: String,
    pub bot_studio_test_submitting: bool,
    pub bot_studio_test_last_post_id: Option<String>,
    pub bot_studio_test_feedback: Option<(String, bool)>,
    pub bot_studio_error: Option<String>,
}

impl Default for CommunityWorkspaceState {
    fn default() -> Self {
        let algo_prefs = AlgoPreferencesDto::default();
        Self {
            route: CommunityRoute::Feed,
            route_stack: Vec::new(),
            identities: None,
            active_actor_id: String::new(),
            known_owned_actor_ids: HashSet::new(),
            runtime_mode: CommunityRuntimeMode::LocalFallback,
            identities_gen: 0,
            actor_switch_gen: 0,
            active_tab: FeedTab::ForYou,
            sort: FeedSort::Algo,
            time_window: TimeWindow::All,
            actor_filter: ActorFilter::AllCitizens,
            active_flair: None,
            density: FeedDensity::Card,
            algo_draft: algo_prefs.clone(),
            algo_prefs,
            algo_prefs_gen: 0,
            feed_items: Vec::new(),
            next_cursor: None,
            feed_loading: true,
            feed_loading_more: false,
            feed_error: None,
            pending_ws_new_posts: 0,
            feed_gen: 0,
            quick_post_title: String::new(),
            quick_post_body: String::new(),
            quick_post_community: None,
            quick_post_expanded: false,
            quick_post_submitting: false,
            quick_post_validation_error: None,
            right_rail_search_draft: String::new(),
            explore_search_draft: String::new(),
            detail_reply_draft: String::new(),
            detail_reply_parent_id: None,
            quote_post_draft: String::new(),
            active_input: ActiveCommunityInput::None,
            ime_preedit: None,
            input_generation: 0,
            active_popover: None,
            active_modal: None,
            card_child_click_consumed: false,
            revealed_spoilers: HashSet::new(),
            revealed_post_veils: HashSet::new(),
            expanded_quotes: HashSet::new(),
            copied_code_msg: None,
            poll_breakdown_by_post: HashMap::new(),
            hidden_post_ids: HashSet::new(),
            post_vote_gen: HashMap::new(),
            post_like_flight_seq: 0,
            post_like_in_flight: HashMap::new(),
            post_like_pending_toggle: HashSet::new(),
            post_like_last_synced: HashMap::new(),
            comment_vote_gen: HashMap::new(),
            comment_like_flight_seq: 0,
            comment_like_in_flight: HashMap::new(),
            comment_like_pending_toggle: HashSet::new(),
            comment_like_last_synced: HashMap::new(),
            communities: Vec::new(),
            communities_gen: 0,
            lists: Vec::new(),
            lists_gen: 0,
            trending: Vec::new(),
            recommended_actors: Vec::new(),
            unread_notifications: 0,
            detail_post: None,
            detail_comment_sort: CommentSort::Best,
            detail_mode: PostDetailMode::RedditTree,
            detail_collapsed_comments: HashSet::new(),
            detail_editing_comment_id: None,
            detail_edit_comment_draft: String::new(),
            detail_loading_more_comment_ids: HashSet::new(),
            detail_comment_more_cursors: HashMap::new(),
            detail_summon_bot_id: None,
            detail_summon_prompt: String::new(),
            detail_summon_pending: false,
            detail_summon_feedback: None,
            detail_focused_comment_id: None,
            detail_opened_from_notification: false,
            detail_loading: false,
            detail_error: None,
            detail_gen: 0,
            composer: ComposerModalState::default(),
            community_detail: None,
            community_posts: Vec::new(),
            community_tab: CommunityPageTab::Posts,
            community_selected_flair: None,
            community_sort: FeedSort::Hot,
            community_time_window: TimeWindow::All,
            community_actor_filter: ActorFilter::AllCitizens,
            community_next_cursor: None,
            community_loading_more: false,
            community_bell_enabled: HashMap::new(),
            community_wiki_editing: false,
            community_wiki_draft: String::new(),
            community_wiki_summary: String::new(),
            community_wiki_preview_rev: None,
            community_wiki_saving: false,
            community_new_rule_title: String::new(),
            community_new_rule_desc: String::new(),
            community_rules_saving: false,
            create_community: CreateCommunityDraft::default(),
            community_loading: false,
            community_error: None,
            community_gen: 0,
            actor_profile: None,
            profile_tab: ActorProfileTab::Posts,
            edit_profile: EditProfileDraft::default(),
            profile_loading: false,
            profile_error: None,
            profile_gen: 0,
            actor_mute_flight_seq: 0,
            actor_mute_in_flight: HashMap::new(),
            actor_mute_dirty: HashSet::new(),
            actor_mute_pending_target: HashMap::new(),
            actor_mute_last_synced: HashMap::new(),
            actor_block_flight_seq: 0,
            actor_block_in_flight: HashMap::new(),
            actor_block_dirty: HashSet::new(),
            actor_block_pending_target: HashMap::new(),
            actor_block_last_synced: HashMap::new(),
            search_result: None,
            search_tab: CommunitySearchTab::Top,
            search_sort: CommunitySearchSort::Relevance,
            search_category_filter: None,
            explore_show_discovery: true,
            search_error: None,
            search_loading: false,
            search_gen: 0,
            notifications: Vec::new(),
            notif_category: NotificationCategoryTab::All,
            notif_unread_only: false,
            notifications_loading: false,
            notifications_gen: 0,
            saved_items: Vec::new(),
            bookmarks_section: BookmarksListsSection::Bookmarks,
            bookmark_selected_folder: "All".to_string(),
            bookmark_custom_folders: Vec::new(),
            bookmark_new_folder_draft: String::new(),
            bookmark_moving_post_id: None,
            bookmark_state_epoch: 0,
            bookmark_flight_seq: 0,
            bookmark_in_flight: HashMap::new(),
            bookmark_dirty: HashSet::new(),
            bookmark_pending_target: HashMap::new(),
            bookmark_last_synced: HashMap::new(),
            bookmark_cached_items: HashMap::new(),
            bookmark_recent_overrides: HashMap::new(),
            selected_list_id: None,
            list_feed_items: Vec::new(),
            list_feed_sort: FeedSort::New,
            list_feed_actor_filter: ActorFilter::AllCitizens,
            list_feed_loading: false,
            list_feed_gen: 0,
            list_membership_gen: 0,
            list_membership_flight_seq: 0,
            list_membership_in_flight: HashMap::new(),
            list_membership_dirty: HashSet::new(),
            list_membership_needs_feed_reload: HashSet::new(),
            list_membership_last_synced: HashMap::new(),
            list_deleting: HashSet::new(),
            list_editor_open: false,
            list_editor: ListEditorDraft::default(),
            saved_loading: false,
            saved_gen: 0,
            mod_slug: None,
            mod_tab: ModConsoleTab::Queue,
            mod_status_filter: Some(ModQueueStatus::Pending),
            mod_reason_draft: String::new(),
            mod_flair_draft: String::new(),
            mod_community: None,
            mod_queue: Vec::new(),
            mod_logs: Vec::new(),
            mod_error: None,
            automod_draft: AutoModRuleDraft::default(),
            automod_dry_run_title: String::new(),
            automod_dry_run_body: String::new(),
            automod_dry_run_is_bot: false,
            automod_dry_run_result: None,
            automod_rules_gen: 0,
            automod_flight_seq: 0,
            automod_in_flight: HashMap::new(),
            automod_dirty: HashSet::new(),
            automod_reset_draft_pending: HashSet::new(),
            automod_last_synced: HashMap::new(),
            mod_loading: false,
            mod_gen: 0,
            bot_studio_name: String::new(),
            bot_studio_handle: String::new(),
            bot_studio_persona: String::new(),
            bot_studio_bio: String::new(),
            bot_studio_badge: "AI Agent".to_string(),
            bot_studio_model: "Fabushi-1".to_string(),
            bot_studio_capabilities: vec!["community".to_string(), "debate".to_string()],
            bot_studio_custom_cap_draft: String::new(),
            bot_studio_cooldown_secs: "30".to_string(),
            bot_studio_max_posts_per_hour: "12".to_string(),
            bot_studio_shape_idx: 0,
            bot_studio_color_idx: 0,
            bot_studio_autonomy: BotAutonomyMode::MentionOnly,
            bot_studio_submitting: false,
            bot_studio_last_token: None,
            bot_studio_tokens_by_actor: HashMap::new(),
            bot_studio_token_revealed: false,
            bot_studio_selected_bot_id: None,
            bot_studio_edit: BotStudioEditDraft::default(),
            bot_studio_test_community: None,
            bot_studio_test_content: String::new(),
            bot_studio_test_submitting: false,
            bot_studio_test_last_post_id: None,
            bot_studio_test_feedback: None,
            bot_studio_error: None,
        }
    }
}

impl CommunityWorkspaceState {
    pub fn push_route(&mut self, next: CommunityRoute) {
        if self.route != next {
            let prev = std::mem::replace(&mut self.route, next);
            self.route_stack.push(prev);
            if self.route_stack.len() > 24 {
                self.route_stack.remove(0);
            }
        }
        if !matches!(self.route, CommunityRoute::PostDetail(_)) {
            self.detail_opened_from_notification = false;
            self.detail_focused_comment_id = None;
        }
        self.active_popover = None;
    }

    pub fn replace_route(&mut self, next: CommunityRoute) {
        self.route_stack.clear();
        self.route = next;
        if !matches!(self.route, CommunityRoute::PostDetail(_)) {
            self.detail_opened_from_notification = false;
            self.detail_focused_comment_id = None;
        }
        self.active_popover = None;
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.route_stack.pop() {
            self.route = prev;
            if !matches!(self.route, CommunityRoute::PostDetail(_)) {
                self.detail_opened_from_notification = false;
                self.detail_focused_comment_id = None;
            }
            self.active_popover = None;
            true
        } else if self.route != CommunityRoute::Feed {
            self.route = CommunityRoute::Feed;
            self.detail_opened_from_notification = false;
            self.detail_focused_comment_id = None;
            self.active_popover = None;
            true
        } else {
            false
        }
    }

    pub fn focus_input(&mut self, target: ActiveCommunityInput) {
        if self.active_input != target {
            self.active_input = target;
            self.ime_preedit = None;
            self.input_generation = self.input_generation.wrapping_add(1);
        }
    }

    pub fn blur_input(&mut self) {
        if self.active_input != ActiveCommunityInput::None || self.ime_preedit.is_some() {
            self.active_input = ActiveCommunityInput::None;
            self.ime_preedit = None;
            self.input_generation = self.input_generation.wrapping_add(1);
        }
    }

    pub fn active_input_text(&self) -> &str {
        match self.active_input {
            ActiveCommunityInput::None => "",
            ActiveCommunityInput::QuickPostTitle => &self.quick_post_title,
            ActiveCommunityInput::QuickPostBody => &self.quick_post_body,
            ActiveCommunityInput::RightRailSearch => &self.right_rail_search_draft,
            ActiveCommunityInput::ExploreSearch => &self.explore_search_draft,
            ActiveCommunityInput::DetailReply => &self.detail_reply_draft,
            ActiveCommunityInput::QuotePostNote => &self.quote_post_draft,
            ActiveCommunityInput::BotStudioName => &self.bot_studio_name,
            ActiveCommunityInput::BotStudioHandle => &self.bot_studio_handle,
            ActiveCommunityInput::BotStudioPersona => &self.bot_studio_persona,
            ActiveCommunityInput::DetailEditComment => &self.detail_edit_comment_draft,
            ActiveCommunityInput::DetailSummonPrompt => &self.detail_summon_prompt,
            ActiveCommunityInput::ComposerTitle => &self.composer.title,
            ActiveCommunityInput::ComposerBody => &self.composer.body,
            ActiveCommunityInput::ComposerLinkUrl => &self.composer.link_url,
            ActiveCommunityInput::ComposerLinkTitle => &self.composer.link_title,
            ActiveCommunityInput::ComposerLinkDesc => &self.composer.link_description,
            ActiveCommunityInput::ComposerMediaUrl(i) => self
                .composer
                .media_items
                .get(i)
                .map(|m| m.url_or_id.as_str())
                .unwrap_or(""),
            ActiveCommunityInput::ComposerMediaAlt(i) => self
                .composer
                .media_items
                .get(i)
                .map(|m| m.alt_text.as_str())
                .unwrap_or(""),
            ActiveCommunityInput::ComposerPollQuestion => &self.composer.poll_question,
            ActiveCommunityInput::ComposerPollOption(i) => self
                .composer
                .poll_options
                .get(i)
                .map(String::as_str)
                .unwrap_or(""),
            ActiveCommunityInput::ComposerThreadPart(i) => self
                .composer
                .thread_parts
                .get(i)
                .map(String::as_str)
                .unwrap_or(""),
            ActiveCommunityInput::ComposerScheduleMinutes => &self.composer.schedule_minutes_input,
            ActiveCommunityInput::ComposerBotPrompt => &self.composer.bot_prompt,
            ActiveCommunityInput::CommunityWikiMarkdown => &self.community_wiki_draft,
            ActiveCommunityInput::CommunityWikiSummary => &self.community_wiki_summary,
            ActiveCommunityInput::CommunityNewRuleTitle => &self.community_new_rule_title,
            ActiveCommunityInput::CommunityNewRuleDesc => &self.community_new_rule_desc,
            ActiveCommunityInput::CreateCommSlug => &self.create_community.slug,
            ActiveCommunityInput::CreateCommName => &self.create_community.name,
            ActiveCommunityInput::CreateCommDescription => &self.create_community.description,
            ActiveCommunityInput::CreateCommIconEmoji => &self.create_community.icon_emoji,
            ActiveCommunityInput::ProfileEditDisplayName => &self.edit_profile.display_name,
            ActiveCommunityInput::ProfileEditBio => &self.edit_profile.bio,
            ActiveCommunityInput::ProfileEditLocation => &self.edit_profile.location,
            ActiveCommunityInput::ProfileEditWebsite => &self.edit_profile.website,
            ActiveCommunityInput::ProfileEditBotBadge => &self.edit_profile.bot_badge,
            ActiveCommunityInput::ProfileEditBotModel => &self.edit_profile.bot_model,
            ActiveCommunityInput::ProfileEditBotPersona => &self.edit_profile.bot_persona,
            ActiveCommunityInput::BookmarkNewFolder => &self.bookmark_new_folder_draft,
            ActiveCommunityInput::ListEditorName => &self.list_editor.name,
            ActiveCommunityInput::ListEditorDesc => &self.list_editor.description,
            ActiveCommunityInput::ModReason => &self.mod_reason_draft,
            ActiveCommunityInput::ModFlair => &self.mod_flair_draft,
            ActiveCommunityInput::AutoModRuleName => &self.automod_draft.name,
            ActiveCommunityInput::AutoModRuleKeywords => &self.automod_draft.keywords,
            ActiveCommunityInput::AutoModRuleRegex => &self.automod_draft.regex_pattern,
            ActiveCommunityInput::AutoModRuleMinKarma => &self.automod_draft.min_karma,
            ActiveCommunityInput::AutoModRuleMaxLinks => &self.automod_draft.max_links,
            ActiveCommunityInput::AutoModRuleFlair => &self.automod_draft.assign_flair,
            ActiveCommunityInput::AutoModRuleReply => &self.automod_draft.auto_reply_template,
            ActiveCommunityInput::AutoModDryRunTitle => &self.automod_dry_run_title,
            ActiveCommunityInput::AutoModDryRunBody => &self.automod_dry_run_body,
            ActiveCommunityInput::BotStudioBio => &self.bot_studio_bio,
            ActiveCommunityInput::BotStudioBadge => &self.bot_studio_badge,
            ActiveCommunityInput::BotStudioModel => &self.bot_studio_model,
            ActiveCommunityInput::BotStudioCustomCap => &self.bot_studio_custom_cap_draft,
            ActiveCommunityInput::BotStudioCooldown => &self.bot_studio_cooldown_secs,
            ActiveCommunityInput::BotStudioMaxPostsPerHour => &self.bot_studio_max_posts_per_hour,
            ActiveCommunityInput::BotStudioEditName => &self.bot_studio_edit.display_name,
            ActiveCommunityInput::BotStudioEditBio => &self.bot_studio_edit.bio,
            ActiveCommunityInput::BotStudioEditBadge => &self.bot_studio_edit.bot_badge,
            ActiveCommunityInput::BotStudioEditModel => &self.bot_studio_edit.bot_model,
            ActiveCommunityInput::BotStudioEditCustomCap => &self.bot_studio_edit.custom_cap_draft,
            ActiveCommunityInput::BotStudioEditPersona => &self.bot_studio_edit.bot_persona,
            ActiveCommunityInput::BotStudioTestPostBody => &self.bot_studio_test_content,
        }
    }

    pub fn active_input_text_mut(&mut self) -> Option<&mut String> {
        match self.active_input {
            ActiveCommunityInput::None => None,
            ActiveCommunityInput::QuickPostTitle => Some(&mut self.quick_post_title),
            ActiveCommunityInput::QuickPostBody => Some(&mut self.quick_post_body),
            ActiveCommunityInput::RightRailSearch => Some(&mut self.right_rail_search_draft),
            ActiveCommunityInput::ExploreSearch => Some(&mut self.explore_search_draft),
            ActiveCommunityInput::DetailReply => Some(&mut self.detail_reply_draft),
            ActiveCommunityInput::QuotePostNote => Some(&mut self.quote_post_draft),
            ActiveCommunityInput::BotStudioName => Some(&mut self.bot_studio_name),
            ActiveCommunityInput::BotStudioHandle => Some(&mut self.bot_studio_handle),
            ActiveCommunityInput::BotStudioPersona => Some(&mut self.bot_studio_persona),
            ActiveCommunityInput::DetailEditComment => Some(&mut self.detail_edit_comment_draft),
            ActiveCommunityInput::DetailSummonPrompt => Some(&mut self.detail_summon_prompt),
            ActiveCommunityInput::ComposerTitle => Some(&mut self.composer.title),
            ActiveCommunityInput::ComposerBody => Some(&mut self.composer.body),
            ActiveCommunityInput::ComposerLinkUrl => Some(&mut self.composer.link_url),
            ActiveCommunityInput::ComposerLinkTitle => Some(&mut self.composer.link_title),
            ActiveCommunityInput::ComposerLinkDesc => Some(&mut self.composer.link_description),
            ActiveCommunityInput::ComposerMediaUrl(i) => {
                self.composer.media_items.get_mut(i).map(|m| &mut m.url_or_id)
            }
            ActiveCommunityInput::ComposerMediaAlt(i) => {
                self.composer.media_items.get_mut(i).map(|m| &mut m.alt_text)
            }
            ActiveCommunityInput::ComposerPollQuestion => Some(&mut self.composer.poll_question),
            ActiveCommunityInput::ComposerPollOption(i) => self.composer.poll_options.get_mut(i),
            ActiveCommunityInput::ComposerThreadPart(i) => self.composer.thread_parts.get_mut(i),
            ActiveCommunityInput::ComposerScheduleMinutes => {
                Some(&mut self.composer.schedule_minutes_input)
            }
            ActiveCommunityInput::ComposerBotPrompt => Some(&mut self.composer.bot_prompt),
            ActiveCommunityInput::CommunityWikiMarkdown => Some(&mut self.community_wiki_draft),
            ActiveCommunityInput::CommunityWikiSummary => Some(&mut self.community_wiki_summary),
            ActiveCommunityInput::CommunityNewRuleTitle => Some(&mut self.community_new_rule_title),
            ActiveCommunityInput::CommunityNewRuleDesc => Some(&mut self.community_new_rule_desc),
            ActiveCommunityInput::CreateCommSlug => Some(&mut self.create_community.slug),
            ActiveCommunityInput::CreateCommName => Some(&mut self.create_community.name),
            ActiveCommunityInput::CreateCommDescription => {
                Some(&mut self.create_community.description)
            }
            ActiveCommunityInput::CreateCommIconEmoji => Some(&mut self.create_community.icon_emoji),
            ActiveCommunityInput::ProfileEditDisplayName => {
                Some(&mut self.edit_profile.display_name)
            }
            ActiveCommunityInput::ProfileEditBio => Some(&mut self.edit_profile.bio),
            ActiveCommunityInput::ProfileEditLocation => Some(&mut self.edit_profile.location),
            ActiveCommunityInput::ProfileEditWebsite => Some(&mut self.edit_profile.website),
            ActiveCommunityInput::ProfileEditBotBadge => Some(&mut self.edit_profile.bot_badge),
            ActiveCommunityInput::ProfileEditBotModel => Some(&mut self.edit_profile.bot_model),
            ActiveCommunityInput::ProfileEditBotPersona => Some(&mut self.edit_profile.bot_persona),
            ActiveCommunityInput::BookmarkNewFolder => Some(&mut self.bookmark_new_folder_draft),
            ActiveCommunityInput::ListEditorName => Some(&mut self.list_editor.name),
            ActiveCommunityInput::ListEditorDesc => Some(&mut self.list_editor.description),
            ActiveCommunityInput::ModReason => Some(&mut self.mod_reason_draft),
            ActiveCommunityInput::ModFlair => Some(&mut self.mod_flair_draft),
            ActiveCommunityInput::AutoModRuleName => Some(&mut self.automod_draft.name),
            ActiveCommunityInput::AutoModRuleKeywords => Some(&mut self.automod_draft.keywords),
            ActiveCommunityInput::AutoModRuleRegex => Some(&mut self.automod_draft.regex_pattern),
            ActiveCommunityInput::AutoModRuleMinKarma => Some(&mut self.automod_draft.min_karma),
            ActiveCommunityInput::AutoModRuleMaxLinks => Some(&mut self.automod_draft.max_links),
            ActiveCommunityInput::AutoModRuleFlair => Some(&mut self.automod_draft.assign_flair),
            ActiveCommunityInput::AutoModRuleReply => {
                Some(&mut self.automod_draft.auto_reply_template)
            }
            ActiveCommunityInput::AutoModDryRunTitle => Some(&mut self.automod_dry_run_title),
            ActiveCommunityInput::AutoModDryRunBody => Some(&mut self.automod_dry_run_body),
            ActiveCommunityInput::BotStudioBio => Some(&mut self.bot_studio_bio),
            ActiveCommunityInput::BotStudioBadge => Some(&mut self.bot_studio_badge),
            ActiveCommunityInput::BotStudioModel => Some(&mut self.bot_studio_model),
            ActiveCommunityInput::BotStudioCustomCap => Some(&mut self.bot_studio_custom_cap_draft),
            ActiveCommunityInput::BotStudioCooldown => Some(&mut self.bot_studio_cooldown_secs),
            ActiveCommunityInput::BotStudioMaxPostsPerHour => {
                Some(&mut self.bot_studio_max_posts_per_hour)
            }
            ActiveCommunityInput::BotStudioEditName => Some(&mut self.bot_studio_edit.display_name),
            ActiveCommunityInput::BotStudioEditBio => Some(&mut self.bot_studio_edit.bio),
            ActiveCommunityInput::BotStudioEditBadge => Some(&mut self.bot_studio_edit.bot_badge),
            ActiveCommunityInput::BotStudioEditModel => Some(&mut self.bot_studio_edit.bot_model),
            ActiveCommunityInput::BotStudioEditCustomCap => {
                Some(&mut self.bot_studio_edit.custom_cap_draft)
            }
            ActiveCommunityInput::BotStudioEditPersona => {
                Some(&mut self.bot_studio_edit.bot_persona)
            }
            ActiveCommunityInput::BotStudioTestPostBody => Some(&mut self.bot_studio_test_content),
        }
    }

    pub fn active_actor(&self) -> Option<&CommunityActorDto> {
        let ids = self.identities.as_ref()?;
        if !self.active_actor_id.is_empty() {
            if let Some(found) = ids.all_actors.iter().find(|a| a.actor_id == self.active_actor_id) {
                return Some(found);
            }
            if let Some(h) = ids.human_actor.as_ref().filter(|a| a.actor_id == self.active_actor_id) {
                return Some(h);
            }
            if let Some(b) = ids.bot_actors.iter().find(|a| a.actor_id == self.active_actor_id) {
                return Some(b);
            }
        }
        ids.human_actor.as_ref().or_else(|| ids.all_actors.first())
    }
}

impl CommunityWorkspaceState {
    pub fn visit_post_mut(&mut self, post_id: &str, mut f: impl FnMut(&mut CommunityPostDto)) {
        for item in &mut self.feed_items {
            if item.post.post_id == post_id {
                f(&mut item.post);
            }
        }
        for item in &mut self.community_posts {
            if item.post.post_id == post_id {
                f(&mut item.post);
            }
        }
        if let Some(ref mut cdet) = self.community_detail {
            for post in &mut cdet.pinned_posts {
                if post.post_id == post_id {
                    f(post);
                }
            }
        }
        for item in &mut self.saved_items {
            if item.post.post_id == post_id {
                f(&mut item.post);
            }
        }
        for item in &mut self.list_feed_items {
            if item.post.post_id == post_id {
                f(&mut item.post);
            }
        }
        if let Some(ref mut det) = self.detail_post {
            if det.post.post_id == post_id {
                f(&mut det.post);
            }
            for post in &mut det.thread_chain {
                if post.post_id == post_id {
                    f(post);
                }
            }
            for post in &mut det.quote_chain {
                if post.post_id == post_id {
                    f(post);
                }
            }
        }
        if let Some(ref mut prof) = self.actor_profile {
            for post in &mut prof.recent_posts {
                if post.post_id == post_id {
                    f(post);
                }
            }
        }
        if let Some(ref mut sr) = self.search_result {
            for post in &mut sr.posts {
                if post.post_id == post_id {
                    f(post);
                }
            }
        }
    }

    pub fn find_post(&self, post_id: &str) -> Option<&CommunityPostDto> {
        self.feed_items
            .iter()
            .map(|i| &i.post)
            .chain(self.community_posts.iter().map(|i| &i.post))
            .chain(
                self.community_detail
                    .iter()
                    .flat_map(|d| d.pinned_posts.iter()),
            )
            .chain(self.saved_items.iter().map(|i| &i.post))
            .chain(self.list_feed_items.iter().map(|i| &i.post))
            .chain(self.detail_post.iter().map(|d| &d.post))
            .chain(self.detail_post.iter().flat_map(|d| d.thread_chain.iter()))
            .chain(self.detail_post.iter().flat_map(|d| d.quote_chain.iter()))
            .chain(self.actor_profile.iter().flat_map(|p| p.recent_posts.iter()))
            .chain(self.search_result.iter().flat_map(|s| s.posts.iter()))
            .find(|p| p.post_id == post_id)
    }

    pub fn find_feed_item(&self, post_id: &str) -> Option<&FeedItemDto> {
        self.feed_items
            .iter()
            .chain(self.community_posts.iter())
            .chain(self.saved_items.iter())
            .chain(self.list_feed_items.iter())
            .find(|i| i.post.post_id == post_id)
    }

    pub fn snapshot_feed_item_for_post(&self, post_id: &str) -> Option<FeedItemDto> {
        self.find_feed_item(post_id)
            .cloned()
            .or_else(|| {
                self.find_post(post_id).cloned().map(|post| FeedItemDto {
                    post,
                    explanation: None,
                    rank_score: 0.0,
                    is_exploration_slot: false,
                })
            })
            .or_else(|| self.bookmark_cached_items.get(post_id).cloned())
    }

    /// Recursively finds an immutable comment reference inside the open PostDetail tree.
    pub fn find_comment(&self, comment_id: &str) -> Option<&CommunityCommentDto> {
        fn find_in_slice<'a>(
            nodes: &'a [CommunityCommentDto],
            cid: &str,
        ) -> Option<&'a CommunityCommentDto> {
            for n in nodes {
                if n.comment_id == cid {
                    return Some(n);
                }
                if let Some(found) = find_in_slice(&n.children, cid) {
                    return Some(found);
                }
            }
            None
        }
        self.detail_post
            .as_ref()
            .and_then(|d| find_in_slice(&d.comments, comment_id))
            .or_else(|| {
                self.actor_profile
                    .as_ref()
                    .and_then(|p| p.recent_comments.iter().find(|c| c.comment_id == comment_id))
            })
    }

    /// Recursively visits matching comments in the open PostDetail tree and ActorProfile cache.
    pub fn visit_comment_mut(
        &mut self,
        comment_id: &str,
        mut f: impl FnMut(&mut CommunityCommentDto),
    ) {
        fn visit_slice(
            nodes: &mut [CommunityCommentDto],
            cid: &str,
            f: &mut impl FnMut(&mut CommunityCommentDto),
        ) {
            for n in nodes {
                if n.comment_id == cid {
                    f(n);
                }
                visit_slice(&mut n.children, cid, f);
            }
        }
        if let Some(ref mut det) = self.detail_post {
            visit_slice(&mut det.comments, comment_id, &mut f);
        }
        if let Some(ref mut prof) = self.actor_profile {
            for c in &mut prof.recent_comments {
                if c.comment_id == comment_id {
                    f(c);
                }
            }
        }
    }

    /// Optimistically applies a vote toggle (`1 | -1`, toggling to `0`) on a comment.
    pub fn apply_optimistic_comment_vote(
        &mut self,
        comment_id: &str,
        clicked_dir: i8,
    ) -> (i8, Option<(i8, i64, i64, i64)>) {
        let mut snap = None;
        let mut next_dir = clicked_dir;
        if let Some(c) = self.find_comment(comment_id) {
            snap = Some((c.viewer_vote, c.upvotes, c.downvotes, c.score));
            next_dir = if c.viewer_vote == clicked_dir { 0 } else { clicked_dir };
        }
        self.visit_comment_mut(comment_id, |c| {
            let prev = c.viewer_vote;
            if prev == 1 {
                c.upvotes = (c.upvotes - 1).max(0);
            } else if prev == -1 {
                c.downvotes = (c.downvotes - 1).max(0);
            }
            if next_dir == 1 {
                c.upvotes += 1;
            } else if next_dir == -1 {
                c.downvotes += 1;
            }
            c.score = c.upvotes - c.downvotes;
            c.viewer_vote = next_dir;
        });
        (next_dir, snap)
    }

    pub fn rollback_comment_vote(&mut self, comment_id: &str, snap: (i8, i64, i64, i64)) {
        self.visit_comment_mut(comment_id, |c| {
            c.viewer_vote = snap.0;
            c.upvotes = snap.1;
            c.downvotes = snap.2;
            c.score = snap.3;
        });
    }

    pub fn reconcile_comment_vote(&mut self, resp: &VoteResponse) {
        self.visit_comment_mut(&resp.target_id, |c| {
            c.viewer_vote = resp.direction;
            c.upvotes = resp.upvotes;
            c.downvotes = resp.downvotes;
            c.score = resp.score;
        });
    }

    pub fn apply_optimistic_comment_like(&mut self, comment_id: &str) -> Option<(bool, u64)> {
        let snap = self
            .find_comment(comment_id)
            .map(|c| (c.viewer_liked, c.likes_count));
        self.visit_comment_mut(comment_id, |c| {
            if c.viewer_liked {
                c.viewer_liked = false;
                c.likes_count = c.likes_count.saturating_sub(1);
            } else {
                c.viewer_liked = true;
                c.likes_count = c.likes_count.saturating_add(1);
            }
        });
        snap
    }

    pub fn rollback_comment_like(&mut self, comment_id: &str, snap: (bool, u64)) {
        self.visit_comment_mut(comment_id, |c| {
            c.viewer_liked = snap.0;
            c.likes_count = snap.1;
        });
    }

    pub fn reconcile_comment_like(&mut self, resp: &ToggleActionResponse) {
        self.visit_comment_mut(&resp.target_id, |c| {
            c.viewer_liked = resp.active;
            c.likes_count = resp.count;
        });
    }

    pub fn reconcile_comment_awards(&mut self, resp: &AwardResponse) {
        self.visit_comment_mut(&resp.target_id, |c| {
            c.awards = resp.awards.clone();
        });
    }

    /// Merges newly loaded child comments under `parent_comment_id` without duplicate nodes.
    pub fn merge_loaded_comment_children(
        &mut self,
        parent_comment_id: &str,
        loaded: Vec<CommunityCommentDto>,
        has_more: bool,
    ) {
        fn collect_ids(nodes: &[CommunityCommentDto], out: &mut HashSet<String>) {
            for n in nodes {
                out.insert(n.comment_id.clone());
                collect_ids(&n.children, out);
            }
        }
        fn merge_into(
            nodes: &mut [CommunityCommentDto],
            parent_id: &str,
            loaded: &[CommunityCommentDto],
            has_more: bool,
        ) -> bool {
            for n in nodes {
                if n.comment_id == parent_id {
                    let mut existing = HashSet::new();
                    collect_ids(&n.children, &mut existing);
                    let mut added = 0u32;
                    for ch in loaded {
                        if existing.insert(ch.comment_id.clone()) {
                            n.children.push(ch.clone());
                            added = added.saturating_add(1);
                        }
                    }
                    n.more_children_count = if has_more {
                        n.more_children_count.saturating_sub(added).max(1)
                    } else {
                        0
                    };
                    return true;
                }
                if merge_into(&mut n.children, parent_id, loaded, has_more) {
                    return true;
                }
            }
            false
        }
        if let Some(ref mut det) = self.detail_post {
            merge_into(&mut det.comments, parent_comment_id, &loaded, has_more);
        }
    }

    /// Removes a deleted post from all feed/community/saved/profile/search caches.
    pub fn remove_post_from_caches(&mut self, post_id: &str) {
        self.feed_items.retain(|i| i.post.post_id != post_id);
        self.community_posts.retain(|i| i.post.post_id != post_id);
        self.saved_items.retain(|i| i.post.post_id != post_id);
        self.list_feed_items.retain(|i| i.post.post_id != post_id);
        if let Some(ref mut cdet) = self.community_detail {
            cdet.pinned_posts.retain(|p| p.post_id != post_id);
        }
        if let Some(ref mut prof) = self.actor_profile {
            prof.recent_posts.retain(|p| p.post_id != post_id);
        }
        if let Some(ref mut sr) = self.search_result {
            sr.posts.retain(|p| p.post_id != post_id);
        }
        self.composer.drafts.retain(|p| p.post_id != post_id);
        self.bookmark_in_flight.remove(post_id);
        self.bookmark_dirty.remove(post_id);
        self.bookmark_pending_target.remove(post_id);
        self.bookmark_last_synced.remove(post_id);
        self.bookmark_cached_items.remove(post_id);
        self.bookmark_recent_overrides.remove(post_id);
        self.post_vote_gen.remove(post_id);
        self.post_like_in_flight.remove(post_id);
        self.post_like_pending_toggle.remove(post_id);
        self.post_like_last_synced.remove(post_id);
    }

    /// Optimistically applies a vote toggle (`1 | -1`, toggling to `0` if already active).
    /// Returns `(new_direction, rollback_snapshot)` when the post exists locally.
    pub fn apply_optimistic_vote(
        &mut self,
        post_id: &str,
        clicked_dir: i8,
    ) -> (i8, Option<(i8, i64, i64, i64)>) {
        let mut snap = None;
        let mut next_dir = clicked_dir;
        if let Some(p) = self.find_post(post_id) {
            snap = Some((p.viewer_vote, p.upvotes, p.downvotes, p.score));
            next_dir = if p.viewer_vote == clicked_dir { 0 } else { clicked_dir };
        }
        self.visit_post_mut(post_id, |post| {
            let prev = post.viewer_vote;
            if prev == 1 {
                post.upvotes = (post.upvotes - 1).max(0);
            } else if prev == -1 {
                post.downvotes = (post.downvotes - 1).max(0);
            }
            if next_dir == 1 {
                post.upvotes += 1;
            } else if next_dir == -1 {
                post.downvotes += 1;
            }
            post.score = post.upvotes - post.downvotes;
            post.viewer_vote = next_dir;
        });
        (next_dir, snap)
    }

    pub fn rollback_vote(&mut self, post_id: &str, snap: (i8, i64, i64, i64)) {
        self.visit_post_mut(post_id, |post| {
            post.viewer_vote = snap.0;
            post.upvotes = snap.1;
            post.downvotes = snap.2;
            post.score = snap.3;
        });
    }

    pub fn reconcile_vote(&mut self, resp: &VoteResponse) {
        self.visit_post_mut(&resp.target_id, |post| {
            post.viewer_vote = resp.direction;
            post.upvotes = resp.upvotes;
            post.downvotes = resp.downvotes;
            post.score = resp.score;
        });
    }

    pub fn apply_optimistic_like(&mut self, post_id: &str) -> Option<(bool, u64)> {
        let snap = self.find_post(post_id).map(|p| (p.viewer_liked, p.likes_count));
        self.visit_post_mut(post_id, |post| {
            if post.viewer_liked {
                post.viewer_liked = false;
                post.likes_count = post.likes_count.saturating_sub(1);
            } else {
                post.viewer_liked = true;
                post.likes_count = post.likes_count.saturating_add(1);
            }
        });
        snap
    }

    pub fn rollback_like(&mut self, post_id: &str, snap: (bool, u64)) {
        self.visit_post_mut(post_id, |post| {
            post.viewer_liked = snap.0;
            post.likes_count = snap.1;
        });
    }

    pub fn reconcile_like(&mut self, resp: &ToggleActionResponse) {
        self.visit_post_mut(&resp.target_id, |post| {
            post.viewer_liked = resp.active;
            post.likes_count = resp.count;
        });
    }

    pub fn reconcile_repost(&mut self, resp: &RepostResponse) {
        self.visit_post_mut(&resp.post_id, |post| {
            post.viewer_reposted = resp.reposted;
            post.reposts_count = resp.reposts_count;
        });
    }

    pub fn reconcile_bookmark(&mut self, resp: &BookmarkResponse) {
        let resolved_folder = if resp.bookmarked {
            Some(normalize_bookmark_folder(&resp.folder))
        } else {
            None
        };
        self.visit_post_mut(&resp.post_id, |post| {
            post.viewer_bookmarked = resp.bookmarked;
            post.viewer_bookmark_folder = resolved_folder.clone();
            post.bookmarks_count = resp.bookmarks_count;
        });
        if !resp.bookmarked {
            self.saved_items
                .retain(|it| it.post.post_id != resp.post_id);
        } else if !self
            .saved_items
            .iter()
            .any(|it| it.post.post_id == resp.post_id)
        {
            if let Some(mut item) = self.snapshot_feed_item_for_post(&resp.post_id) {
                item.post.viewer_bookmarked = true;
                item.post.viewer_bookmark_folder = resolved_folder;
                item.post.bookmarks_count = resp.bookmarks_count;
                self.saved_items.insert(0, item);
            }
        }
    }

    pub fn reconcile_poll(&mut self, post_id: &str, poll: PollDto) {
        self.visit_post_mut(post_id, |post| {
            post.poll = Some(poll.clone());
        });
    }

    pub fn reconcile_awards(&mut self, resp: &AwardResponse) {
        self.visit_post_mut(&resp.target_id, |post| {
            post.awards = resp.awards.clone();
        });
    }

    pub fn reconcile_join(&mut self, resp: &JoinCommunityResponse) {
        for c in &mut self.communities {
            if c.slug.eq_ignore_ascii_case(&resp.community_slug) {
                c.is_joined = resp.is_joined;
                c.member_count = resp.member_count;
                c.viewer_role = resp.viewer_role;
            }
        }
        if let Some(ref mut det) = self.community_detail {
            if det.community.slug.eq_ignore_ascii_case(&resp.community_slug) {
                det.community.is_joined = resp.is_joined;
                det.community.member_count = resp.member_count;
                det.community.viewer_role = resp.viewer_role;
            }
        }
        if let Some(ref mut mc) = self.mod_community {
            if mc.slug.eq_ignore_ascii_case(&resp.community_slug) {
                mc.is_joined = resp.is_joined;
                mc.member_count = resp.member_count;
                mc.viewer_role = resp.viewer_role;
            }
        }
        if let Some(ref mut sr) = self.search_result {
            for c in &mut sr.communities {
                if c.slug.eq_ignore_ascii_case(&resp.community_slug) {
                    c.is_joined = resp.is_joined;
                    c.member_count = resp.member_count;
                    c.viewer_role = resp.viewer_role;
                }
            }
        }
    }

    pub fn reconcile_follow(&mut self, resp: &FollowActionResponse) {
        for a in &mut self.recommended_actors {
            if a.actor_id == resp.actor_id {
                a.is_following = resp.is_following;
                a.followers_count = resp.followers_count;
            }
        }
        if let Some(ref mut prof) = self.actor_profile {
            if prof.actor.actor_id == resp.actor_id {
                prof.actor.is_following = resp.is_following;
                prof.actor.followers_count = resp.followers_count;
            }
        }
        if let Some(ref mut sr) = self.search_result {
            for a in &mut sr.actors {
                if a.actor_id == resp.actor_id {
                    a.is_following = resp.is_following;
                    a.followers_count = resp.followers_count;
                }
            }
        }
    }

    pub fn find_actor(&self, actor_id: &str) -> Option<&CommunityActorDto> {
        self.actor_profile
            .as_ref()
            .map(|p| &p.actor)
            .filter(|a| a.actor_id == actor_id)
            .or_else(|| {
                self.recommended_actors
                    .iter()
                    .find(|a| a.actor_id == actor_id)
            })
            .or_else(|| {
                self.search_result
                    .as_ref()
                    .and_then(|sr| sr.actors.iter().find(|a| a.actor_id == actor_id))
            })
    }

    pub fn reconcile_mute_block(
        &mut self,
        actor_id: &str,
        is_muted: Option<bool>,
        is_blocked: Option<bool>,
    ) {
        let apply = |a: &mut CommunityActorDto| {
            if a.actor_id == actor_id {
                if let Some(m) = is_muted {
                    a.is_muted = m;
                }
                if let Some(b) = is_blocked {
                    a.is_blocked = b;
                    if b {
                        a.is_following = false;
                    }
                }
            }
        };
        for a in &mut self.recommended_actors {
            apply(a);
        }
        if let Some(ref mut prof) = self.actor_profile {
            apply(&mut prof.actor);
        }
        if let Some(ref mut sr) = self.search_result {
            for a in &mut sr.actors {
                apply(a);
            }
        }
    }

    pub fn restore_block_and_follow(
        &mut self,
        actor_id: &str,
        is_blocked: bool,
        is_following: bool,
    ) {
        let apply = |a: &mut CommunityActorDto| {
            if a.actor_id == actor_id {
                a.is_blocked = is_blocked;
                a.is_following = is_following;
            }
        };
        for a in &mut self.recommended_actors {
            apply(a);
        }
        if let Some(ref mut prof) = self.actor_profile {
            apply(&mut prof.actor);
        }
        if let Some(ref mut sr) = self.search_result {
            for a in &mut sr.actors {
                apply(a);
            }
        }
    }

    /// Replaces `self.lists` with a freshly fetched snapshot while excluding lists currently
    /// being deleted and preserving optimistic `(is_pinned, member_actor_ids, community_slugs)`
    /// for any list with an active or pending single-flight sync.
    pub fn merge_refreshed_lists(&mut self, mut fresh: Vec<CommunityListDto>) {
        if !self.list_deleting.is_empty() {
            fresh.retain(|item| !self.list_deleting.contains(&item.list_id));
        }
        if !self.list_membership_in_flight.is_empty() || !self.list_membership_dirty.is_empty() {
            for item in &mut fresh {
                if self.list_membership_in_flight.contains_key(&item.list_id)
                    || self.list_membership_dirty.contains(&item.list_id)
                {
                    if let Some(prev) = self.lists.iter().find(|x| x.list_id == item.list_id) {
                        item.is_pinned = prev.is_pinned;
                        item.member_actor_ids = prev.member_actor_ids.clone();
                        item.community_slugs = prev.community_slugs.clone();
                    }
                }
            }
        }
        self.lists = fresh;
    }

    /// Replaces `self.saved_items` with a freshly fetched snapshot while preserving:
    /// 1. Order 2 post-settlement overrides (`bookmark_recent_overrides` with `ov.epoch > started_bookmark_epoch`)
    ///    that committed while this `load_saved_posts` was still awaiting `list_lists()`, and
    /// 2. Order 1 currently in-flight or dirty optimistic bookmark targets.
    pub fn merge_refreshed_saved_items(
        &mut self,
        mut fresh: Vec<FeedItemDto>,
        started_bookmark_epoch: u64,
    ) {
        // Step 1 (Order 2): apply post-settlement overrides newer than when `load_saved_posts` started.
        for (pid, ov) in &self.bookmark_recent_overrides {
            if ov.epoch <= started_bookmark_epoch
                || self.bookmark_in_flight.contains_key(pid)
                || self.bookmark_dirty.contains(pid)
            {
                continue;
            }
            if !ov.bookmarked {
                fresh.retain(|it| it.post.post_id != *pid);
            } else if let Some(existing) = fresh.iter_mut().find(|it| it.post.post_id == *pid) {
                existing.post.viewer_bookmarked = true;
                existing.post.viewer_bookmark_folder = ov.folder.clone();
                if let Some(cnt) = ov.bookmarks_count {
                    existing.post.bookmarks_count = cnt;
                }
            } else if let Some(mut item) = ov
                .item
                .clone()
                .or_else(|| self.snapshot_feed_item_for_post(pid))
            {
                item.post.viewer_bookmarked = true;
                item.post.viewer_bookmark_folder = ov.folder.clone();
                if let Some(cnt) = ov.bookmarks_count {
                    item.post.bookmarks_count = cnt;
                }
                fresh.insert(0, item);
            }
        }

        // Step 2 (Order 1): apply currently in-flight or dirty optimistic bookmark targets.
        for (pid, (target_bookmarked, target_folder)) in &self.bookmark_pending_target {
            if !self.bookmark_in_flight.contains_key(pid) && !self.bookmark_dirty.contains(pid) {
                continue;
            }
            if !*target_bookmarked {
                fresh.retain(|it| it.post.post_id != *pid);
            } else if let Some(existing) = fresh.iter_mut().find(|it| it.post.post_id == *pid) {
                if !existing.post.viewer_bookmarked {
                    existing.post.bookmarks_count =
                        existing.post.bookmarks_count.saturating_add(1);
                }
                existing.post.viewer_bookmarked = true;
                existing.post.viewer_bookmark_folder = target_folder.clone();
            } else if let Some(mut item) = self.snapshot_feed_item_for_post(pid) {
                item.post.viewer_bookmarked = true;
                item.post.viewer_bookmark_folder = target_folder.clone();
                fresh.insert(0, item);
            }
        }

        self.saved_items = fresh;
        if !self.saved_loading {
            self.bookmark_recent_overrides.clear();
        }
    }
}
