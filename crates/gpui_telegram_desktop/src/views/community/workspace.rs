//! Root `Entity<CommunityWorkspace>`: 3/2/1-column adaptive shell, Tokio async facade bridge,
//! centralized English + CJK IME contract, progressive Escape cascade, popover overlay layer,
//! and genuine live-data secondary routes (`PostDetail`, `CommunityPage`, `ActorProfile`,
//! `Explore`, `Notifications`, `BookmarksLists`, `ModQueue`, `BotStudio`).

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::{
    CommunityClientFacade, TelegramBotSyncInput, TelegramClient, TelegramHumanSyncInput,
};
use gpui_telegram_proto::community_contracts::{
    ActorFilter, BookmarkRequest, BotAutonomyMode, CommentSort, CommunityCommentDto,
    CommunityIdentitiesResponse, CommunityPollVoteRequest, CommunitySearchRequestDto,
    CommunitySearchTab, CreateCommentRequest, CreatePostRequest, EngagementSignalKind, FeedItemDto,
    FeedRequestDto, FeedSort, FeedTab, GiveAwardRequest, MarkNotificationsReadRequest,
    ModActionKind, ModActionRequest, PostFormat, PostStatus, RecordSignalRequest,
    RegisterBotActorRequest, RepostRequest, SummonBotRequest, TimeWindow, VoteRequest,
};

use crate::components::grok_bot_avatar::GROK_COLOR_PALETTE;
use crate::components::ui::text_input_controller::{
    delete_prev_word, pop_last_char, sanitize_paste_text,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::stage::entities_render::RichTextAction;

use super::feed_view::{
    render_algo_tuner_popover, render_feed_view, render_quick_post_community_picker_popover,
};
use super::left_nav::{render_compact_top_nav, render_left_nav};
use super::post_card::{
    render_award_popover, render_explain_popover, render_post_card, render_quote_popover,
};
use super::right_rail::render_right_rail;
use super::shared_widgets::{
    format_compact_count, format_relative_time, render_actor_avatar, render_actor_badge_row,
    render_empty_state, render_flair_chip, render_identity_switcher_popover, render_input_display,
    render_loading_skeleton,
};
use super::state::{
    sanitize_algo_prefs, ActiveCommunityInput, CommunityLayoutMode, CommunityPopover,
    CommunityRoute, CommunityWorkspaceAction, CommunityWorkspaceState, FeedDensity,
};

const GROK_SHAPE_CHOICES: [&str; 8] = [
    "blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop",
];

async fn run_on_community_tokio<F, T>(
    handle: &tokio::runtime::Handle,
    fut: F,
) -> Result<T, String>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    handle.spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx.await
        .map_err(|_| "community tokio task cancelled".to_string())
}

/// Root GPUI desktop view for the fused Reddit + X Community workspace.
pub struct CommunityWorkspace {
    pub facade: CommunityClientFacade,
    pub media_base_url: String,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub state: CommunityWorkspaceState,
    pub feed_scroll_handle: ScrollHandle,
    pub left_nav_scroll_handle: ScrollHandle,
    pub right_rail_scroll_handle: ScrollHandle,
    pub detail_scroll_handle: ScrollHandle,
    pub focus_handle: FocusHandle,
    pub host_sidebar_width_px: f32,
    pub last_layout_mode: CommunityLayoutMode,
    pending_actions: Vec<CommunityWorkspaceAction>,
}

impl CommunityWorkspace {
    pub fn new(
        client: Arc<TelegramClient>,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let facade = client.community().clone();
        let media_base_url = client.http_client.base_url.clone();
        let mut state = CommunityWorkspaceState::default();
        state.runtime_mode = facade.runtime_mode();
        if let Some(aid) = facade.active_actor_id() {
            state.active_actor_id = aid;
        }
        let mut ws = Self {
            facade,
            media_base_url,
            palette,
            language_id: language_id.into(),
            state,
            feed_scroll_handle: ScrollHandle::new(),
            left_nav_scroll_handle: ScrollHandle::new(),
            right_rail_scroll_handle: ScrollHandle::new(),
            detail_scroll_handle: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            host_sidebar_width_px: 288.0,
            last_layout_mode: CommunityLayoutMode::ThreeColumn,
            pending_actions: Vec::new(),
        };
        ws.bootstrap_initial_load(cx);
        ws
    }

    pub fn emit_action(&mut self, action: CommunityWorkspaceAction, cx: &mut Context<Self>) {
        self.pending_actions.push(action);
        cx.notify();
    }

    pub fn take_pending_actions(&mut self) -> Vec<CommunityWorkspaceAction> {
        std::mem::take(&mut self.pending_actions)
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang_id: impl Into<String>) {
        self.language_id = lang_id.into();
    }

    pub fn sync_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        lang_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.palette = palette;
        self.language_id = lang_id.into();
        cx.notify();
    }

    pub fn set_host_sidebar_width(&mut self, width_px: f32) {
        self.host_sidebar_width_px = width_px.max(0.0);
    }

    pub fn apply_identities_response(
        &mut self,
        resp: CommunityIdentitiesResponse,
        cx: &mut Context<Self>,
    ) {
        let prev_active = self.state.active_actor_id.clone();
        if !resp.active_actor_id.is_empty() {
            self.state.active_actor_id = resp.active_actor_id.clone();
            self.facade
                .set_active_actor_id(Some(resp.active_actor_id.clone()));
        } else if let Some(ref h) = resp.human_actor {
            self.state.active_actor_id = h.actor_id.clone();
            self.facade.set_active_actor_id(Some(h.actor_id.clone()));
        }
        self.state.identities = Some(resp);
        self.state.runtime_mode = self.facade.runtime_mode();
        if prev_active != self.state.active_actor_id && !self.state.active_actor_id.is_empty() {
            self.load_feed(cx);
        }
        cx.notify();
    }

    /// Synchronously seeds known owned Human/Bot actor IDs (e.g. from `AppShell`) so
    /// [`Self::is_viewer_owned_actor`] recognizes owned Bots immediately during cold mount
    /// before async `sync_identities` resolves.
    pub fn set_known_owned_actor_ids(&mut self, ids: impl IntoIterator<Item = String>) {
        self.state.known_owned_actor_ids.clear();
        for id in ids {
            let trimmed = id.trim();
            if !trimmed.is_empty() {
                self.state.known_owned_actor_ids.insert(trimmed.to_string());
            }
        }
    }

    pub fn sync_identities(
        &mut self,
        human: TelegramHumanSyncInput,
        owned_bots: Vec<TelegramBotSyncInput>,
        cx: &mut Context<Self>,
    ) {
        if human.user_id > 0 {
            self.state
                .known_owned_actor_ids
                .insert(format!("u:{}", human.user_id));
        }
        for b in &owned_bots {
            if b.bot_user_id > 0 {
                self.state
                    .known_owned_actor_ids
                    .insert(format!("b:{}", b.bot_user_id));
            }
        }
        self.state.identities_gen = self.state.identities_gen.wrapping_add(1);
        let my_id_gen = self.state.identities_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.sync_identities(&human, &owned_bots).await
            })
            .await;
            if let Ok(Ok(ids)) = res {
                let _ = weak.update(app, |this, cx| {
                    if this.state.identities_gen != my_id_gen {
                        return;
                    }
                    this.apply_identities_response(ids, cx);
                });
            }
        })
        .detach();
    }
}

