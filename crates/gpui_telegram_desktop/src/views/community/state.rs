//! Community workspace ViewModels, route stack, responsive breakpoints, and optimistic mutations.

use std::collections::{HashMap, HashSet};

use gpui_telegram_client::CommunityRuntimeMode;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorProfileResponse, AlgoPreferencesDto, AwardResponse, BookmarkResponse,
    BotAutonomyMode, CommentSort, CommunityActorDto, CommunityDetailResponse, CommunityListDto,
    CommunityNotificationDto, CommunityPostDto, CommunitySearchResultDto, CommunitySearchTab,
    FeedItemDto, FeedSort, FeedTab, FollowActionResponse, JoinCommunityResponse, ModLogEntryDto,
    ModQueueItemDto, PollDto, PostDetailResponse, RepostResponse, SubCommunityDto, TimeWindow,
    ToggleActionResponse, TrendingTopicDto, VoteResponse,
};

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
}

impl ActiveCommunityInput {
    pub fn allows_newlines(self) -> bool {
        matches!(
            self,
            Self::QuickPostBody
                | Self::DetailReply
                | Self::QuotePostNote
                | Self::BotStudioPersona
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

/// Active floating popover overlay rendered on top of the Community scroll containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommunityPopover {
    IdentitySwitcher,
    AlgoTuner,
    ExplainPost(String),
    AwardPost(String),
    QuickPostCommunityPicker,
    QuotePost(String),
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

    // Card interactive states
    pub active_popover: Option<CommunityPopover>,
    pub card_child_click_consumed: bool,
    pub revealed_spoilers: HashSet<(u64, usize)>,
    pub revealed_post_veils: HashSet<String>,
    pub expanded_quotes: HashSet<(u64, usize)>,
    pub copied_code_msg: Option<u64>,
    pub poll_breakdown_by_post: HashMap<String, PollBreakdownMode>,
    pub hidden_post_ids: HashSet<String>,

    // Left Nav & Right Rail caches
    pub communities: Vec<SubCommunityDto>,
    pub lists: Vec<CommunityListDto>,
    pub trending: Vec<TrendingTopicDto>,
    pub recommended_actors: Vec<CommunityActorDto>,
    pub unread_notifications: u32,

    // Secondary route live caches
    pub detail_post: Option<PostDetailResponse>,
    pub detail_comment_sort: CommentSort,
    pub detail_loading: bool,
    pub detail_error: Option<String>,
    pub detail_gen: u64,

    pub community_detail: Option<CommunityDetailResponse>,
    pub community_posts: Vec<FeedItemDto>,
    pub community_loading: bool,
    pub community_error: Option<String>,
    pub community_gen: u64,

    pub actor_profile: Option<ActorProfileResponse>,
    pub profile_loading: bool,
    pub profile_error: Option<String>,
    pub profile_gen: u64,

    pub search_result: Option<CommunitySearchResultDto>,
    pub search_tab: CommunitySearchTab,
    pub search_loading: bool,
    pub search_gen: u64,

    pub notifications: Vec<CommunityNotificationDto>,
    pub notifications_loading: bool,
    pub notifications_gen: u64,

    pub saved_items: Vec<FeedItemDto>,
    pub saved_loading: bool,
    pub saved_gen: u64,

    pub mod_slug: Option<String>,
    pub mod_queue: Vec<ModQueueItemDto>,
    pub mod_logs: Vec<ModLogEntryDto>,
    pub mod_loading: bool,
    pub mod_gen: u64,

    pub bot_studio_name: String,
    pub bot_studio_handle: String,
    pub bot_studio_persona: String,
    pub bot_studio_shape_idx: usize,
    pub bot_studio_color_idx: usize,
    pub bot_studio_autonomy: BotAutonomyMode,
    pub bot_studio_submitting: bool,
    pub bot_studio_last_token: Option<String>,
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
            card_child_click_consumed: false,
            revealed_spoilers: HashSet::new(),
            revealed_post_veils: HashSet::new(),
            expanded_quotes: HashSet::new(),
            copied_code_msg: None,
            poll_breakdown_by_post: HashMap::new(),
            hidden_post_ids: HashSet::new(),
            communities: Vec::new(),
            lists: Vec::new(),
            trending: Vec::new(),
            recommended_actors: Vec::new(),
            unread_notifications: 0,
            detail_post: None,
            detail_comment_sort: CommentSort::Best,
            detail_loading: false,
            detail_error: None,
            detail_gen: 0,
            community_detail: None,
            community_posts: Vec::new(),
            community_loading: false,
            community_error: None,
            community_gen: 0,
            actor_profile: None,
            profile_loading: false,
            profile_error: None,
            profile_gen: 0,
            search_result: None,
            search_tab: CommunitySearchTab::Top,
            search_loading: false,
            search_gen: 0,
            notifications: Vec::new(),
            notifications_loading: false,
            notifications_gen: 0,
            saved_items: Vec::new(),
            saved_loading: false,
            saved_gen: 0,
            mod_slug: None,
            mod_queue: Vec::new(),
            mod_logs: Vec::new(),
            mod_loading: false,
            mod_gen: 0,
            bot_studio_name: String::new(),
            bot_studio_handle: String::new(),
            bot_studio_persona: String::new(),
            bot_studio_shape_idx: 0,
            bot_studio_color_idx: 0,
            bot_studio_autonomy: BotAutonomyMode::MentionOnly,
            bot_studio_submitting: false,
            bot_studio_last_token: None,
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
        self.active_popover = None;
    }

    pub fn replace_route(&mut self, next: CommunityRoute) {
        self.route_stack.clear();
        self.route = next;
        self.active_popover = None;
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.route_stack.pop() {
            self.route = prev;
            self.active_popover = None;
            true
        } else if self.route != CommunityRoute::Feed {
            self.route = CommunityRoute::Feed;
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
    fn visit_post_mut(&mut self, post_id: &str, mut f: impl FnMut(&mut CommunityPostDto)) {
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
        for item in &mut self.saved_items {
            if item.post.post_id == post_id {
                f(&mut item.post);
            }
        }
        if let Some(ref mut det) = self.detail_post {
            if det.post.post_id == post_id {
                f(&mut det.post);
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
            .chain(self.saved_items.iter().map(|i| &i.post))
            .chain(self.detail_post.iter().map(|d| &d.post))
            .chain(self.actor_profile.iter().flat_map(|p| p.recent_posts.iter()))
            .chain(self.search_result.iter().flat_map(|s| s.posts.iter()))
            .find(|p| p.post_id == post_id)
    }

    pub fn find_feed_item(&self, post_id: &str) -> Option<&FeedItemDto> {
        self.feed_items
            .iter()
            .chain(self.community_posts.iter())
            .chain(self.saved_items.iter())
            .find(|i| i.post.post_id == post_id)
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
        self.visit_post_mut(&resp.post_id, |post| {
            post.viewer_bookmarked = resp.bookmarked;
            post.viewer_bookmark_folder = if resp.bookmarked && !resp.folder.trim().is_empty() {
                Some(resp.folder.clone())
            } else {
                None
            };
            post.bookmarks_count = resp.bookmarks_count;
        });
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
    }
}