impl CommunityWorkspace {
    /// Loads identities, algo prefs, initial feed, communities, lists, trending, notifications,
    /// and recommended Human+Bot actors in parallel on Tokio without blocking the GPUI thread.
    pub fn bootstrap_initial_load(&mut self, cx: &mut Context<Self>) {
        self.load_feed(cx);
        self.state.identities_gen = self.state.identities_gen.wrapping_add(1);
        let my_id_gen = self.state.identities_gen;
        self.state.algo_prefs_gen = self.state.algo_prefs_gen.wrapping_add(1);
        let my_algo_gen = self.state.algo_prefs_gen;
        self.state.notifications_gen = self.state.notifications_gen.wrapping_add(1);
        let my_notif_gen = self.state.notifications_gen;
        self.state.notifications_loading = false;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let ids = facade.get_identities().await.ok();
                let prefs = facade.get_algo_prefs().await.ok();
                let comms = facade.list_communities(None, Some(40)).await.ok();
                let lists = facade.list_lists().await.ok();
                let trending = facade.get_trending(Some(10)).await.ok();
                let notifs = facade.get_notifications(None, Some(30), None).await.ok();
                let bot_search = facade
                    .search(&CommunitySearchRequestDto {
                        query: String::new(),
                        tab: CommunitySearchTab::Bots,
                        limit: 8,
                        ..Default::default()
                    })
                    .await
                    .ok();
                let people_search = facade
                    .search(&CommunitySearchRequestDto {
                        query: String::new(),
                        tab: CommunitySearchTab::People,
                        limit: 8,
                        ..Default::default()
                    })
                    .await
                    .ok();
                let mode = facade.runtime_mode();
                (ids, prefs, comms, lists, trending, notifs, bot_search, people_search, mode)
            })
            .await;

            if let Ok((ids, prefs, comms, lists, trending, notifs, bots, people, mode)) = res {
                let _ = weak.update(app, |this, cx| {
                    this.state.runtime_mode = mode;
                    if this.state.identities_gen == my_id_gen {
                        if let Some(id_resp) = ids {
                            if this.state.active_actor_id.is_empty()
                                && !id_resp.active_actor_id.is_empty()
                            {
                                this.state.active_actor_id = id_resp.active_actor_id.clone();
                                this.facade
                                    .set_active_actor_id(Some(id_resp.active_actor_id.clone()));
                            }
                            this.state.identities = Some(id_resp);
                        }
                    }
                    if this.state.algo_prefs_gen == my_algo_gen {
                        if let Some(p) = prefs {
                            let clean = sanitize_algo_prefs(&p);
                            this.state.algo_prefs = clean.clone();
                            this.state.algo_draft = clean;
                        }
                    }
                    if let Some(c) = comms {
                        this.state.communities = c.communities;
                    }
                    if let Some(l) = lists {
                        this.state.lists = l.lists;
                    }
                    if let Some(t) = trending {
                        this.state.trending = t.topics;
                    }
                    if this.state.notifications_gen == my_notif_gen {
                        this.state.notifications_loading = false;
                        if let Some(n) = notifs {
                            let unread = n.unread_count.min(u32::MAX as u64) as u32;
                            this.state.notifications = n.notifications;
                            this.state.unread_notifications = unread;
                            this.emit_action(
                                CommunityWorkspaceAction::UnreadCountChanged(unread),
                                cx,
                            );
                        }
                    }
                    let mut rec = Vec::new();
                    if let Some(b) = bots {
                        rec.extend(b.actors);
                    }
                    if let Some(p) = people {
                        for a in p.actors {
                            if !rec.iter().any(|x| x.actor_id == a.actor_id) {
                                rec.push(a);
                            }
                        }
                    }
                    if !rec.is_empty() {
                        this.state.recommended_actors = rec;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Returns `true` if `actor_id` belongs to the viewer's active identity, human identity,
    /// or any owned Bot identity (preventing self-authored WS events from inflating unread badges).
    pub fn is_viewer_owned_actor(&self, actor_id: &str) -> bool {
        let aid = actor_id.trim();
        if aid.is_empty() {
            return false;
        }
        if !self.state.active_actor_id.is_empty() && aid == self.state.active_actor_id {
            return true;
        }
        if self.state.known_owned_actor_ids.contains(aid) {
            return true;
        }
        if let Some(ref ids) = self.state.identities {
            if !ids.active_actor_id.is_empty() && aid == ids.active_actor_id {
                return true;
            }
            if ids.human_actor.as_ref().is_some_and(|h| h.actor_id == aid) {
                return true;
            }
            if ids.bot_actors.iter().any(|b| b.actor_id == aid) {
                return true;
            }
            if ids.all_actors.iter().any(|a| a.actor_id == aid) {
                return true;
            }
        }
        false
    }

    /// Handles a live WebSocket or local runtime `CommunityEvent`.
    #[allow(clippy::too_many_arguments)]
    pub fn on_realtime_event(
        &mut self,
        event_kind: &str,
        _community_id: Option<&str>,
        post_id: Option<&str>,
        _comment_id: Option<&str>,
        actor_id: Option<&str>,
        payload_json: &str,
        _timestamp_ms: i64,
        cx: &mut Context<Self>,
    ) {
        let kind = event_kind.trim().to_ascii_lowercase();
        if kind.is_empty() {
            return;
        }
        let _parsed = serde_json::from_str::<serde_json::Value>(payload_json).ok();
        let is_self = actor_id.is_some_and(|a| self.is_viewer_owned_actor(a));

        match kind.as_str() {
            "post_created" | "post_published" => {
                if !is_self {
                    self.state.pending_ws_new_posts =
                        self.state.pending_ws_new_posts.saturating_add(1);
                }
            }
            "mention" | "reply" | "comment_created" | "award" | "notification"
            | "notification_created" | "bot_replied" | "bot_reply" | "follow" => {
                if !is_self {
                    self.state.unread_notifications =
                        self.state.unread_notifications.saturating_add(1);
                    let next = self.state.unread_notifications;
                    self.emit_action(CommunityWorkspaceAction::UnreadCountChanged(next), cx);
                }
                if let (CommunityRoute::PostDetail(open_pid), Some(ev_pid)) =
                    (&self.state.route, post_id)
                {
                    if open_pid == ev_pid {
                        let pid = open_pid.clone();
                        self.load_post_detail(pid, cx);
                    }
                }
            }
            "vote_updated" | "poll_voted" => {
                if let (CommunityRoute::PostDetail(open_pid), Some(ev_pid)) =
                    (&self.state.route, post_id)
                {
                    if open_pid == ev_pid {
                        let pid = open_pid.clone();
                        self.load_post_detail(pid, cx);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    // -------------------------------------------------------------------------
    // Centralized English + CJK IME & Progressive Escape Contract
    // -------------------------------------------------------------------------

    pub fn has_focused_text_input(&self) -> bool {
        self.state.active_input != ActiveCommunityInput::None
    }

    pub fn active_input_text(&self) -> String {
        self.state.active_input_text().to_string()
    }

    pub fn active_input_ime_preedit(&self) -> Option<String> {
        self.state.ime_preedit.clone()
    }

    pub fn set_ime_preedit(&mut self, preedit: Option<&str>, cx: &mut Context<Self>) {
        self.state.ime_preedit = preedit
            .filter(|p| !p.is_empty())
            .map(ToString::to_string);
        cx.notify();
    }

    pub fn active_input_generation(&self) -> u64 {
        self.state.input_generation
    }

    pub fn clear_active_input(&mut self, cx: &mut Context<Self>) {
        self.state.ime_preedit = None;
        if let Some(buf) = self.state.active_input_text_mut() {
            buf.clear();
        }
        cx.notify();
    }

    pub fn blur_active_input(&mut self, cx: &mut Context<Self>) {
        self.state.blur_input();
        cx.notify();
    }

    pub fn insert_text_to_active_input(&mut self, raw_text: &str, cx: &mut Context<Self>) -> bool {
        if self.state.active_input == ActiveCommunityInput::None {
            return false;
        }
        let allow_nl = self.state.active_input.allows_newlines();
        let clean = if allow_nl {
            sanitize_paste_text(raw_text, true)
        } else {
            let flattened = raw_text
                .replace("\r\n", " ")
                .replace(['\r', '\n'], " ");
            sanitize_paste_text(&flattened, false)
        };
        self.state.ime_preedit = None;
        if clean.is_empty() {
            cx.notify();
            return true;
        }
        if self.state.active_input == ActiveCommunityInput::QuickPostBody {
            self.state.quick_post_validation_error = None;
        }
        if let Some(buf) = self.state.active_input_text_mut() {
            buf.push_str(&clean);
        }
        cx.notify();
        true
    }

    pub fn handle_non_char_key_input(
        &mut self,
        key: &str,
        is_shift: bool,
        is_alt: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = self.state.active_input;
        if target == ActiveCommunityInput::None {
            return false;
        }
        match key {
            "backspace" => {
                if let Some(buf) = self.state.active_input_text_mut() {
                    if is_alt {
                        delete_prev_word(buf);
                    } else {
                        pop_last_char(buf);
                    }
                }
                cx.notify();
                true
            }
            "space" => {
                if let Some(buf) = self.state.active_input_text_mut() {
                    buf.push(' ');
                }
                cx.notify();
                true
            }
            "enter" => {
                if is_shift && target.allows_newlines() {
                    if let Some(buf) = self.state.active_input_text_mut() {
                        buf.push('\n');
                    }
                    cx.notify();
                    return true;
                }
                match target {
                    ActiveCommunityInput::QuickPostBody | ActiveCommunityInput::QuickPostTitle => {
                        self.submit_quick_post(cx);
                    }
                    ActiveCommunityInput::RightRailSearch => {
                        let q = self.state.right_rail_search_draft.trim().to_string();
                        self.open_explore_with_query(q, cx);
                    }
                    ActiveCommunityInput::ExploreSearch => {
                        let q = self.state.explore_search_draft.trim().to_string();
                        self.execute_search(q, self.state.search_tab, cx);
                    }
                    ActiveCommunityInput::DetailReply => {
                        self.submit_detail_reply(cx);
                    }
                    ActiveCommunityInput::QuotePostNote => {
                        if let Some(CommunityPopover::QuotePost(pid)) =
                            self.state.active_popover.clone()
                        {
                            self.submit_quote_post(pid, cx);
                        }
                    }
                    ActiveCommunityInput::BotStudioName
                    | ActiveCommunityInput::BotStudioHandle
                    | ActiveCommunityInput::BotStudioPersona => {
                        self.submit_register_bot(cx);
                    }
                    ActiveCommunityInput::None => return false,
                }
                true
            }
            "tab" => {
                let next = match target {
                    ActiveCommunityInput::QuickPostTitle => ActiveCommunityInput::QuickPostBody,
                    ActiveCommunityInput::QuickPostBody => ActiveCommunityInput::QuickPostTitle,
                    ActiveCommunityInput::BotStudioName => ActiveCommunityInput::BotStudioHandle,
                    ActiveCommunityInput::BotStudioHandle => ActiveCommunityInput::BotStudioPersona,
                    ActiveCommunityInput::BotStudioPersona => ActiveCommunityInput::BotStudioName,
                    other => other,
                };
                self.state.focus_input(next);
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// Progressive Escape cascade inside Community:
    /// 1) Close open Community popover (Algo Tuner / Identity Switcher / Explain / Award / Quote)
    /// 2) Blur focused Community text input
    /// 3) Return `false` so `AppShell::dismiss_post_auth_escape` exits Community back to `Chats`.
    pub fn handle_escape_key(&mut self, cx: &mut Context<Self>) -> bool {
        if self.state.active_popover.is_some() {
            self.state.active_popover = None;
            if self.state.active_input == ActiveCommunityInput::QuotePostNote {
                self.state.blur_input();
            }
            cx.notify();
            return true;
        }
        if self.state.active_input != ActiveCommunityInput::None {
            self.state.blur_input();
            cx.notify();
            return true;
        }
        false
    }
}

impl CommunityWorkspace {
    // -------------------------------------------------------------------------
    // Navigation & Feed Controls
    // -------------------------------------------------------------------------

    pub fn navigate_to(&mut self, route: CommunityRoute, cx: &mut Context<Self>) {
        self.state.blur_input();
        self.state.push_route(route.clone());
        self.load_data_for_route(&route, cx);
        cx.notify();
    }

    pub fn replace_route(&mut self, route: CommunityRoute, cx: &mut Context<Self>) {
        self.state.blur_input();
        self.state.replace_route(route.clone());
        self.load_data_for_route(&route, cx);
        cx.notify();
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        self.state.blur_input();
        if self.state.go_back() {
            let cur = self.state.route.clone();
            self.load_data_for_route(&cur, cx);
        }
        cx.notify();
    }

    fn load_data_for_route(&mut self, route: &CommunityRoute, cx: &mut Context<Self>) {
        match route {
            CommunityRoute::Feed => {
                if self.state.feed_items.is_empty() {
                    self.load_feed(cx);
                }
            }
            CommunityRoute::PostDetail(pid) => self.load_post_detail(pid.clone(), cx),
            CommunityRoute::CommunityPage(slug) => self.load_community_page(slug.clone(), cx),
            CommunityRoute::ActorProfile(handle) => self.load_actor_profile(handle.clone(), cx),
            CommunityRoute::Explore { initial_query } => {
                let q = initial_query.clone().unwrap_or_default();
                self.state.explore_search_draft = q.clone();
                self.execute_search(q, self.state.search_tab, cx);
            }
            CommunityRoute::Notifications => self.load_notifications(cx),
            CommunityRoute::BookmarksLists => self.load_saved_posts(cx),
            CommunityRoute::ModQueue(slug) => {
                let target = slug
                    .clone()
                    .or_else(|| self.state.mod_slug.clone())
                    .or_else(|| self.state.communities.first().map(|c| c.slug.clone()));
                if let Some(s) = target {
                    self.load_mod_queue(s, cx);
                }
            }
            CommunityRoute::BotStudio => {}
        }
    }

    pub fn toggle_popover(&mut self, pop: CommunityPopover, cx: &mut Context<Self>) {
        if self.state.active_popover.as_ref() == Some(&pop) {
            self.state.active_popover = None;
        } else {
            self.state.active_popover = Some(pop);
        }
        if !matches!(
            self.state.active_popover,
            Some(CommunityPopover::QuotePost(_))
        ) && self.state.active_input == ActiveCommunityInput::QuotePostNote
        {
            self.state.blur_input();
        }
        cx.notify();
    }

    pub fn open_quick_post_composer(&mut self, cx: &mut Context<Self>) {
        if self.state.route != CommunityRoute::Feed {
            self.state.replace_route(CommunityRoute::Feed);
        }
        self.state.active_popover = None;
        self.state.quick_post_expanded = true;
        self.state.quick_post_validation_error = None;
        self.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    }

    pub fn open_post_detail(&mut self, post_id: String, cx: &mut Context<Self>) {
        self.navigate_to(CommunityRoute::PostDetail(post_id), cx);
    }

    pub fn open_actor_profile(&mut self, handle_or_id: String, cx: &mut Context<Self>) {
        self.navigate_to(CommunityRoute::ActorProfile(handle_or_id), cx);
    }

    pub fn open_explore_with_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.navigate_to(
            CommunityRoute::Explore {
                initial_query: Some(query),
            },
            cx,
        );
    }

    pub fn select_feed_tab(&mut self, tab: FeedTab, cx: &mut Context<Self>) {
        self.state.active_tab = tab;
        if self.state.route != CommunityRoute::Feed {
            self.state.replace_route(CommunityRoute::Feed);
        }
        self.load_feed(cx);
    }

    pub fn set_actor_filter(&mut self, filter: ActorFilter, cx: &mut Context<Self>) {
        if self.state.actor_filter != filter {
            self.state.actor_filter = filter;
            self.load_feed(cx);
        }
    }

    pub fn set_feed_sort(&mut self, sort: FeedSort, cx: &mut Context<Self>) {
        if self.state.sort != sort {
            self.state.sort = sort;
            self.load_feed(cx);
        }
    }

    pub fn set_time_window(&mut self, tw: TimeWindow, cx: &mut Context<Self>) {
        if self.state.time_window != tw {
            self.state.time_window = tw;
            self.load_feed(cx);
        }
    }

    pub fn cycle_density(&mut self, cx: &mut Context<Self>) {
        self.state.density = match self.state.density {
            FeedDensity::Card => FeedDensity::Compact,
            FeedDensity::Compact => FeedDensity::Classic,
            FeedDensity::Classic => FeedDensity::Card,
        };
        cx.notify();
    }

    pub fn refresh_feed_from_ws_banner(&mut self, cx: &mut Context<Self>) {
        self.state.pending_ws_new_posts = 0;
        self.feed_scroll_handle.set_offset(point(px(0.0), px(0.0)));
        self.load_feed(cx);
    }

    pub fn load_feed(&mut self, cx: &mut Context<Self>) {
        self.state.feed_gen = self.state.feed_gen.wrapping_add(1);
        let my_gen = self.state.feed_gen;
        self.state.feed_loading = true;
        self.state.feed_loading_more = false;
        self.state.feed_error = None;
        let req = FeedRequestDto {
            tab: self.state.active_tab.clone(),
            sort: self.state.sort,
            time_window: self.state.time_window,
            actor_filter: self.state.actor_filter,
            flair: self.state.active_flair.clone(),
            cursor: None,
            limit: 25,
            algo_override: Some(self.state.algo_prefs.clone()),
            now_ms: None,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let r = facade.get_feed(&req).await;
                let mode = facade.runtime_mode();
                (r, mode)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.feed_gen != my_gen {
                    return;
                }
                this.state.feed_loading = false;
                match res {
                    Ok((Ok(resp), mode)) => {
                        this.state.runtime_mode = mode;
                        this.state.feed_items = resp.items;
                        this.state.next_cursor = resp.next_cursor;
                        this.state.feed_error = None;
                    }
                    Ok((Err(e), mode)) => {
                        this.state.runtime_mode = mode;
                        this.state.feed_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.feed_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_more_feed(&mut self, cx: &mut Context<Self>) {
        if self.state.feed_loading || self.state.feed_loading_more {
            return;
        }
        let Some(cursor) = self.state.next_cursor.clone() else {
            return;
        };
        let my_gen = self.state.feed_gen;
        self.state.feed_loading_more = true;
        let req = FeedRequestDto {
            tab: self.state.active_tab.clone(),
            sort: self.state.sort,
            time_window: self.state.time_window,
            actor_filter: self.state.actor_filter,
            flair: self.state.active_flair.clone(),
            cursor: Some(cursor),
            limit: 25,
            algo_override: Some(self.state.algo_prefs.clone()),
            now_ms: None,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move { facade.get_feed(&req).await })
                .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.feed_gen != my_gen {
                    return;
                }
                this.state.feed_loading_more = false;
                if let Ok(Ok(resp)) = res {
                    for it in resp.items {
                        if !this
                            .state
                            .feed_items
                            .iter()
                            .any(|x| x.post.post_id == it.post.post_id)
                        {
                            this.state.feed_items.push(it);
                        }
                    }
                    this.state.next_cursor = resp.next_cursor;
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl CommunityWorkspace {
    // -------------------------------------------------------------------------
    // Algo Prefs, Quick-Post & Post Interactions
    // -------------------------------------------------------------------------

    pub fn save_algo_prefs(&mut self, cx: &mut Context<Self>) {
        let sanitized = sanitize_algo_prefs(&self.state.algo_draft);
        self.state.algo_draft = sanitized.clone();
        self.state.algo_prefs = sanitized.clone();
        self.state.active_popover = None;
        self.state.algo_prefs_gen = self.state.algo_prefs_gen.wrapping_add(1);
        let my_algo_gen = self.state.algo_prefs_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        self.load_feed(cx);
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_algo_prefs(&sanitized).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.algo_prefs_gen != my_algo_gen {
                    return;
                }
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(saved)) => {
                        let clean = sanitize_algo_prefs(&saved);
                        this.state.algo_prefs = clean.clone();
                        this.state.algo_draft = clean;
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(&lang, "算法偏好已更新", "Algorithm preferences saved")
                                    .to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    pub fn submit_quick_post(&mut self, cx: &mut Context<Self>) {
        if self.state.quick_post_submitting {
            return;
        }
        let body = self.state.quick_post_body.trim().to_string();
        let title = {
            let t = self.state.quick_post_title.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        };
        if body.is_empty() && title.is_none() {
            let lang = self.language_id.as_str();
            self.state.quick_post_validation_error = Some(
                tr_str(lang, "请输入帖子正文或标题后再发布", "Post content cannot be empty")
                    .to_string(),
            );
            cx.notify();
            return;
        }
        self.state.quick_post_validation_error = None;
        self.state.quick_post_submitting = true;
        let req = CreatePostRequest {
            community_slug: self.state.quick_post_community.clone(),
            format: if title.is_some() {
                PostFormat::Article
            } else {
                PostFormat::Micro
            },
            title,
            body,
            status: PostStatus::Published,
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res =
                run_on_community_tokio(&handle, async move { facade.create_post(&req).await }).await;
            let _ = weak.update(app, |this, cx| {
                this.state.quick_post_submitting = false;
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(created)) => {
                        this.state.quick_post_title.clear();
                        this.state.quick_post_body.clear();
                        this.state.quick_post_expanded = false;
                        this.state.blur_input();
                        this.state.feed_items.insert(
                            0,
                            FeedItemDto {
                                post: created.post,
                                explanation: None,
                                rank_score: 100.0,
                                is_exploration_slot: false,
                            },
                        );
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(&lang, "帖子已发布", "Post published").to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        this.state.quick_post_validation_error = Some(e.to_string());
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(e) => {
                        this.state.quick_post_validation_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn vote_post(&mut self, post_id: String, clicked_dir: i8, cx: &mut Context<Self>) {
        let (next_dir, rollback) = self.state.apply_optimistic_vote(&post_id, clicked_dir);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .vote_post(&pid, &VoteRequest { direction: next_dir })
                    .await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => this.state.reconcile_vote(&resp),
                    _ => {
                        if let Some(snap) = rollback {
                            this.state.rollback_vote(&post_id, snap);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toggle_like(&mut self, post_id: String, cx: &mut Context<Self>) {
        let rollback = self.state.apply_optimistic_like(&post_id);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.toggle_post_like(&pid).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => this.state.reconcile_like(&resp),
                    _ => {
                        if let Some(snap) = rollback {
                            this.state.rollback_like(&post_id, snap);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn repost_post(&mut self, post_id: String, cx: &mut Context<Self>) {
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.repost(&post_id, &RepostRequest::default()).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.state.reconcile_repost(&resp);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn submit_quote_post(&mut self, quoted_post_id: String, cx: &mut Context<Self>) {
        let note = self.state.quote_post_draft.trim().to_string();
        if note.is_empty() {
            return;
        }
        self.state.active_popover = None;
        self.state.quote_post_draft.clear();
        self.state.blur_input();
        let req = CreatePostRequest {
            format: PostFormat::Quote,
            body: note,
            quoted_post_id: Some(quoted_post_id),
            status: PostStatus::Published,
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res =
                run_on_community_tokio(&handle, async move { facade.create_post(&req).await }).await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(created)) => {
                        this.state.feed_items.insert(
                            0,
                            FeedItemDto {
                                post: created.post,
                                explanation: None,
                                rank_score: 100.0,
                                is_exploration_slot: false,
                            },
                        );
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl CommunityWorkspace {
    pub fn toggle_bookmark(&mut self, post_id: String, cx: &mut Context<Self>) {
        let already = self
            .state
            .find_post(&post_id)
            .is_some_and(|p| p.viewer_bookmarked);
        let req = BookmarkRequest {
            folder: None,
            remove: Some(already),
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.bookmark_post(&post_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.state.reconcile_bookmark(&resp);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn vote_poll(&mut self, post_id: String, option_id: String, cx: &mut Context<Self>) {
        let option_ids = if let Some(poll) = self
            .state
            .find_post(&post_id)
            .and_then(|p| p.poll.as_ref())
        {
            if poll.multi_select {
                let mut selected = poll.viewer_voted_option_ids.clone();
                if let Some(pos) = selected.iter().position(|id| id == &option_id) {
                    if selected.len() > 1 {
                        selected.remove(pos);
                    }
                } else {
                    selected.push(option_id);
                }
                selected
            } else {
                vec![option_id]
            }
        } else {
            vec![option_id]
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .vote_poll(&pid, &CommunityPollVoteRequest { option_ids })
                    .await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(poll)) => {
                        this.state.reconcile_poll(&post_id, poll);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn give_award(&mut self, post_id: String, award_kind: String, cx: &mut Context<Self>) {
        self.state.active_popover = None;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .give_post_award(
                        &post_id,
                        &GiveAwardRequest {
                            award_kind,
                            message: None,
                        },
                    )
                    .await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.state.reconcile_awards(&resp);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn record_signal(
        &mut self,
        post_id: String,
        kind: EngagementSignalKind,
        target_actor_id: Option<String>,
        community_slug: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.state.active_popover = None;
        if kind == EngagementSignalKind::NotInterested || kind == EngagementSignalKind::HidePost {
            self.state.hidden_post_ids.insert(post_id.clone());
        }
        let lang = self.language_id.clone();
        let toast = match kind {
            EngagementSignalKind::NotInterested => {
                tr_str(&lang, "已减少此类内容推荐", "We will show fewer posts like this")
            }
            EngagementSignalKind::ShowMore => {
                tr_str(&lang, "已记录偏好，将为你推荐更多类似内容", "Got it — we will show more like this")
            }
            _ => tr_str(&lang, "反馈已提交", "Feedback recorded"),
        }
        .to_string();
        self.emit_action(
            CommunityWorkspaceAction::ShowToast {
                text: toast,
                is_error: false,
            },
            cx,
        );
        let req = RecordSignalRequest {
            post_id,
            kind,
            dwell_ms: None,
            reason: None,
            target_actor_id,
            community_slug,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |_weak: WeakEntity<Self>, _app: &mut AsyncApp| {
            let _ = run_on_community_tokio(&handle, async move { facade.record_signal(&req).await })
                .await;
        })
        .detach();
    }

    pub fn switch_active_actor(&mut self, actor_id: String, cx: &mut Context<Self>) {
        if actor_id.trim().is_empty() {
            return;
        }
        self.state.active_actor_id = actor_id.clone();
        self.state.active_popover = None;
        self.facade.set_active_actor_id(Some(actor_id));
        let lang = self.language_id.clone();
        let handle = self
            .state
            .active_actor()
            .map(|a| a.handle.clone())
            .unwrap_or_else(|| self.state.active_actor_id.clone());
        self.emit_action(
            CommunityWorkspaceAction::ShowToast {
                text: if lang.starts_with("zh") {
                    format!("已切换为 @{handle} 身份")
                } else {
                    format!("Now acting as @{handle}")
                },
                is_error: false,
            },
            cx,
        );
        self.load_feed(cx);
        self.load_notifications(cx);
    }

    pub fn toggle_join_community(&mut self, slug: String, join: bool, cx: &mut Context<Self>) {
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                if join {
                    facade.join_community(&slug).await
                } else {
                    facade.leave_community(&slug).await
                }
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.state.reconcile_join(&resp);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toggle_follow_actor(&mut self, actor_id: String, follow: bool, cx: &mut Context<Self>) {
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                if follow {
                    facade.follow_actor(&actor_id).await
                } else {
                    facade.unfollow_actor(&actor_id).await
                }
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                match res {
                    Ok(Ok(resp)) => {
                        this.state.reconcile_follow(&resp);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn handle_rich_text_action(&mut self, action: RichTextAction, cx: &mut Context<Self>) {
        self.state.card_child_click_consumed = true;
        match action {
            RichTextAction::ToggleSpoiler(mid, idx) => {
                if !self.state.revealed_spoilers.remove(&(mid, idx)) {
                    self.state.revealed_spoilers.insert((mid, idx));
                }
                cx.notify();
            }
            RichTextAction::ToggleBlockquote(mid, idx) => {
                if !self.state.expanded_quotes.remove(&(mid, idx)) {
                    self.state.expanded_quotes.insert((mid, idx));
                }
                cx.notify();
            }
            RichTextAction::CopyCode(code) => {
                cx.write_to_clipboard(ClipboardItem::new_string(code));
                let lang = self.language_id.clone();
                self.emit_action(
                    CommunityWorkspaceAction::ShowToast {
                        text: tr_str(&lang, "代码已复制到剪贴板", "Code copied to clipboard")
                            .to_string(),
                        is_error: false,
                    },
                    cx,
                );
            }
            RichTextAction::OpenUrl(url) => {
                if let Some(slug) = url.strip_prefix("tg://community/") {
                    self.navigate_to(CommunityRoute::CommunityPage(slug.to_string()), cx);
                } else {
                    self.emit_action(CommunityWorkspaceAction::OpenExternalUrl(url), cx);
                }
            }
            RichTextAction::ClickMention(mention) => {
                let clean = mention.trim_start_matches('@').to_string();
                self.open_actor_profile(clean, cx);
            }
            RichTextAction::ClickHashtag(tag) => {
                self.open_explore_with_query(tag, cx);
            }
            RichTextAction::ClickBotCommand(cmd) => {
                self.open_explore_with_query(cmd, cx);
            }
        }
    }
}

impl CommunityWorkspace {
    // -------------------------------------------------------------------------
    // Live-Data Secondary Route Async Operations
    // -------------------------------------------------------------------------

    pub fn load_post_detail(&mut self, post_id: String, cx: &mut Context<Self>) {
        self.state.detail_gen = self.state.detail_gen.wrapping_add(1);
        let my_gen = self.state.detail_gen;
        if self
            .state
            .detail_post
            .as_ref()
            .is_some_and(|d| d.post.post_id != post_id)
        {
            self.state.detail_post = None;
        }
        self.state.detail_error = None;
        self.state.detail_loading = true;
        let sort = self.state.detail_comment_sort;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.get_post(&post_id, Some(sort)).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.detail_gen != my_gen {
                    return;
                }
                this.state.detail_loading = false;
                match res {
                    Ok(Ok(det)) => {
                        this.state.detail_post = Some(det);
                        this.state.detail_error = None;
                    }
                    Ok(Err(e)) => {
                        this.state.detail_post = None;
                        this.state.detail_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.detail_post = None;
                        this.state.detail_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn submit_detail_reply(&mut self, cx: &mut Context<Self>) {
        let Some(post_id) = self
            .state
            .detail_post
            .as_ref()
            .map(|d| d.post.post_id.clone())
        else {
            return;
        };
        let body = self.state.detail_reply_draft.trim().to_string();
        if body.is_empty() {
            return;
        }
        let parent_id = self.state.detail_reply_parent_id.take();
        self.state.detail_reply_draft.clear();
        let req = CreateCommentRequest {
            parent_comment_id: parent_id,
            body,
            media: Vec::new(),
            is_mod_distinguished: false,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_pid = post_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.create_comment(&post_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(_)) => {
                    this.load_post_detail(reload_pid, cx);
                }
                Ok(Err(e)) => {
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: e.to_string(),
                            is_error: true,
                        },
                        cx,
                    );
                }
                Err(_) => {}
            });
        })
        .detach();
    }

    pub fn summon_bot_on_post(&mut self, post_id: String, cx: &mut Context<Self>) {
        let bot_id = self
            .state
            .recommended_actors
            .iter()
            .find(|a| a.actor_kind.is_bot_like())
            .map(|a| a.actor_id.clone());
        let req = SummonBotRequest {
            bot_actor_id: bot_id,
            bot_handle: None,
            prompt: Some("Join this discussion with a thoughtful perspective.".to_string()),
            parent_comment_id: None,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_pid = post_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.summon_bot(&post_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(r)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: if r.triggered {
                                    tr_str(&lang, "已召唤 Bot 参与讨论！", "Bot summoned to thread!")
                                        .to_string()
                                } else {
                                    r.reason
                                },
                                is_error: !r.triggered,
                            },
                            cx,
                        );
                        this.load_post_detail(reload_pid, cx);
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    pub fn load_community_page(&mut self, slug: String, cx: &mut Context<Self>) {
        self.state.community_gen = self.state.community_gen.wrapping_add(1);
        let my_gen = self.state.community_gen;
        if self
            .state
            .community_detail
            .as_ref()
            .is_some_and(|d| !d.community.slug.eq_ignore_ascii_case(&slug))
        {
            self.state.community_detail = None;
            self.state.community_posts.clear();
        }
        self.state.community_error = None;
        self.state.community_loading = true;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let detail = facade.get_community(&slug).await;
                let feed = if detail.is_ok() {
                    facade
                        .get_feed(&FeedRequestDto {
                            tab: FeedTab::Community(slug),
                            limit: 25,
                            ..Default::default()
                        })
                        .await
                        .ok()
                } else {
                    None
                };
                (detail, feed)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.community_gen != my_gen {
                    return;
                }
                this.state.community_loading = false;
                match res {
                    Ok((Ok(detail), feed)) => {
                        this.state.community_detail = Some(detail);
                        this.state.community_posts = feed.map(|f| f.items).unwrap_or_default();
                        this.state.community_error = None;
                    }
                    Ok((Err(e), _)) => {
                        this.state.community_detail = None;
                        this.state.community_posts.clear();
                        this.state.community_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.community_detail = None;
                        this.state.community_posts.clear();
                        this.state.community_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_actor_profile(&mut self, handle_or_id: String, cx: &mut Context<Self>) {
        self.state.profile_gen = self.state.profile_gen.wrapping_add(1);
        let my_gen = self.state.profile_gen;
        let norm_target = handle_or_id.trim().trim_start_matches('@');
        if self.state.actor_profile.as_ref().is_some_and(|p| {
            !p.actor.handle.eq_ignore_ascii_case(norm_target) && p.actor.actor_id != norm_target
        }) {
            self.state.actor_profile = None;
        }
        self.state.profile_error = None;
        self.state.profile_loading = true;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.get_actor_profile(&handle_or_id).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.profile_gen != my_gen {
                    return;
                }
                this.state.profile_loading = false;
                match res {
                    Ok(Ok(prof)) => {
                        this.state.actor_profile = Some(prof);
                        this.state.profile_error = None;
                    }
                    Ok(Err(e)) => {
                        this.state.actor_profile = None;
                        this.state.profile_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.actor_profile = None;
                        this.state.profile_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl CommunityWorkspace {
    pub fn execute_search(
        &mut self,
        query: String,
        tab: CommunitySearchTab,
        cx: &mut Context<Self>,
    ) {
        self.state.search_gen = self.state.search_gen.wrapping_add(1);
        let my_gen = self.state.search_gen;
        self.state.search_tab = tab;
        self.state.search_loading = true;
        let req = CommunitySearchRequestDto {
            query,
            tab,
            limit: 25,
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res =
                run_on_community_tokio(&handle, async move { facade.search(&req).await }).await;
            let _ = weak.update(app, |this, cx| {
                if this.state.search_gen != my_gen {
                    return;
                }
                this.state.search_loading = false;
                if let Ok(Ok(sr)) = res {
                    this.state.search_result = Some(sr);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_notifications(&mut self, cx: &mut Context<Self>) {
        self.state.notifications_gen = self.state.notifications_gen.wrapping_add(1);
        let my_notif_gen = self.state.notifications_gen;
        self.state.notifications_loading = true;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.get_notifications(None, Some(40), None).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.notifications_gen != my_notif_gen {
                    return;
                }
                this.state.notifications_loading = false;
                if let Ok(Ok(resp)) = res {
                    let unread = resp.unread_count.min(u32::MAX as u64) as u32;
                    this.state.notifications = resp.notifications;
                    this.state.unread_notifications = unread;
                    this.emit_action(CommunityWorkspaceAction::UnreadCountChanged(unread), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn mark_all_notifications_read(&mut self, cx: &mut Context<Self>) {
        self.state.notifications_gen = self.state.notifications_gen.wrapping_add(1);
        let my_notif_gen = self.state.notifications_gen;
        self.state.notifications_loading = false;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .mark_notifications_read(&MarkNotificationsReadRequest { notif_ids: None })
                    .await
            })
            .await;
            if let Ok(Ok(resp)) = res {
                let _ = weak.update(app, |this, cx| {
                    if this.state.notifications_gen != my_notif_gen {
                        return;
                    }
                    this.state.notifications_loading = false;
                    let unread = resp.unread_count.min(u32::MAX as u64) as u32;
                    this.state.unread_notifications = unread;
                    for n in &mut this.state.notifications {
                        n.is_read = true;
                    }
                    this.emit_action(CommunityWorkspaceAction::UnreadCountChanged(unread), cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn load_saved_posts(&mut self, cx: &mut Context<Self>) {
        self.state.saved_gen = self.state.saved_gen.wrapping_add(1);
        let my_gen = self.state.saved_gen;
        self.state.saved_loading = true;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let saved = facade
                    .get_feed(&FeedRequestDto {
                        tab: FeedTab::Saved,
                        limit: 30,
                        ..Default::default()
                    })
                    .await
                    .ok();
                let lists = facade.list_lists().await.ok();
                (saved, lists)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.saved_gen != my_gen {
                    return;
                }
                this.state.saved_loading = false;
                if let Ok((saved, lists)) = res {
                    this.state.saved_items = saved.map(|s| s.items).unwrap_or_default();
                    if let Some(l) = lists {
                        this.state.lists = l.lists;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_mod_queue(&mut self, slug: String, cx: &mut Context<Self>) {
        self.state.mod_gen = self.state.mod_gen.wrapping_add(1);
        let my_gen = self.state.mod_gen;
        self.state.mod_slug = Some(slug.clone());
        self.state.mod_loading = true;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let queue = facade.get_mod_queue(&slug, None, Some(30)).await.ok();
                let logs = facade.get_mod_logs(&slug, Some(30)).await.ok();
                (queue, logs)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.mod_gen != my_gen {
                    return;
                }
                this.state.mod_loading = false;
                if let Ok((q, l)) = res {
                    this.state.mod_queue = q.map(|x| x.items).unwrap_or_default();
                    this.state.mod_logs = l.map(|x| x.logs).unwrap_or_default();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn execute_mod_decision(
        &mut self,
        slug: String,
        item_id: String,
        target_kind: String,
        target_id: String,
        action: ModActionKind,
        cx: &mut Context<Self>,
    ) {
        let req = ModActionRequest {
            action,
            target_kind,
            target_id,
            reason: Some("Reviewed in Moderation Queue".to_string()),
            flair: None,
            queue_item_id: Some(item_id),
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_slug = slug.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.execute_mod_action(&slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(_)) => {
                    this.load_mod_queue(reload_slug, cx);
                }
                Ok(Err(e)) => {
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: e.to_string(),
                            is_error: true,
                        },
                        cx,
                    );
                }
                Err(_) => {}
            });
        })
        .detach();
    }

    pub fn submit_register_bot(&mut self, cx: &mut Context<Self>) {
        if self.state.bot_studio_submitting {
            return;
        }
        let name = self.state.bot_studio_name.trim().to_string();
        let handle_raw = self
            .state
            .bot_studio_handle
            .trim()
            .trim_start_matches('@')
            .to_string();
        if name.is_empty() || handle_raw.is_empty() {
            let lang = self.language_id.as_str();
            self.state.bot_studio_error = Some(
                tr_str(lang, "请填写 Bot 名称和 @handle", "Bot display name and @handle are required")
                    .to_string(),
            );
            cx.notify();
            return;
        }
        let shape = GROK_SHAPE_CHOICES[self.state.bot_studio_shape_idx % GROK_SHAPE_CHOICES.len()];
        let color = GROK_COLOR_PALETTE[self.state.bot_studio_color_idx % GROK_COLOR_PALETTE.len()].id;
        let persona = {
            let p = self.state.bot_studio_persona.trim().to_string();
            if p.is_empty() { None } else { Some(p) }
        };
        let req = RegisterBotActorRequest {
            bot_user_id: None,
            actor_kind: Some(gpui_telegram_proto::community_contracts::ActorKind::Bot),
            handle: handle_raw,
            display_name: name,
            bio: persona.clone(),
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: Some(shape.to_string()),
            avatar_color: Some(color.to_string()),
            bot_badge: Some("AI Agent".to_string()),
            bot_model: Some("Fabushi-1".to_string()),
            bot_capabilities: vec!["community".to_string(), "debate".to_string()],
            bot_autonomy_mode: self.state.bot_studio_autonomy,
            bot_persona: persona,
            cooldown_secs: Some(30),
            max_posts_per_hour: Some(12),
        };
        self.state.bot_studio_submitting = true;
        self.state.bot_studio_error = None;
        self.state.identities_gen = self.state.identities_gen.wrapping_add(1);
        let my_id_gen = self.state.identities_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let created = facade.register_bot_actor(&req).await;
                let ids = facade.get_identities().await.ok();
                (created, ids)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                this.state.bot_studio_submitting = false;
                match res {
                    Ok((Ok(reg), ids)) => {
                        this.state.bot_studio_name.clear();
                        this.state.bot_studio_handle.clear();
                        this.state.bot_studio_persona.clear();
                        this.state.bot_studio_last_token = reg.bot_token;
                        if this.state.identities_gen == my_id_gen {
                            if let Some(i) = ids {
                                this.state.identities = Some(i);
                            }
                        }
                        let lang = this.language_id.clone();
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: tr_str(
                                    &lang,
                                    "Bot 社区公民注册成功！现可在左侧身份切换器中选择它发帖。",
                                    "Bot citizen registered! Select it in the Identity Switcher to post.",
                                )
                                .to_string(),
                                is_error: false,
                            },
                            cx,
                        );
                    }
                    Ok((Err(e), _)) => {
                        this.state.bot_studio_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.bot_studio_error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl CommunityWorkspace {
    fn render_secondary_header(
        &self,
        title: String,
        subtitle: Option<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        div()
            .w_full()
            .flex_shrink_0()
            .px(px(14.0))
            .py(px(10.0))
            .bg(rgb(pal.dialogs_bg))
            .border_b_1()
            .border_color(pal.separator_rgba())
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .id("community-secondary-back-btn")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded_full()
                    .bg(pal.service_pill_rgba())
                    .hover(move |s| s.bg(pal.row_hover_rgba()))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(12.0))
                    .text_color(rgb(pal.text_primary))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.go_back(cx);
                        }),
                    )
                    .child(tg_icon(IconName::BackArrow, 13.0, pal.text_primary))
                    .child(tr_str(lang, "返回", "Back")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(title),
                    )
                    .when_some(subtitle, |col, sub| {
                        col.child(
                            div()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(sub),
                        )
                    }),
            )
    }

    fn render_comment_node(
        &self,
        comment: &CommunityCommentDto,
        cx: &mut Context<Self>,
    ) -> Div {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let indent = ((comment.depth.min(6)) as f32) * 16.0;
        let cid = comment.comment_id.clone();
        let reply_cid = cid.clone();
        let author_handle = comment.author_handle.clone();

        div()
            .w_full()
            .pl(px(indent))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .w_full()
                    .p(px(10.0))
                    .rounded(px(9.0))
                    .bg(pal.card_surface_rgba())
                    .border_l_2()
                    .border_color(if comment.author_kind.is_bot_like() {
                        rgb(0x8B5CF6).into()
                    } else {
                        pal.separator_rgba()
                    })
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("cmt-author-{cid}")))
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_actor_profile(author_handle.clone(), cx);
                                        }),
                                    )
                                    .child(comment.author_display_name.clone()),
                            )
                            .child(render_actor_badge_row(
                                comment.author_kind,
                                comment.author_verified_tier,
                                comment.author_bot_badge.as_deref(),
                                comment.is_mod_distinguished,
                                comment.is_op,
                                &comment.author_avatar_color,
                                &pal,
                                lang,
                            ))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!(
                                        "@{} · {} · ▲ {}",
                                        comment.author_handle,
                                        format_relative_time(comment.created_at_ms, lang),
                                        comment.score
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(pal.text_primary))
                            .child(comment.body.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("cmt-reply-btn-{cid}")))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.detail_reply_parent_id = Some(reply_cid.clone());
                                    this.state.focus_input(ActiveCommunityInput::DetailReply);
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "回复", "Reply")),
                    ),
            )
            .children(comment.children.iter().map(|ch| self.render_comment_node(ch, cx)))
    }

    fn render_post_detail_route(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let det = self.state.detail_post.clone();
        let reply_focused = self.state.active_input == ActiveCommunityInput::DetailReply;

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                tr_str(lang, "帖子详情与讨论树", "Post Thread & Comments").to_string(),
                det.as_ref().map(|d| format!("{} comments", d.total_comments)),
                cx,
            ))
            .child(
                div()
                    .id("community-detail-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.detail_scroll_handle)
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .when(self.state.detail_loading && det.is_none(), |col| {
                                col.child(render_loading_skeleton(2, &pal))
                            })
                            .when(!self.state.detail_loading && det.is_none(), |col| {
                                let sub = self.state.detail_error.as_deref().unwrap_or_else(|| {
                                    tr_str(
                                        lang,
                                        "该帖子可能已被删除或链接无效，请返回信息流浏览。",
                                        "This post may have been removed or does not exist.",
                                    )
                                });
                                col.child(render_empty_state(
                                    IconName::CommunityThread,
                                    tr_str(lang, "未找到该帖子", "Post not found"),
                                    sub,
                                    &pal,
                                ))
                            })
                            .when_some(det, |col, d| {
                                let pid = d.post.post_id.clone();
                                let summon_pid = pid.clone();
                                let item = FeedItemDto {
                                    post: d.post,
                                    explanation: None,
                                    rank_score: 0.0,
                                    is_exploration_slot: false,
                                };
                                col.child(render_post_card(self, &item, 0, cx))
                                    // Inline Reply + Summon Bot bar
                                    .child(
                                        div()
                                            .p(px(10.0))
                                            .rounded(px(10.0))
                                            .bg(pal.card_surface_rgba())
                                            .border_1()
                                            .border_color(pal.hairline_border_rgba())
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .id("detail-reply-input")
                                                    .cursor_text()
                                                    .w_full()
                                                    .min_h(px(44.0))
                                                    .p(px(8.0))
                                                    .rounded(px(8.0))
                                                    .bg(rgb(pal.input_bg))
                                                    .border_1()
                                                    .border_color(if reply_focused {
                                                        rgb(pal.accent).into()
                                                    } else {
                                                        pal.hairline_border_rgba()
                                                    })
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.state.focus_input(ActiveCommunityInput::DetailReply);
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(render_input_display(
                                                        &self.state.detail_reply_draft,
                                                        if reply_focused {
                                                            self.state.ime_preedit.as_deref()
                                                        } else {
                                                            None
                                                        },
                                                        tr_str(lang, "写下你的回复（以当前人类或 Bot 身份发言）…", "Write a reply as your active Human or Bot identity…"),
                                                        reply_focused,
                                                        &pal,
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .id("detail-summon-bot-btn")
                                                            .cursor_pointer()
                                                            .px(px(10.0))
                                                            .py(px(5.0))
                                                            .rounded_full()
                                                            .bg(TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.16))
                                                            .text_size(px(11.5))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(0x8B5CF6))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener(move |this, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.summon_bot_on_post(summon_pid.clone(), cx);
                                                                }),
                                                            )
                                                            .child(tr_str(lang, "🤖 召唤 Bot 参与讨论", "🤖 Summon Bot")),
                                                    )
                                                    .child(
                                                        div()
                                                            .id("detail-reply-submit-btn")
                                                            .cursor_pointer()
                                                            .px(px(14.0))
                                                            .py(px(5.0))
                                                            .rounded_full()
                                                            .bg(rgb(pal.accent))
                                                            .text_size(px(12.0))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(pal.active_button_fg))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener(|this, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.submit_detail_reply(cx);
                                                                }),
                                                            )
                                                            .child(tr_str(lang, "发送回复", "Reply")),
                                                    ),
                                            ),
                                    )
                                    .children(d.comments.iter().map(|c| self.render_comment_node(c, cx)))
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl CommunityWorkspace {
    fn render_community_page_route(&self, slug: &str, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let detail = self.state.community_detail.clone();
        let posts = self.state.community_posts.clone();

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                format!("c/{slug}"),
                detail.as_ref().map(|d| d.community.name.clone()),
                cx,
            ))
            .child(
                div()
                    .id("community-page-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .when(self.state.community_loading && detail.is_none(), |col| {
                                col.child(render_loading_skeleton(2, &pal))
                            })
                            .when(!self.state.community_loading && detail.is_none(), |col| {
                                let sub =
                                    self.state.community_error.as_deref().unwrap_or_else(|| {
                                        tr_str(
                                            lang,
                                            "未找到对应的 c/ 社区，请检查社区名称或前往探索页。",
                                            "Could not find this c/ community. Check the slug or browse Explore.",
                                        )
                                    });
                                col.child(render_empty_state(
                                    IconName::CommunityGlobe,
                                    tr_str(lang, "未找到该社区", "Community not found"),
                                    sub,
                                    &pal,
                                ))
                            })
                            .when_some(detail, |col, d| {
                                let c = d.community;
                                let join_slug = c.slug.clone();
                                let joined = c.is_joined;
                                col.child(
                                    div()
                                        .p(px(14.0))
                                        .rounded(px(12.0))
                                        .bg(pal.card_surface_rgba())
                                        .border_1()
                                        .border_color(pal.hairline_border_rgba())
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(8.0))
                                                        .child(div().text_size(px(22.0)).child(if c.icon_emoji.trim().is_empty() { "🌐".to_string() } else { c.icon_emoji }))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_col()
                                                                .child(div().text_size(px(15.0)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(c.name))
                                                                .child(div().text_size(px(11.5)).text_color(rgb(pal.text_secondary)).child(format!("c/{} · {} Humans · {} Bots", c.slug, c.human_member_count, c.bot_member_count))),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .id("comm-page-join-btn")
                                                        .cursor_pointer()
                                                        .px(px(14.0))
                                                        .py(px(6.0))
                                                        .rounded_full()
                                                        .bg(if joined { pal.service_pill_rgba() } else { rgb(pal.accent).into() })
                                                        .text_size(px(12.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(rgb(if joined { pal.text_primary } else { pal.active_button_fg }))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.toggle_join_community(join_slug.clone(), !joined, cx);
                                                            }),
                                                        )
                                                        .child(if joined { tr_str(lang, "已加入", "Joined") } else { tr_str(lang, "加入社区", "Join") }),
                                                ),
                                        )
                                        .child(div().text_size(px(12.5)).text_color(rgb(pal.text_secondary)).child(c.description))
                                        .when(!c.post_flairs.is_empty(), |box_el| {
                                            box_el.child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .gap(px(5.0))
                                                    .children(c.post_flairs.iter().map(|f| render_flair_chip(&f.text, Some(&f.bg_color), &pal))),
                                            )
                                        })
                                        .when(!c.rules.is_empty(), |box_el| {
                                            box_el.child(
                                                div()
                                                    .pt(px(6.0))
                                                    .border_t_1()
                                                    .border_color(pal.hairline_border_rgba())
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(3.0))
                                                    .child(div().text_size(px(11.0)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "社区规则", "Community Rules")))
                                                    .children(c.rules.iter().enumerate().map(|(i, r)| {
                                                        div().text_size(px(11.5)).text_color(rgb(pal.text_primary)).child(format!("{}. {}", i + 1, r.title))
                                                    })),
                                            )
                                        }),
                                )
                            })
                            .children(posts.iter().enumerate().map(|(i, it)| render_post_card(self, it, i, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_actor_profile_route(&self, handle: &str, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let prof = self.state.actor_profile.clone();

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                format!("@{handle}"),
                prof.as_ref().map(|p| p.actor.display_name.clone()),
                cx,
            ))
            .child(
                div()
                    .id("community-profile-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .when(self.state.profile_loading && prof.is_none(), |col| {
                                col.child(render_loading_skeleton(2, &pal))
                            })
                            .when(!self.state.profile_loading && prof.is_none(), |col| {
                                let sub =
                                    self.state.profile_error.as_deref().unwrap_or_else(|| {
                                        tr_str(
                                            lang,
                                            "未找到该人类或 Bot 公民主页。",
                                            "This Human or Bot citizen profile could not be found.",
                                        )
                                    });
                                col.child(render_empty_state(
                                    IconName::BotRobot,
                                    tr_str(lang, "未找到该用户或 Bot", "Actor not found"),
                                    sub,
                                    &pal,
                                ))
                            })
                            .when_some(prof, |col, p| {
                                let a = p.actor;
                                let aid = a.actor_id.clone();
                                let following = a.is_following;
                                let linked_uid = a.linked_user_id.unwrap_or(0);
                                col.child(
                                    div()
                                        .p(px(14.0))
                                        .rounded(px(12.0))
                                        .bg(pal.card_surface_rgba())
                                        .border_1()
                                        .border_color(pal.hairline_border_rgba())
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap(px(10.0))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(10.0))
                                                        .child(render_actor_avatar(
                                                            &a.actor_id,
                                                            &a.display_name,
                                                            a.actor_kind,
                                                            a.avatar_media_id.as_deref(),
                                                            &a.avatar_shape,
                                                            &a.avatar_color,
                                                            48.0,
                                                            pal.card_bg,
                                                            &self.media_base_url,
                                                            true,
                                                            &pal,
                                                        ))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_col()
                                                                .gap(px(2.0))
                                                                .child(
                                                                    div()
                                                                        .flex()
                                                                        .items_center()
                                                                        .gap(px(6.0))
                                                                        .child(div().text_size(px(15.0)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(a.display_name))
                                                                        .child(render_actor_badge_row(a.actor_kind, a.verified_tier, a.bot_badge.as_deref(), false, false, &a.avatar_color, &pal, lang)),
                                                                )
                                                                .child(div().text_size(px(12.0)).text_color(rgb(pal.text_secondary)).child(format!("@{} · {} Post Karma · {} Comment Karma", a.handle, format_compact_count(a.post_karma), format_compact_count(a.comment_karma)))),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(6.0))
                                                        .child(
                                                            div()
                                                                .id("profile-dm-btn")
                                                                .cursor_pointer()
                                                                .px(px(10.0))
                                                                .py(px(5.0))
                                                                .rounded_full()
                                                                .bg(pal.service_pill_rgba())
                                                                .text_size(px(11.5))
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .text_color(rgb(pal.text_primary))
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(move |this, _, _, cx| {
                                                                        cx.stop_propagation();
                                                                        this.emit_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: linked_uid }, cx);
                                                                    }),
                                                                )
                                                                .child(tr_str(lang, "私聊", "Message")),
                                                        )
                                                        .child(
                                                            div()
                                                                .id("profile-follow-btn")
                                                                .cursor_pointer()
                                                                .px(px(12.0))
                                                                .py(px(5.0))
                                                                .rounded_full()
                                                                .bg(if following { pal.service_pill_rgba() } else { rgb(pal.accent).into() })
                                                                .text_size(px(11.5))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(if following { pal.text_primary } else { pal.active_button_fg }))
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(move |this, _, _, cx| {
                                                                        cx.stop_propagation();
                                                                        this.toggle_follow_actor(aid.clone(), !following, cx);
                                                                    }),
                                                                )
                                                                .child(if following { tr_str(lang, "已关注", "Following") } else { tr_str(lang, "关注", "Follow") }),
                                                        ),
                                                ),
                                        )
                                        .when(!a.bio.trim().is_empty(), |b| {
                                            b.child(div().text_size(px(12.5)).text_color(rgb(pal.text_primary)).child(a.bio))
                                        }),
                                )
                                .children(p.recent_posts.into_iter().enumerate().map(|(i, post)| {
                                    let it = FeedItemDto { post, explanation: None, rank_score: 0.0, is_exploration_slot: false };
                                    render_post_card(self, &it, i, cx)
                                }))
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl CommunityWorkspace {
    fn render_explore_route(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let focused = self.state.active_input == ActiveCommunityInput::ExploreSearch;
        let sr = self.state.search_result.clone();
        let active_tab = self.state.search_tab;
        let tabs: [(CommunitySearchTab, &'static str, &'static str); 7] = [
            (CommunitySearchTab::Top, "综合", "Top"),
            (CommunitySearchTab::Latest, "最新", "Latest"),
            (CommunitySearchTab::People, "人类", "People"),
            (CommunitySearchTab::Bots, "Bots", "Bots"),
            (CommunitySearchTab::Communities, "社区", "Communities"),
            (CommunitySearchTab::Media, "媒体", "Media"),
            (CommunitySearchTab::Polls, "投票", "Polls"),
        ];

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                tr_str(lang, "探索与全站搜索", "Explore & Search").to_string(),
                None,
                cx,
            ))
            .child(
                div()
                    .id("community-explore-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .id("explore-search-box")
                                    .cursor_text()
                                    .w_full()
                                    .px(px(12.0))
                                    .py(px(9.0))
                                    .rounded_full()
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(if focused { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.state.focus_input(ActiveCommunityInput::ExploreSearch);
                                            cx.notify();
                                        }),
                                    )
                                    .child(tg_icon(IconName::Search, 15.0, pal.text_secondary))
                                    .child(div().flex_1().child(render_input_display(
                                        &self.state.explore_search_draft,
                                        if focused { self.state.ime_preedit.as_deref() } else { None },
                                        tr_str(lang, "输入关键词、#话题、from:handle、c/slug 并按回车…", "Search keywords, #tag, from:handle, c/slug + Enter…"),
                                        focused,
                                        &pal,
                                    ))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(5.0))
                                    .children(tabs.into_iter().map(|(tab, zh, en)| {
                                        let active = active_tab == tab;
                                        div()
                                            .id(SharedString::from(format!("explore-tab-{en}")))
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(4.0))
                                            .rounded_full()
                                            .bg(if active { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if active { pal.active_button_fg } else { pal.text_secondary }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    let q = this.state.explore_search_draft.trim().to_string();
                                                    this.execute_search(q, tab, cx);
                                                }),
                                            )
                                            .child(tr_str(lang, zh, en))
                                    })),
                            )
                            .when_some(sr, |col, res| {
                                col.children(res.posts.into_iter().enumerate().map(|(i, p)| {
                                    let it = FeedItemDto { post: p, explanation: None, rank_score: 0.0, is_exploration_slot: false };
                                    render_post_card(self, &it, i, cx)
                                }))
                                .children(res.actors.into_iter().map(|a| {
                                    let handle = a.handle.clone();
                                    div()
                                        .id(SharedString::from(format!("explore-actor-{}", a.actor_id)))
                                        .cursor_pointer()
                                        .p(px(10.0))
                                        .rounded(px(10.0))
                                        .bg(pal.card_surface_rgba())
                                        .border_1()
                                        .border_color(pal.hairline_border_rgba())
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.open_actor_profile(handle.clone(), cx);
                                            }),
                                        )
                                        .child(render_actor_avatar(&a.actor_id, &a.display_name, a.actor_kind, a.avatar_media_id.as_deref(), &a.avatar_shape, &a.avatar_color, 34.0, pal.card_bg, &self.media_base_url, true, &pal))
                                        .child(div().flex().flex_col().child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_primary)).child(a.display_name)).child(div().text_size(px(11.5)).text_color(rgb(pal.text_secondary)).child(format!("@{}", a.handle))))
                                }))
                                .children(res.communities.into_iter().map(|c| {
                                    let slug = c.slug.clone();
                                    div()
                                        .id(SharedString::from(format!("explore-comm-{}", c.slug)))
                                        .cursor_pointer()
                                        .p(px(10.0))
                                        .rounded(px(10.0))
                                        .bg(pal.card_surface_rgba())
                                        .border_1()
                                        .border_color(pal.hairline_border_rgba())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.navigate_to(CommunityRoute::CommunityPage(slug.clone()), cx);
                                            }),
                                        )
                                        .child(div().text_size(px(13.0)).font_weight(FontWeight::BOLD).text_color(rgb(pal.accent)).child(format!("c/{} · {}", c.slug, c.name)))
                                        .child(div().text_size(px(11.5)).text_color(rgb(pal.text_secondary)).child(c.description))
                                }))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_notifications_route(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let notifs = self.state.notifications.clone();

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                tr_str(lang, "社区通知", "Notifications").to_string(),
                Some(format!("{} unread", self.state.unread_notifications)),
                cx,
            ))
            .child(
                div()
                    .id("community-notifs-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .child(
                                        div()
                                            .id("notifs-mark-all-read-btn")
                                            .cursor_pointer()
                                            .px(px(12.0))
                                            .py(px(5.0))
                                            .rounded_full()
                                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.mark_all_notifications_read(cx);
                                                }),
                                            )
                                            .child(tr_str(lang, "全部标为已读", "Mark all read")),
                                    ),
                            )
                            .when(notifs.is_empty(), |col| {
                                col.child(render_empty_state(
                                    IconName::ChannelMegaphone,
                                    tr_str(lang, "暂无社区通知", "No notifications yet"),
                                    tr_str(lang, "当有人类或 Bot 回复、@提及或打赏你时将实时出现在这里。", "Replies, mentions, bot responses, and awards will appear here."),
                                    &pal,
                                ))
                            })
                            .children(notifs.into_iter().map(|n| {
                                let target_post = n.target_post_id.clone();
                                div()
                                    .id(SharedString::from(format!("notif-row-{}", n.notif_id)))
                                    .cursor_pointer()
                                    .p(px(11.0))
                                    .rounded(px(10.0))
                                    .bg(if n.is_read { pal.card_surface_rgba() } else { TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.10) })
                                    .border_1()
                                    .border_color(pal.hairline_border_rgba())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if let Some(pid) = target_post.clone() {
                                                this.open_post_detail(pid, cx);
                                            }
                                        }),
                                    )
                                    .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_primary)).child(format!("{} (@{}) · {}", n.actor_display_name, n.actor_handle, n.kind.as_str())))
                                    .child(div().text_size(px(12.0)).text_color(rgb(pal.text_secondary)).child(n.preview_text))
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_bookmarks_lists_route(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let saved = self.state.saved_items.clone();
        let lists = self.state.lists.clone();

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                tr_str(lang, "收藏夹与自定义列表", "Bookmarks & Custom Lists").to_string(),
                None,
                cx,
            ))
            .child(
                div()
                    .id("community-bookmarks-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .when(!lists.is_empty(), |col| {
                                col.child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(8.0))
                                        .children(lists.into_iter().map(|l| {
                                            let lid = l.list_id.clone();
                                            div()
                                                .id(SharedString::from(format!("bmk-list-{}", l.list_id)))
                                                .cursor_pointer()
                                                .px(px(12.0))
                                                .py(px(8.0))
                                                .rounded(px(10.0))
                                                .bg(pal.card_surface_rgba())
                                                .border_1()
                                                .border_color(pal.hairline_border_rgba())
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.select_feed_tab(FeedTab::List(lid.clone()), cx);
                                                    }),
                                                )
                                                .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_primary)).child(l.name))
                                                .child(div().text_size(px(11.0)).text_color(rgb(pal.text_secondary)).child(l.description))
                                        })),
                                )
                            })
                            .when(saved.is_empty(), |col| {
                                col.child(render_empty_state(
                                    IconName::Bookmark,
                                    tr_str(lang, "还没有收藏帖子", "No saved posts yet"),
                                    tr_str(lang, "点击任意帖子底部的书签图标即可加入收藏夹。", "Click the bookmark icon on any post to save it here."),
                                    &pal,
                                ))
                            })
                            .children(saved.iter().enumerate().map(|(i, it)| render_post_card(self, it, i, cx))),
                    ),
            )
            .into_any_element()
    }
}

impl CommunityWorkspace {
    fn render_mod_queue_route(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let active_slug = self
            .state
            .mod_slug
            .clone()
            .unwrap_or_else(|| "general".to_string());
        let comms = self.state.communities.clone();
        let queue = self.state.mod_queue.clone();
        let logs = self.state.mod_logs.clone();

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                tr_str(lang, "社区审核队列与审计日志", "Moderation Queue & Audit Logs").to_string(),
                Some(format!("c/{active_slug}")),
                cx,
            ))
            .child(
                div()
                    .id("community-mod-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(5.0))
                                    .children(comms.into_iter().take(8).map(|c| {
                                        let slug = c.slug.clone();
                                        let is_act = slug == active_slug;
                                        div()
                                            .id(SharedString::from(format!("mod-comm-{slug}")))
                                            .cursor_pointer()
                                            .px(px(9.0))
                                            .py(px(4.0))
                                            .rounded_full()
                                            .bg(if is_act { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                                            .text_size(px(11.5))
                                            .text_color(rgb(if is_act { pal.active_button_fg } else { pal.text_primary }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.load_mod_queue(slug.clone(), cx);
                                                }),
                                            )
                                            .child(format!("c/{}", c.slug))
                                    })),
                            )
                            .when(queue.is_empty(), |col| {
                                col.child(render_empty_state(
                                    IconName::ShieldMod,
                                    tr_str(lang, "当前队列已清空", "Moderation queue is clear"),
                                    tr_str(lang, "所有举报与 AutoMod 触发项均已处理完毕。", "All reports and AutoMod flags are resolved."),
                                    &pal,
                                ))
                            })
                            .children(queue.into_iter().map(|it| {
                                let slug_a = active_slug.clone();
                                let slug_r = active_slug.clone();
                                let iid_a = it.item_id.clone();
                                let iid_r = it.item_id.clone();
                                let tk_a = it.target_kind.clone();
                                let tk_r = it.target_kind.clone();
                                let tid_a = it.target_id.clone();
                                let tid_r = it.target_id.clone();
                                div()
                                    .p(px(11.0))
                                    .rounded(px(10.0))
                                    .bg(pal.card_surface_rgba())
                                    .border_1()
                                    .border_color(pal.hairline_border_rgba())
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .child(div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_primary)).child(format!("[{}] {}", it.target_kind, it.reason)))
                                            .child(div().text_size(px(11.0)).text_color(rgb(pal.text_secondary)).child(format!("Source: {} · Target: {}", it.source, it.target_id))),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!("mod-approve-{}", it.item_id)))
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(5.0))
                                                    .rounded(px(7.0))
                                                    .bg(TdesktopThemePalette::rgba_with_alpha(0x10B981, 0.16))
                                                    .text_size(px(11.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(0x10B981))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.execute_mod_decision(slug_a.clone(), iid_a.clone(), tk_a.clone(), tid_a.clone(), ModActionKind::Approve, cx);
                                                        }),
                                                    )
                                                    .child(tr_str(lang, "通过", "Approve")),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!("mod-remove-{}", it.item_id)))
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(5.0))
                                                    .rounded(px(7.0))
                                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                                                    .text_size(px(11.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(pal.danger))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.execute_mod_decision(slug_r.clone(), iid_r.clone(), tk_r.clone(), tid_r.clone(), ModActionKind::Remove, cx);
                                                        }),
                                                    )
                                                    .child(tr_str(lang, "移除", "Remove")),
                                            ),
                                    )
                            }))
                            .when(!logs.is_empty(), |col| {
                                col.child(div().pt(px(8.0)).text_size(px(12.5)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(tr_str(lang, "透明审核日志", "Transparent Mod Audit Log")))
                                    .children(logs.into_iter().take(12).map(|log| {
                                        div()
                                            .p(px(9.0))
                                            .rounded(px(8.0))
                                            .bg(pal.card_surface_rgba())
                                            .text_size(px(11.5))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(format!("@{} -> {} ({}) · {}", log.moderator_handle, log.action.as_str(), log.target_id, log.reason.unwrap_or_default()))
                                    }))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_bot_studio_route(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let bots = self
            .state
            .identities
            .as_ref()
            .map(|i| i.bot_actors.clone())
            .unwrap_or_default();
        let name_foc = self.state.active_input == ActiveCommunityInput::BotStudioName;
        let handle_foc = self.state.active_input == ActiveCommunityInput::BotStudioHandle;
        let persona_foc = self.state.active_input == ActiveCommunityInput::BotStudioPersona;
        let shape = GROK_SHAPE_CHOICES[self.state.bot_studio_shape_idx % GROK_SHAPE_CHOICES.len()];
        let color = GROK_COLOR_PALETTE[self.state.bot_studio_color_idx % GROK_COLOR_PALETTE.len()].id;
        let autonomy = self.state.bot_studio_autonomy;
        let autonomies = [
            (BotAutonomyMode::Manual, "手动", "Manual"),
            (BotAutonomyMode::MentionOnly, "@提及回复", "Mention Only"),
            (BotAutonomyMode::Reactive, "社区响应", "Reactive"),
            (BotAutonomyMode::Autonomous, "自主创作", "Autonomous"),
        ];

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(pal.stage_bg))
            .child(self.render_secondary_header(
                tr_str(lang, "Bot 公民工作室", "Bot Citizenship Studio").to_string(),
                Some(tr_str(lang, "Bot 与人类同等一等公民 · 注册、定制外观与发帖身份", "Bots are equal first-class citizens · Register, customize & post").to_string()),
                cx,
            ))
            .child(
                div()
                    .id("community-bot-studio-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p(px(14.0))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(740.0))
                            .mx_auto()
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            // Owned bots list
                            .child(
                                div()
                                    .p(px(12.0))
                                    .rounded(px(12.0))
                                    .bg(pal.card_surface_rgba())
                                    .border_1()
                                    .border_color(pal.hairline_border_rgba())
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .child(div().text_size(px(13.5)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(format!("{} ({})", tr_str(lang, "我的社区 Bot 身份", "My Community Bots"), bots.len())))
                                    .when(bots.is_empty(), |col| {
                                        col.child(div().text_size(px(12.0)).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "暂无绑定的 Bot，在下方注册一个即可立即在社区发帖和参与辩论。", "No bots registered yet. Create one below to start posting and debating.")))
                                    })
                                    .children(bots.into_iter().map(|b| {
                                        let aid = b.actor_id.clone();
                                        let is_active = aid == self.state.active_actor_id;
                                        div()
                                            .p(px(8.0))
                                            .rounded(px(9.0))
                                            .bg(pal.service_pill_rgba())
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .child(render_actor_avatar(&b.actor_id, &b.display_name, b.actor_kind, b.avatar_media_id.as_deref(), &b.avatar_shape, &b.avatar_color, 34.0, pal.card_bg, &self.media_base_url, true, &pal))
                                                    .child(div().flex().flex_col().child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_primary)).child(b.display_name)).child(div().text_size(px(11.0)).text_color(rgb(pal.text_secondary)).child(format!("@{} · {} · {} karma", b.handle, b.bot_autonomy_mode.as_str(), b.post_karma + b.comment_karma)))),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!("studio-act-as-{}", b.actor_id)))
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(5.0))
                                                    .rounded_full()
                                                    .bg(if is_active { TdesktopThemePalette::rgba_with_alpha(0x10B981, 0.16) } else { rgb(pal.accent).into() })
                                                    .text_size(px(11.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(if is_active { 0x10B981 } else { pal.active_button_fg }))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.switch_active_actor(aid.clone(), cx);
                                                        }),
                                                    )
                                                    .child(if is_active { tr_str(lang, "当前身份", "Active") } else { tr_str(lang, "切换为此身份", "Act As Bot") }),
                                            )
                                    })),
                            )
                            // Register new bot form
                            .child(
                                div()
                                    .p(px(14.0))
                                    .rounded(px(12.0))
                                    .bg(pal.card_surface_rgba())
                                    .border_1()
                                    .border_color(pal.hairline_border_rgba())
                                    .flex()
                                    .flex_col()
                                    .gap(px(9.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(div().text_size(px(13.5)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(tr_str(lang, "注册新 Bot 公民", "Register New Bot Citizen")))
                                            .child(render_actor_avatar("preview-bot", "Bot", gpui_telegram_proto::community_contracts::ActorKind::Bot, None, shape, color, 34.0, pal.card_bg, &self.media_base_url, true, &pal)),
                                    )
                                    .child(
                                        div()
                                            .id("studio-name-input")
                                            .cursor_text()
                                            .p(px(8.0))
                                            .rounded(px(8.0))
                                            .bg(rgb(pal.input_bg))
                                            .border_1()
                                            .border_color(if name_foc { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.state.focus_input(ActiveCommunityInput::BotStudioName); cx.notify(); }))
                                            .child(render_input_display(&self.state.bot_studio_name, if name_foc { self.state.ime_preedit.as_deref() } else { None }, tr_str(lang, "Bot 显示名称 (如: Rust架构师Bot)", "Bot Display Name"), name_foc, &pal)),
                                    )
                                    .child(
                                        div()
                                            .id("studio-handle-input")
                                            .cursor_text()
                                            .p(px(8.0))
                                            .rounded(px(8.0))
                                            .bg(rgb(pal.input_bg))
                                            .border_1()
                                            .border_color(if handle_foc { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.state.focus_input(ActiveCommunityInput::BotStudioHandle); cx.notify(); }))
                                            .child(render_input_display(&self.state.bot_studio_handle, if handle_foc { self.state.ime_preedit.as_deref() } else { None }, tr_str(lang, "Bot @handle (如: rust_arch_bot)", "Bot @handle"), handle_foc, &pal)),
                                    )
                                    .child(
                                        div()
                                            .id("studio-persona-input")
                                            .cursor_text()
                                            .min_h(px(46.0))
                                            .p(px(8.0))
                                            .rounded(px(8.0))
                                            .bg(rgb(pal.input_bg))
                                            .border_1()
                                            .border_color(if persona_foc { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.state.focus_input(ActiveCommunityInput::BotStudioPersona); cx.notify(); }))
                                            .child(render_input_display(&self.state.bot_studio_persona, if persona_foc { self.state.ime_preedit.as_deref() } else { None }, tr_str(lang, "Bot 人设与专长简介 (可选)", "Bot persona & bio (optional)"), persona_foc, &pal)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(5.0))
                                            .children(autonomies.into_iter().map(|(m, zh, en)| {
                                                let act = autonomy == m;
                                                div()
                                                    .id(SharedString::from(format!("studio-autonomy-{en}")))
                                                    .cursor_pointer()
                                                    .px(px(9.0))
                                                    .py(px(4.0))
                                                    .rounded_full()
                                                    .bg(if act { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(if act { pal.active_button_fg } else { pal.text_primary }))
                                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.state.bot_studio_autonomy = m; cx.notify(); }))
                                                    .child(tr_str(lang, zh, en))
                                            })),
                                    )
                                    .when_some(self.state.bot_studio_error.clone(), |col, err| {
                                        col.child(div().text_size(px(11.5)).text_color(rgb(pal.danger)).child(err))
                                    })
                                    .child(
                                        div()
                                            .id("studio-register-btn")
                                            .cursor_pointer()
                                            .py(px(8.0))
                                            .rounded(px(9.0))
                                            .bg(rgb(pal.accent))
                                            .hover(move |s| s.bg(rgb(pal.accent_hover)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.active_button_fg))
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { cx.stop_propagation(); this.submit_register_bot(cx); }))
                                            .child(tr_str(lang, "注册 Bot 公民", "Register Bot Citizen")),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl CommunityWorkspace {
    fn render_route_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.state.route {
            CommunityRoute::Feed => render_feed_view(self, cx),
            CommunityRoute::PostDetail(_) => self.render_post_detail_route(cx),
            CommunityRoute::CommunityPage(slug) => self.render_community_page_route(slug, cx),
            CommunityRoute::ActorProfile(handle) => self.render_actor_profile_route(handle, cx),
            CommunityRoute::Explore { .. } => self.render_explore_route(cx),
            CommunityRoute::Notifications => self.render_notifications_route(cx),
            CommunityRoute::BookmarksLists => self.render_bookmarks_lists_route(cx),
            CommunityRoute::ModQueue(_) => self.render_mod_queue_route(cx),
            CommunityRoute::BotStudio => self.render_bot_studio_route(cx),
        }
    }

    fn render_active_popover_overlay(
        &self,
        pop: &CommunityPopover,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = match pop {
            CommunityPopover::IdentitySwitcher => render_identity_switcher_popover(self, cx),
            CommunityPopover::AlgoTuner => render_algo_tuner_popover(self, cx),
            CommunityPopover::ExplainPost(pid) => render_explain_popover(self, pid, cx),
            CommunityPopover::AwardPost(pid) => render_award_popover(self, pid, cx),
            CommunityPopover::QuickPostCommunityPicker => {
                render_quick_post_community_picker_popover(self, cx)
            }
            CommunityPopover::QuotePost(pid) => render_quote_popover(self, pid, cx),
        };

        div()
            .id("community-popover-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .bg(TdesktopThemePalette::rgba_with_alpha(0x000000, 0.24))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.state.active_popover = None;
                    if this.state.active_input == ActiveCommunityInput::QuotePostNote {
                        this.state.blur_input();
                    }
                    cx.notify();
                }),
            )
            .child(content)
            .into_any_element()
    }

    pub fn set_detail_comment_sort(&mut self, sort: CommentSort, cx: &mut Context<Self>) {
        if self.state.detail_comment_sort != sort {
            self.state.detail_comment_sort = sort;
            if let CommunityRoute::PostDetail(pid) = self.state.route.clone() {
                self.load_post_detail(pid, cx);
            }
        }
    }
    /// Builds the full Community GPUI element tree for a given window viewport width.
    pub fn render_for_viewport_width(
        &mut self,
        viewport_width_px: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = CommunityLayoutMode::resolve(viewport_width_px, self.host_sidebar_width_px);
        self.last_layout_mode = layout;

        let pal = self.palette;
        let active_popover = self.state.active_popover.clone();
        let center = self.render_route_content(cx);

        div()
            .id("community-workspace-root")
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .bg(rgb(pal.stage_bg))
            .overflow_hidden()
            .flex()
            .flex_col()
            .when(layout == CommunityLayoutMode::OneColumn, |col| {
                col.child(render_compact_top_nav(self, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .flex()
                    .when(layout != CommunityLayoutMode::OneColumn, |row| {
                        row.child(render_left_nav(self, cx))
                    })
                    .child(center)
                    .when(layout == CommunityLayoutMode::ThreeColumn, |row| {
                        row.child(render_right_rail(self, cx))
                    }),
            )
            .when_some(active_popover, |root, pop| {
                root.child(self.render_active_popover_overlay(&pop, cx))
            })
            .into_any_element()
    }
}

impl Render for CommunityWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vp_w = f32::from(window.viewport_size().width);
        self.render_for_viewport_width(vp_w, cx)
    }
}
