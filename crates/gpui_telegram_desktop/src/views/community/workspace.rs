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
use gpui_telegram_community::validate_create_comment_request;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, CommentSort, CommunityIdentitiesResponse, CommunityPollVoteRequest, CommunitySearchRequestDto, CommunitySearchTab, CreateCommentRequest,
    CreatePostRequest, EngagementSignalKind, FeedItemDto, FeedRequestDto, FeedSort, FeedTab,
    GiveAwardRequest, MarkNotificationsReadRequest, ModActionKind, ModActionRequest, PostFormat,
    PostStatus, RecordSignalRequest, RepostRequest, TimeWindow, VoteRequest,
};

use crate::components::ui::text_input_controller::{
    delete_prev_word, pop_last_char, sanitize_paste_text,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::stage::entities_render::RichTextAction;

use super::actor_profile_view::{render_actor_profile_view, render_edit_profile_modal};
use super::bookmarks_lists_view::render_bookmarks_lists_view;
use super::bot_studio_view::render_bot_studio_view;
use super::community_page_view::{render_community_page_view, render_create_community_modal};
use super::composer_modal::{compute_composer_autocomplete, render_composer_modal};
use super::explore_search_view::{
    build_search_request, filter_search_result_by_operators, render_explore_search_view,
};
use super::feed_view::{
    render_algo_tuner_popover, render_feed_view, render_quick_post_community_picker_popover,
};
use super::left_nav::{render_compact_top_nav, render_left_nav};
use super::mod_queue_view::render_mod_queue_view;
use super::notifications_view::render_notifications_view;
use super::post_card::{
    render_award_popover, render_explain_popover, render_quote_popover,
};
use super::post_detail_view::{
    render_comment_award_popover, render_post_detail_view, render_summon_bot_picker_popover,
};
use super::right_rail::render_right_rail;
use super::shared_widgets::render_identity_switcher_popover;
use super::state::{
    sanitize_algo_prefs, ActiveCommunityInput, BookmarksListsSection, CommunityLayoutMode,
    CommunityModal, CommunityPopover, CommunityRoute, CommunityWorkspaceAction,
    CommunityWorkspaceState, FeedDensity,
};

pub(super) async fn run_on_community_tokio<F, T>(
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
        self.ensure_bot_studio_selected_hydrated(cx);
        if prev_active != self.state.active_actor_id && !self.state.active_actor_id.is_empty() {
            self.load_feed(cx);
        }
        cx.notify();
    }

    pub(super) fn ensure_bot_studio_selected_hydrated(&mut self, cx: &mut Context<Self>) {
        if self.state.route != CommunityRoute::BotStudio {
            return;
        }
        if let Some(bot) = self.resolve_selected_owned_bot() {
            if self.state.bot_studio_edit.actor_id.is_empty() {
                self.state.bot_studio_selected_bot_id = Some(bot.actor_id.clone());
                self.ensure_bot_studio_edit_hydrated(&bot);
                cx.notify();
            } else if self.state.bot_studio_selected_bot_id.as_deref()
                != Some(bot.actor_id.as_str())
                || self.state.bot_studio_edit.actor_id != bot.actor_id
            {
                self.select_bot_for_studio(bot, cx);
            }
        }
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
        let my_actor_gen = self.state.actor_switch_gen;
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
                    if this.state.actor_switch_gen != my_actor_gen
                        && !this.state.active_actor_id.is_empty()
                    {
                        this.state.identities = Some(ids);
                        this.state.runtime_mode = this.facade.runtime_mode();
                        this.ensure_bot_studio_selected_hydrated(cx);
                        cx.notify();
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
        let my_actor_gen = self.state.actor_switch_gen;
        self.state.communities_gen = self.state.communities_gen.wrapping_add(1);
        self.state.communities_loading = true;
        let my_comm_gen = self.state.communities_gen;
        self.state.lists_gen = self.state.lists_gen.wrapping_add(1);
        let my_lists_gen = self.state.lists_gen;
        self.state.algo_prefs_gen = self.state.algo_prefs_gen.wrapping_add(1);
        let my_algo_gen = self.state.algo_prefs_gen;
        self.state.notifications_gen = self.state.notifications_gen.wrapping_add(1);
        let my_notif_gen = self.state.notifications_gen;
        self.state.notifications_loading = false;
        self.state.recommended_actors_gen = self.state.recommended_actors_gen.wrapping_add(1);
        self.state.recommended_actors_loading = true;
        let my_rec_gen = self.state.recommended_actors_gen;
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

            match res {
                Ok((ids, prefs, comms, lists, trending, notifs, bots, people, mode)) => {
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
                                this.ensure_bot_studio_selected_hydrated(cx);
                            }
                        }
                        if this.state.algo_prefs_gen == my_algo_gen {
                            if let Some(p) = prefs {
                                let clean = sanitize_algo_prefs(&p);
                                this.state.algo_prefs = clean.clone();
                                this.state.algo_draft = clean;
                            }
                        }
                        if this.state.communities_gen == my_comm_gen
                            && this.state.actor_switch_gen == my_actor_gen
                        {
                            this.state.communities_loading = false;
                            if let Some(mut c) = comms {
                                for comm in &mut c.communities {
                                    this.state.overlay_in_flight_community_mutations(comm);
                                }
                                this.state.communities = c.communities;
                            }
                        }
                        if this.state.lists_gen == my_lists_gen
                            && this.state.actor_switch_gen == my_actor_gen
                        {
                            if let Some(l) = lists {
                                this.state.merge_refreshed_lists(l.lists);
                            }
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
                        if this.state.recommended_actors_gen == my_rec_gen
                            && this.state.actor_switch_gen == my_actor_gen
                        {
                            this.state.recommended_actors_loading = false;
                            if !rec.is_empty() {
                                for a in &mut rec {
                                    this.state.overlay_in_flight_actor_mutations(a);
                                }
                                this.state.recommended_actors = rec;
                            }
                        }
                        cx.notify();
                    });
                }
                Err(_) => {
                    let _ = weak.update(app, |this, cx| {
                        if this.state.communities_gen == my_comm_gen
                            && this.state.actor_switch_gen == my_actor_gen
                        {
                            this.state.communities_loading = false;
                        }
                        if this.state.recommended_actors_gen == my_rec_gen
                            && this.state.actor_switch_gen == my_actor_gen
                        {
                            this.state.recommended_actors_loading = false;
                        }
                        cx.notify();
                    });
                }
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
        } else if self.state.active_input.is_composer_input() {
            self.state.composer.validation_error = None;
            if self.state.active_input == ActiveCommunityInput::ComposerBody {
                self.state.composer.autocomplete_dismissed = false;
            }
        } else if self.state.active_input.is_create_comm_input() {
            self.state.create_community.error = None;
        } else if self.state.active_input.is_profile_edit_input() {
            self.state.edit_profile.error = None;
        } else if self.state.active_input.is_list_editor_input() {
            self.state.list_editor.error = None;
        } else if self.state.active_input.is_automod_input() {
            self.state.automod_draft.error = None;
        } else if self.state.active_input.is_bot_studio_create_input() {
            self.state.bot_studio_error = None;
        } else if self.state.active_input.is_bot_studio_edit_input() {
            self.state.bot_studio_edit.error = None;
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
                if target == ActiveCommunityInput::ComposerBody {
                    self.state.composer.autocomplete_dismissed = false;
                }
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
                if target == ActiveCommunityInput::ComposerBody {
                    self.state.composer.autocomplete_dismissed = false;
                }
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
                        self.state.explore_show_discovery = q.is_empty();
                        self.execute_search(q, self.state.search_tab, cx);
                    }
                    ActiveCommunityInput::DetailReply => {
                        self.submit_detail_reply(cx);
                    }
                    ActiveCommunityInput::DetailEditComment => {
                        self.submit_edit_comment(cx);
                    }
                    ActiveCommunityInput::DetailSummonPrompt => {
                        if let Some(CommunityPopover::SummonBotPicker(pid)) =
                            self.state.active_popover.clone()
                        {
                            let bid = self.state.detail_summon_bot_id.clone();
                            let prompt = Some(self.state.detail_summon_prompt.trim().to_string())
                                .filter(|s| !s.is_empty());
                            let parent = self.state.detail_reply_parent_id.clone();
                            self.state.detail_summon_prompt.clear();
                            self.summon_bot_with_options(pid, bid, prompt, parent, cx);
                        }
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
                    | ActiveCommunityInput::BotStudioPersona
                    | ActiveCommunityInput::BotStudioBadge
                    | ActiveCommunityInput::BotStudioModel
                    | ActiveCommunityInput::BotStudioCooldown
                    | ActiveCommunityInput::BotStudioMaxPostsPerHour => {
                        self.submit_register_bot(cx);
                    }
                    ActiveCommunityInput::BotStudioCustomCap => {
                        self.add_bot_studio_custom_capability(cx);
                    }
                    ActiveCommunityInput::BotStudioEditCustomCap => {
                        self.add_bot_studio_edit_custom_capability(cx);
                    }
                    ActiveCommunityInput::BotStudioEditName
                    | ActiveCommunityInput::BotStudioEditBadge
                    | ActiveCommunityInput::BotStudioEditModel => {
                        self.save_selected_bot_edits(cx);
                    }
                    ActiveCommunityInput::BookmarkNewFolder => {
                        self.create_bookmark_folder(cx);
                    }
                    ActiveCommunityInput::ListEditorName => {
                        self.submit_list_editor(cx);
                    }
                    ActiveCommunityInput::AutoModRuleName
                    | ActiveCommunityInput::AutoModRuleKeywords
                    | ActiveCommunityInput::AutoModRuleRegex
                    | ActiveCommunityInput::AutoModRuleMinKarma
                    | ActiveCommunityInput::AutoModRuleMaxLinks
                    | ActiveCommunityInput::AutoModRuleFlair => {
                        self.submit_add_automod_rule(cx);
                    }
                    ActiveCommunityInput::AutoModDryRunTitle => {
                        self.run_automod_dry_run(cx);
                    }
                    ActiveCommunityInput::ModReason | ActiveCommunityInput::ModFlair => {
                        self.state.blur_input();
                        cx.notify();
                    }
                    ActiveCommunityInput::ComposerBody
                    | ActiveCommunityInput::ComposerThreadPart(_)
                    | ActiveCommunityInput::CommunityWikiMarkdown
                    | ActiveCommunityInput::CommunityNewRuleDesc
                    | ActiveCommunityInput::CreateCommDescription
                    | ActiveCommunityInput::ProfileEditBio
                    | ActiveCommunityInput::ProfileEditBotPersona
                    | ActiveCommunityInput::ListEditorDesc
                    | ActiveCommunityInput::AutoModRuleReply
                    | ActiveCommunityInput::AutoModDryRunBody
                    | ActiveCommunityInput::BotStudioBio
                    | ActiveCommunityInput::BotStudioEditBio
                    | ActiveCommunityInput::BotStudioEditPersona
                    | ActiveCommunityInput::BotStudioTestPostBody => {
                        if let Some(buf) = self.state.active_input_text_mut() {
                            buf.push('\n');
                        }
                        cx.notify();
                    }
                    ActiveCommunityInput::ComposerBotPrompt => {
                        self.generate_bot_coauthor_suggestion(cx);
                    }
                    ActiveCommunityInput::ComposerTitle
                    | ActiveCommunityInput::ComposerLinkUrl
                    | ActiveCommunityInput::ComposerLinkTitle
                    | ActiveCommunityInput::ComposerLinkDesc
                    | ActiveCommunityInput::ComposerMediaUrl(_)
                    | ActiveCommunityInput::ComposerMediaAlt(_)
                    | ActiveCommunityInput::ComposerPollQuestion
                    | ActiveCommunityInput::ComposerPollOption(_)
                    | ActiveCommunityInput::ComposerScheduleMinutes => {
                        let status = if self.state.composer.schedule_enabled {
                            PostStatus::Scheduled
                        } else {
                            PostStatus::Published
                        };
                        self.submit_composer(status, cx);
                    }
                    ActiveCommunityInput::CommunityWikiSummary => {
                        if let Some(slug) = self
                            .state
                            .community_detail
                            .as_ref()
                            .map(|d| d.community.slug.clone())
                        {
                            self.save_community_wiki(slug, cx);
                        }
                    }
                    ActiveCommunityInput::CommunityNewRuleTitle => {
                        if let Some(slug) = self
                            .state
                            .community_detail
                            .as_ref()
                            .map(|d| d.community.slug.clone())
                        {
                            self.add_community_rule(slug, cx);
                        }
                    }
                    ActiveCommunityInput::CreateCommSlug
                    | ActiveCommunityInput::CreateCommName
                    | ActiveCommunityInput::CreateCommIconEmoji => {
                        self.submit_create_community(cx);
                    }
                    ActiveCommunityInput::ProfileEditDisplayName
                    | ActiveCommunityInput::ProfileEditLocation
                    | ActiveCommunityInput::ProfileEditWebsite
                    | ActiveCommunityInput::ProfileEditBotBadge
                    | ActiveCommunityInput::ProfileEditBotModel => {
                        self.submit_edit_profile(cx);
                    }
                    ActiveCommunityInput::None => return false,
                }
                true
            }
            "tab" => {
                let next = match target {
                    ActiveCommunityInput::QuickPostTitle => ActiveCommunityInput::QuickPostBody,
                    ActiveCommunityInput::QuickPostBody => ActiveCommunityInput::QuickPostTitle,
                    ActiveCommunityInput::ComposerTitle => ActiveCommunityInput::ComposerBody,
                    ActiveCommunityInput::ComposerBody => ActiveCommunityInput::ComposerTitle,
                    ActiveCommunityInput::CreateCommSlug => ActiveCommunityInput::CreateCommName,
                    ActiveCommunityInput::CreateCommName => {
                        ActiveCommunityInput::CreateCommDescription
                    }
                    ActiveCommunityInput::CreateCommDescription => {
                        ActiveCommunityInput::CreateCommSlug
                    }
                    ActiveCommunityInput::ProfileEditDisplayName => {
                        ActiveCommunityInput::ProfileEditBio
                    }
                    ActiveCommunityInput::ProfileEditBio => {
                        ActiveCommunityInput::ProfileEditLocation
                    }
                    ActiveCommunityInput::ProfileEditLocation => {
                        ActiveCommunityInput::ProfileEditWebsite
                    }
                    ActiveCommunityInput::ProfileEditWebsite => {
                        ActiveCommunityInput::ProfileEditDisplayName
                    }
                    ActiveCommunityInput::ListEditorName => ActiveCommunityInput::ListEditorDesc,
                    ActiveCommunityInput::ListEditorDesc => ActiveCommunityInput::ListEditorName,
                    ActiveCommunityInput::AutoModRuleName => {
                        ActiveCommunityInput::AutoModRuleKeywords
                    }
                    ActiveCommunityInput::AutoModRuleKeywords => {
                        ActiveCommunityInput::AutoModRuleRegex
                    }
                    ActiveCommunityInput::AutoModRuleRegex => ActiveCommunityInput::AutoModRuleName,
                    ActiveCommunityInput::BotStudioName => ActiveCommunityInput::BotStudioHandle,
                    ActiveCommunityInput::BotStudioHandle => ActiveCommunityInput::BotStudioPersona,
                    ActiveCommunityInput::BotStudioPersona => ActiveCommunityInput::BotStudioName,
                    ActiveCommunityInput::BotStudioEditName => {
                        ActiveCommunityInput::BotStudioEditBio
                    }
                    ActiveCommunityInput::BotStudioEditBio => {
                        ActiveCommunityInput::BotStudioEditPersona
                    }
                    ActiveCommunityInput::BotStudioEditPersona => {
                        ActiveCommunityInput::BotStudioEditName
                    }
                    other => other,
                };
                self.state.focus_input(next);
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// Progressive Escape cascade inside Community (strictly matching visual Z-order):
    /// 1) Close topmost open Community popover (Identity Switcher / Algo Tuner / Explain / Award / Quote / Summon Bot)
    /// 2) Dismiss open Composer `@/#`/`c/` autocomplete popup
    /// 3) Close open Community modal sheet (Composer / Create Community / Edit Profile)
    /// 4) Close open inline List Editor / Bookmark Folder Move picker
    /// 5) Blur focused Community text input
    /// 6) Return `false` so `AppShell::dismiss_post_auth_escape` exits Community back to `Chats`.
    pub fn handle_escape_key(&mut self, cx: &mut Context<Self>) -> bool {
        if self.state.active_popover.is_some() {
            self.state.active_popover = None;
            if matches!(
                self.state.active_input,
                ActiveCommunityInput::QuotePostNote | ActiveCommunityInput::DetailSummonPrompt
            ) {
                self.state.blur_input();
            }
            cx.notify();
            return true;
        }
        if self.state.active_modal == Some(CommunityModal::Composer)
            && compute_composer_autocomplete(self).is_some()
        {
            self.state.composer.autocomplete_dismissed = true;
            cx.notify();
            return true;
        }
        if let Some(modal) = self.state.active_modal.take() {
            match modal {
                CommunityModal::Composer => {
                    if self.state.active_input.is_composer_input() {
                        self.state.blur_input();
                    }
                }
                CommunityModal::CreateCommunity => {
                    if self.state.active_input.is_create_comm_input() {
                        self.state.blur_input();
                    }
                }
                CommunityModal::EditProfile => {
                    if self.state.active_input.is_profile_edit_input() {
                        self.state.blur_input();
                    }
                }
            }
            cx.notify();
            return true;
        }
        if self.state.list_editor_open {
            self.close_list_editor(cx);
            return true;
        }
        if self.state.bookmark_moving_post_id.is_some() {
            self.state.bookmark_moving_post_id = None;
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
                self.state.explore_show_discovery = q.trim().is_empty();
                self.execute_search(q, self.state.search_tab, cx);
            }
            CommunityRoute::Notifications => self.load_notifications(cx),
            CommunityRoute::BookmarksLists => self.load_saved_posts(cx),
            CommunityRoute::ModQueue(slug) => {
                let target = slug
                    .clone()
                    .or_else(|| self.moderated_communities().first().map(|c| c.slug.clone()));
                if let Some(s) = target {
                    self.load_mod_queue(s, cx);
                } else {
                    self.state.mod_slug = None;
                    self.state.mod_community = None;
                    self.state.mod_queue.clear();
                    self.state.mod_logs.clear();
                }
            }
            CommunityRoute::BotStudio => {
                self.ensure_bot_studio_selected_hydrated(cx);
            }
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
                    Ok((Ok(mut resp), mode)) => {
                        this.state.runtime_mode = mode;
                        this.state.overlay_in_flight_feed_items(&mut resp.items);
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
                if let Ok(Ok(mut resp)) = res {
                    this.state.overlay_in_flight_feed_items(&mut resp.items);
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
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        self.load_feed(cx);
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_algo_prefs(&sanitized).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.algo_prefs_gen != my_algo_gen
                    || this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
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
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res =
                run_on_community_tokio(&handle, async move { facade.create_post(&req).await }).await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
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
                        if this.state.feed_loading || this.state.feed_loading_more {
                            this.load_feed(cx);
                        }
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

    pub(super) fn reload_detail_if_loading_post(
        &mut self,
        _post_id: &str,
        allow_saved_reload: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.feed_loading || self.state.feed_loading_more {
            self.load_feed(cx);
        }
        match self.state.route.clone() {
            CommunityRoute::PostDetail(id) => {
                if self.state.detail_loading {
                    self.load_post_detail(id, cx);
                }
            }
            CommunityRoute::CommunityPage(slug) => {
                if self.state.community_loading || self.state.community_loading_more {
                    self.load_community_page(slug, cx);
                }
            }
            CommunityRoute::ActorProfile(handle) => {
                if self.state.profile_loading {
                    self.load_actor_profile(handle, cx);
                }
            }
            CommunityRoute::Explore { .. } => {
                if self.state.search_loading {
                    let q = self.state.explore_search_draft.trim().to_string();
                    self.execute_search(q, self.state.search_tab, cx);
                }
            }
            CommunityRoute::BookmarksLists => {
                // CRITICAL: Pass `allow_saved_reload = false` ONLY from `flush_bookmark_sync` so
                // F2/F3 Order 2 `bookmark_recent_overrides` and `R8-7B` stay untouched; non-bookmark
                // post mutations pass `true` to prevent Case B wipes on the Saved tab.
                if allow_saved_reload && self.state.saved_loading {
                    self.load_saved_posts(cx);
                }
                if self.state.list_feed_loading {
                    if let Some(lid) = self.state.selected_list_id.clone() {
                        self.load_selected_list_feed(lid, cx);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn reload_comment_route_if_loading(
        &mut self,
        _detail_pid: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        match self.state.route.clone() {
            CommunityRoute::PostDetail(id) => {
                if self.state.detail_loading {
                    self.load_post_detail(id, cx);
                }
            }
            CommunityRoute::ActorProfile(handle) => {
                if self.state.profile_loading {
                    self.load_actor_profile(handle, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn reload_recommended_actors(&mut self, cx: &mut Context<Self>) {
        self.state.recommended_actors_gen = self.state.recommended_actors_gen.wrapping_add(1);
        self.state.recommended_actors_loading = true;
        let my_rec_gen = self.state.recommended_actors_gen;
        let my_actor_gen = self.state.actor_switch_gen;
        let facade = self.facade.clone();
        let rt_handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&rt_handle, async move {
                let bots = facade
                    .search(&CommunitySearchRequestDto {
                        query: String::new(),
                        tab: CommunitySearchTab::Bots,
                        limit: 8,
                        ..Default::default()
                    })
                    .await
                    .ok();
                let people = facade
                    .search(&CommunitySearchRequestDto {
                        query: String::new(),
                        tab: CommunitySearchTab::People,
                        limit: 8,
                        ..Default::default()
                    })
                    .await
                    .ok();
                (bots, people)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.recommended_actors_gen != my_rec_gen
                    || this.state.actor_switch_gen != my_actor_gen
                {
                    return;
                }
                this.state.recommended_actors_loading = false;
                if let Ok((bots, people)) = res {
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
                        for a in &mut rec {
                            this.state.overlay_in_flight_actor_mutations(a);
                        }
                        this.state.recommended_actors = rec;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn reload_actor_route_if_loading(&mut self, cx: &mut Context<Self>) {
        if self.state.recommended_actors_loading {
            self.reload_recommended_actors(cx);
        }
        match self.state.route.clone() {
            CommunityRoute::ActorProfile(handle) => {
                if self.state.profile_loading {
                    self.load_actor_profile(handle, cx);
                }
            }
            CommunityRoute::CommunityPage(slug) => {
                if self.state.community_loading {
                    self.load_community_page(slug, cx);
                }
            }
            CommunityRoute::Explore { .. } => {
                if self.state.search_loading {
                    let q = self.state.explore_search_draft.trim().to_string();
                    self.execute_search(q, self.state.search_tab, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn reload_communities_list(&mut self, cx: &mut Context<Self>) {
        self.state.communities_gen = self.state.communities_gen.wrapping_add(1);
        self.state.communities_loading = true;
        let my_comm_gen = self.state.communities_gen;
        let my_actor_gen = self.state.actor_switch_gen;
        let facade = self.facade.clone();
        let rt_handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&rt_handle, async move {
                facade.list_communities(None, Some(50)).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.communities_gen != my_comm_gen
                    || this.state.actor_switch_gen != my_actor_gen
                {
                    return;
                }
                this.state.communities_loading = false;
                if let Ok(Ok(mut c)) = res {
                    for comm in &mut c.communities {
                        this.state.overlay_in_flight_community_mutations(comm);
                    }
                    this.state.communities = c.communities;
                    if matches!(this.state.route, CommunityRoute::ModQueue(None)) {
                        if let Some(s) = this
                            .moderated_communities()
                            .first()
                            .map(|cm| cm.slug.clone())
                        {
                            this.load_mod_queue(s, cx);
                        } else {
                            this.state.mod_slug = None;
                            this.state.mod_community = None;
                            this.state.mod_queue.clear();
                            this.state.mod_logs.clear();
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn reload_community_join_route_if_loading(
        &mut self,
        slug: &str,
        cx: &mut Context<Self>,
    ) {
        if self.state.communities_loading {
            self.reload_communities_list(cx);
        }
        if self.state.community_loading
            && matches!(
                &self.state.route,
                CommunityRoute::CommunityPage(s) if s.eq_ignore_ascii_case(slug)
            )
        {
            self.load_community_page(slug.to_string(), cx);
        }
        if self.state.mod_loading
            && self
                .state
                .mod_slug
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(slug))
        {
            self.load_mod_queue(slug.to_string(), cx);
        }
        if self.state.search_loading && matches!(self.state.route, CommunityRoute::Explore { .. }) {
            let q = self.state.explore_search_draft.trim().to_string();
            self.execute_search(q, self.state.search_tab, cx);
        }
    }

    pub fn vote_post(&mut self, post_id: String, clicked_dir: i8, cx: &mut Context<Self>) {
        let (next_dir, snap) = self.state.apply_optimistic_vote(&post_id, clicked_dir);
        if !self.state.post_vote_last_synced.contains_key(&post_id) {
            if let Some(orig) = snap {
                self.state
                    .post_vote_last_synced
                    .insert(post_id.clone(), orig);
            }
        }
        self.state
            .post_vote_pending_dir
            .insert(post_id.clone(), next_dir);
        cx.notify();
        if self.state.post_vote_in_flight.contains_key(&post_id) {
            self.state.post_vote_dirty.insert(post_id);
            return;
        }
        self.flush_post_vote_sync(post_id, cx);
    }

    fn flush_post_vote_sync(&mut self, post_id: String, cx: &mut Context<Self>) {
        let Some(target_dir) = self.state.post_vote_pending_dir.get(&post_id).copied() else {
            self.state.post_vote_dirty.remove(&post_id);
            self.state.post_vote_last_synced.remove(&post_id);
            return;
        };
        self.state.post_vote_dirty.remove(&post_id);
        self.state.post_vote_flight_seq = self.state.post_vote_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.post_vote_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .post_vote_in_flight
            .insert(post_id.clone(), my_flight_seq);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .vote_post(&pid, &VoteRequest { direction: target_dir })
                    .await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                if this.state.post_vote_in_flight.get(&post_id).copied() != Some(my_flight_seq) {
                    return;
                }
                this.state.post_vote_in_flight.remove(&post_id);
                match res {
                    Ok(Ok(resp)) => {
                        this.state.post_vote_last_synced.insert(
                            post_id.clone(),
                            (resp.direction, resp.upvotes, resp.downvotes, resp.score),
                        );
                        let is_dirty = this.state.post_vote_dirty.remove(&post_id);
                        if is_dirty
                            && this
                                .state
                                .post_vote_pending_dir
                                .get(&post_id)
                                .copied()
                                .is_some_and(|want| want != target_dir)
                        {
                            this.flush_post_vote_sync(post_id, cx);
                            return;
                        }
                        this.state.post_vote_pending_dir.remove(&post_id);
                        this.state.post_vote_last_synced.remove(&post_id);
                        this.state.reconcile_vote(&resp);
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
                    Ok(Err(e)) => {
                        this.state.post_vote_dirty.remove(&post_id);
                        this.state.post_vote_pending_dir.remove(&post_id);
                        if let Some(snap) = this.state.post_vote_last_synced.remove(&post_id) {
                            this.state.rollback_vote(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {
                        this.state.post_vote_dirty.remove(&post_id);
                        this.state.post_vote_pending_dir.remove(&post_id);
                        if let Some(snap) = this.state.post_vote_last_synced.remove(&post_id) {
                            this.state.rollback_vote(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toggle_like(&mut self, post_id: String, cx: &mut Context<Self>) {
        let baseline_liked = self
            .state
            .post_like_pending_liked
            .get(&post_id)
            .copied()
            .or_else(|| self.state.find_post(&post_id).map(|p| p.viewer_liked))
            .unwrap_or(false);
        let next_liked = !baseline_liked;
        if !self.state.post_like_last_synced.contains_key(&post_id) {
            if let Some(p) = self.state.find_post(&post_id) {
                self.state
                    .post_like_last_synced
                    .insert(post_id.clone(), (p.viewer_liked, p.likes_count));
            }
        }
        self.state
            .post_like_pending_liked
            .insert(post_id.clone(), next_liked);
        let _ = self.state.apply_optimistic_like_target(&post_id, next_liked);
        cx.notify();
        if self.state.post_like_in_flight.contains_key(&post_id) {
            if !self.state.post_like_pending_toggle.remove(&post_id) {
                self.state.post_like_pending_toggle.insert(post_id);
            }
            return;
        }
        self.flush_post_like_sync(post_id, cx);
    }

    fn flush_post_like_sync(&mut self, post_id: String, cx: &mut Context<Self>) {
        self.state.post_like_flight_seq = self.state.post_like_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.post_like_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .post_like_in_flight
            .insert(post_id.clone(), my_flight_seq);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.toggle_post_like(&pid).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                if this.state.post_like_in_flight.get(&post_id).copied() != Some(my_flight_seq) {
                    return;
                }
                this.state.post_like_in_flight.remove(&post_id);
                match res {
                    Ok(Ok(resp)) => {
                        this.state
                            .post_like_last_synced
                            .insert(post_id.clone(), (resp.active, resp.count));
                        if this.state.post_like_pending_toggle.remove(&post_id) {
                            this.flush_post_like_sync(post_id, cx);
                            return;
                        }
                        this.state.post_like_pending_liked.remove(&post_id);
                        this.state.post_like_last_synced.remove(&post_id);
                        this.state.reconcile_like(&resp);
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
                    Ok(Err(e)) => {
                        this.state.post_like_pending_toggle.remove(&post_id);
                        this.state.post_like_pending_liked.remove(&post_id);
                        if let Some(snap) = this.state.post_like_last_synced.remove(&post_id) {
                            this.state.rollback_like(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {
                        this.state.post_like_pending_toggle.remove(&post_id);
                        this.state.post_like_pending_liked.remove(&post_id);
                        if let Some(snap) = this.state.post_like_last_synced.remove(&post_id) {
                            this.state.rollback_like(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn repost_post(&mut self, post_id: String, cx: &mut Context<Self>) {
        let baseline_reposted = self
            .state
            .post_repost_pending_target
            .get(&post_id)
            .copied()
            .or_else(|| self.state.find_post(&post_id).map(|p| p.viewer_reposted))
            .unwrap_or(false);
        let next_reposted = !baseline_reposted;
        if !self.state.post_repost_last_synced.contains_key(&post_id) {
            if let Some(p) = self.state.find_post(&post_id) {
                self.state
                    .post_repost_last_synced
                    .insert(post_id.clone(), (p.viewer_reposted, p.reposts_count));
            }
        }
        self.state
            .post_repost_pending_target
            .insert(post_id.clone(), next_reposted);
        let _ = self
            .state
            .apply_optimistic_repost_target(&post_id, next_reposted);
        cx.notify();
        if self.state.post_repost_in_flight.contains_key(&post_id) {
            if !self.state.post_repost_pending_toggle.remove(&post_id) {
                self.state.post_repost_pending_toggle.insert(post_id);
            }
            return;
        }
        self.flush_post_repost_sync(post_id, cx);
    }

    fn flush_post_repost_sync(&mut self, post_id: String, cx: &mut Context<Self>) {
        self.state.post_repost_flight_seq = self.state.post_repost_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.post_repost_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .post_repost_in_flight
            .insert(post_id.clone(), my_flight_seq);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.repost(&pid, &RepostRequest::default()).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                if this.state.post_repost_in_flight.get(&post_id).copied() != Some(my_flight_seq) {
                    return;
                }
                this.state.post_repost_in_flight.remove(&post_id);
                match res {
                    Ok(Ok(resp)) => {
                        this.state
                            .post_repost_last_synced
                            .insert(post_id.clone(), (resp.reposted, resp.reposts_count));
                        if this.state.post_repost_pending_toggle.remove(&post_id) {
                            this.flush_post_repost_sync(post_id, cx);
                            return;
                        }
                        this.state.post_repost_pending_target.remove(&post_id);
                        this.state.post_repost_last_synced.remove(&post_id);
                        this.state.reconcile_repost(&resp);
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
                    Ok(Err(e)) => {
                        this.state.post_repost_pending_toggle.remove(&post_id);
                        this.state.post_repost_pending_target.remove(&post_id);
                        if let Some(snap) = this.state.post_repost_last_synced.remove(&post_id) {
                            this.state.rollback_repost(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {
                        this.state.post_repost_pending_toggle.remove(&post_id);
                        this.state.post_repost_pending_target.remove(&post_id);
                        if let Some(snap) = this.state.post_repost_last_synced.remove(&post_id) {
                            this.state.rollback_repost(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
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
        let quoted_pid = quoted_post_id.clone();
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
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
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
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
                        this.reload_detail_if_loading_post(&quoted_pid, true, cx);
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
            .bookmark_pending_target
            .get(&post_id)
            .map(|(bookmarked, _)| *bookmarked)
            .unwrap_or_else(|| {
                self.state
                    .find_post(&post_id)
                    .is_some_and(|p| p.viewer_bookmarked)
            });
        if already {
            self.enqueue_bookmark_mutation(post_id, false, None, cx);
        } else {
            self.enqueue_bookmark_mutation(post_id, true, Some("All".to_string()), cx);
        }
    }

    pub fn vote_poll(&mut self, post_id: String, option_id: String, cx: &mut Context<Self>) {
        let poll_ref = self
            .state
            .find_post(&post_id)
            .and_then(|p| p.poll.as_ref())
            .cloned();
        let option_ids = if let Some(ref poll) = poll_ref {
            if poll.multi_select {
                let mut selected = self
                    .state
                    .poll_vote_pending_options
                    .get(&post_id)
                    .cloned()
                    .unwrap_or_else(|| poll.viewer_voted_option_ids.clone());
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
        if !self.state.poll_vote_last_synced.contains_key(&post_id) {
            if let Some(poll) = poll_ref {
                self.state.poll_vote_last_synced.insert(post_id.clone(), poll);
            }
        }
        self.state
            .poll_vote_pending_options
            .insert(post_id.clone(), option_ids.clone());
        self.state
            .apply_optimistic_poll_options(&post_id, &option_ids);
        cx.notify();
        if self.state.poll_vote_in_flight.contains_key(&post_id) {
            self.state.poll_vote_dirty.insert(post_id);
            return;
        }
        self.flush_poll_vote_sync(post_id, cx);
    }

    fn flush_poll_vote_sync(&mut self, post_id: String, cx: &mut Context<Self>) {
        let Some(target_option_ids) = self
            .state
            .poll_vote_pending_options
            .get(&post_id)
            .cloned()
        else {
            self.state.poll_vote_dirty.remove(&post_id);
            self.state.poll_vote_last_synced.remove(&post_id);
            return;
        };
        self.state.poll_vote_dirty.remove(&post_id);
        self.state.poll_vote_flight_seq = self.state.poll_vote_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.poll_vote_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .poll_vote_in_flight
            .insert(post_id.clone(), my_flight_seq);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        let req_options = target_option_ids.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .vote_poll(&pid, &CommunityPollVoteRequest { option_ids: req_options })
                    .await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                if this.state.poll_vote_in_flight.get(&post_id).copied() != Some(my_flight_seq) {
                    return;
                }
                this.state.poll_vote_in_flight.remove(&post_id);
                match res {
                    Ok(Ok(poll)) => {
                        this.state
                            .poll_vote_last_synced
                            .insert(post_id.clone(), poll.clone());
                        let is_dirty = this.state.poll_vote_dirty.remove(&post_id);
                        if is_dirty
                            && this
                                .state
                                .poll_vote_pending_options
                                .get(&post_id)
                                .is_some_and(|want| want != &target_option_ids)
                        {
                            this.flush_poll_vote_sync(post_id, cx);
                            return;
                        }
                        this.state.poll_vote_pending_options.remove(&post_id);
                        this.state.poll_vote_last_synced.remove(&post_id);
                        this.state.reconcile_poll(&post_id, poll);
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
                    Ok(Err(e)) => {
                        this.state.poll_vote_dirty.remove(&post_id);
                        this.state.poll_vote_pending_options.remove(&post_id);
                        if let Some(snap) = this.state.poll_vote_last_synced.remove(&post_id) {
                            this.state.reconcile_poll(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {
                        this.state.poll_vote_dirty.remove(&post_id);
                        this.state.poll_vote_pending_options.remove(&post_id);
                        if let Some(snap) = this.state.poll_vote_last_synced.remove(&post_id) {
                            this.state.reconcile_poll(&post_id, snap);
                        }
                        this.reload_detail_if_loading_post(&post_id, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn give_award(&mut self, post_id: String, award_kind: String, cx: &mut Context<Self>) {
        self.state.active_popover = None;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let pid_cb = post_id.clone();
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
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                match res {
                    Ok(Ok(resp)) => {
                        this.state.reconcile_awards(&resp);
                        this.reload_detail_if_loading_post(&pid_cb, true, cx);
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

        // Monotonic ABA-proof actor-switch epoch — fenced across all actor-scoped callbacks.
        // Intentionally does NOT bump `identities_gen` (account-wide owned identities list).
        self.state.actor_switch_gen = self.state.actor_switch_gen.wrapping_add(1);
        let my_actor_gen = self.state.actor_switch_gen;

        // 1. Immediately revoke ALL cached viewer_role fields synchronously so unauthorized actors
        //    never retain the previous actor's moderator permissions on any route.
        for c in &mut self.state.communities {
            c.viewer_role = None;
        }
        if let Some(ref mut det) = self.state.community_detail {
            det.community.viewer_role = None;
        }
        if let Some(ref mut sr) = self.state.search_result {
            for c in &mut sr.communities {
                c.viewer_role = None;
            }
        }
        self.state.mod_community = None;
        self.state.mod_queue.clear();
        self.state.mod_logs.clear();
        self.state.mod_error = None;
        if !matches!(self.state.route, CommunityRoute::ModQueue(Some(_))) {
            self.state.mod_slug = None;
        }

        // 2. Unconditionally bump ALL actor-scoped generation counters & reset loading/busy flags
        //    BEFORE active-route reloads so NO stale off-route fetch or mutation can write back.
        self.state.mod_gen = self.state.mod_gen.wrapping_add(1);
        self.state.automod_rules_gen = self.state.automod_rules_gen.wrapping_add(1);
        self.state.mod_loading = false;
        self.state.automod_draft.saving = false;
        self.state.automod_draft.error = None;
        self.state.automod_in_flight.clear();
        self.state.automod_dirty.clear();
        self.state.automod_reset_draft_pending.clear();
        self.state.automod_last_synced.clear();
        self.state.post_vote_in_flight.clear();
        self.state.post_vote_dirty.clear();
        self.state.post_vote_pending_dir.clear();
        self.state.post_vote_last_synced.clear();
        self.state.quick_post_submitting = false;
        self.state.hidden_post_ids.clear();
        self.state.post_like_in_flight.clear();
        self.state.post_like_pending_toggle.clear();
        self.state.post_like_pending_liked.clear();
        self.state.post_like_last_synced.clear();
        self.state.post_repost_in_flight.clear();
        self.state.post_repost_pending_toggle.clear();
        self.state.post_repost_pending_target.clear();
        self.state.post_repost_last_synced.clear();
        self.state.poll_vote_in_flight.clear();
        self.state.poll_vote_dirty.clear();
        self.state.poll_vote_pending_options.clear();
        self.state.poll_vote_last_synced.clear();
        self.state.comment_vote_in_flight.clear();
        self.state.comment_vote_dirty.clear();
        self.state.comment_vote_pending_dir.clear();
        self.state.comment_vote_last_synced.clear();
        self.state.comment_like_in_flight.clear();
        self.state.comment_like_pending_toggle.clear();
        self.state.comment_like_pending_liked.clear();
        self.state.comment_like_last_synced.clear();

        self.state.community_gen = self.state.community_gen.wrapping_add(1);
        self.state.communities_loading = false;
        self.state.community_loading = false;
        self.state.community_loading_more = false;
        self.state.community_wiki_editing = false;
        self.state.community_wiki_saving = false;
        self.state.community_wiki_save_seq = self.state.community_wiki_save_seq.wrapping_add(1);
        self.state.community_rules_saving = false;
        self.state.community_rules_save_seq = self.state.community_rules_save_seq.wrapping_add(1);
        self.state.create_community.submitting = false;
        self.state.community_join_in_flight.clear();
        self.state.community_join_dirty.clear();
        self.state.community_join_pending_target.clear();
        self.state.community_join_last_synced.clear();

        self.state.profile_gen = self.state.profile_gen.wrapping_add(1);
        self.state.profile_loading = false;
        self.state.recommended_actors_gen = self.state.recommended_actors_gen.wrapping_add(1);
        self.state.recommended_actors_loading = false;
        self.state.edit_profile.submitting = false;
        self.state.actor_mute_in_flight.clear();
        self.state.actor_mute_dirty.clear();
        self.state.actor_mute_pending_target.clear();
        self.state.actor_mute_last_synced.clear();
        self.state.actor_block_in_flight.clear();
        self.state.actor_block_dirty.clear();
        self.state.actor_block_pending_target.clear();
        self.state.actor_block_chain_had_block.clear();
        self.state.actor_block_last_synced.clear();
        self.state.actor_follow_in_flight.clear();
        self.state.actor_follow_dirty.clear();
        self.state.actor_follow_pending_target.clear();
        self.state.actor_follow_last_synced.clear();

        // CRITICAL: Do NOT clear `detail_post = None` synchronously — same-tick reply/delete
        // calls right after `switch_active_actor` on PostDetail depend on `detail_post` staying
        // available until the fresh actor-scoped `load_post_detail` completes.
        self.state.detail_gen = self.state.detail_gen.wrapping_add(1);
        self.state.detail_loading = false;
        self.state.detail_edit_comment_saving = false;
        self.state.detail_edit_comment_save_seq =
            self.state.detail_edit_comment_save_seq.wrapping_add(1);
        self.state.detail_summon_pending = false;
        self.state.detail_summon_feedback = None;
        self.state.detail_loading_more_comment_ids.clear();
        self.state.detail_comment_more_cursors.clear();

        self.state.search_gen = self.state.search_gen.wrapping_add(1);
        self.state.search_loading = false;

        self.state.algo_prefs_gen = self.state.algo_prefs_gen.wrapping_add(1);

        self.state.saved_gen = self.state.saved_gen.wrapping_add(1);
        self.state.saved_loading = false;
        self.state.saved_items.clear();
        self.state.bookmark_moving_post_id = None;
        self.state.bookmark_state_epoch = self.state.bookmark_state_epoch.wrapping_add(1);
        self.state.bookmark_in_flight.clear();
        self.state.bookmark_dirty.clear();
        self.state.bookmark_pending_target.clear();
        self.state.bookmark_last_synced.clear();
        self.state.bookmark_cached_items.clear();
        self.state.bookmark_recent_overrides.clear();

        self.state.lists_gen = self.state.lists_gen.wrapping_add(1);
        self.state.list_feed_gen = self.state.list_feed_gen.wrapping_add(1);
        self.state.list_feed_loading = false;
        self.state.list_feed_items.clear();
        self.state.list_membership_gen = self.state.list_membership_gen.wrapping_add(1);
        self.state.list_membership_in_flight.clear();
        self.state.list_membership_dirty.clear();
        self.state.list_membership_needs_feed_reload.clear();
        self.state.list_membership_last_synced.clear();
        self.state
            .list_membership_last_synced_editor_pinned_flight
            .clear();
        self.state.list_membership_toggle_seq.clear();
        self.state.list_deleting.clear();
        self.state.list_editor.submitting = false;
        let next_actor_id = self.state.active_actor_id.clone();
        self.state
            .lists
            .retain(|l| !l.is_private || l.owner_actor_id == next_actor_id);
        if let Some(ref sel) = self.state.selected_list_id {
            if !self.state.lists.iter().any(|l| &l.list_id == sel) {
                self.state.selected_list_id = None;
            }
        }

        self.state.composer.drafts_gen = self.state.composer.drafts_gen.wrapping_add(1);
        self.state.composer.drafts_loading = false;
        self.state.composer.submitting = false;
        self.state.composer.drafts.clear();
        self.state.bot_studio_test_submitting = false;

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

        // Generation-guarded communities refresh for the new actor's viewer_role memberships,
        // and automatic ModQueue re-evaluation when switching identities on ModQueue.
        self.reload_communities_list(cx);

        // Off-route concurrent refresh of lists for the new actor (e.g. LeftNav pinned lists).
        if !matches!(self.state.route, CommunityRoute::BookmarksLists) {
            let my_lists_gen = self.state.lists_gen;
            let list_facade = self.facade.clone();
            let list_rt = list_facade.runtime_handle().clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res =
                    run_on_community_tokio(&list_rt, async move { list_facade.list_lists().await })
                        .await;
                let _ = weak.update(app, |this, cx| {
                    if this.state.lists_gen != my_lists_gen
                        || this.state.actor_switch_gen != my_actor_gen
                    {
                        return;
                    }
                    if let Ok(Ok(l)) = res {
                        this.state.merge_refreshed_lists(l.lists);
                        cx.notify();
                    }
                });
            })
            .detach();
        }

        // Reload active non-feed route for the newly selected actor.
        let active_route = self.state.route.clone();
        match active_route {
            CommunityRoute::Feed | CommunityRoute::Notifications => {}
            CommunityRoute::ModQueue(Some(slug)) => self.load_mod_queue(slug, cx),
            CommunityRoute::ModQueue(None) => {
                // Will auto-select & load once refreshed `communities` arrives above.
                self.state.mod_slug = None;
                cx.notify();
            }
            other => self.load_data_for_route(&other, cx),
        }
    }

    pub fn toggle_join_community(&mut self, slug: String, join: bool, cx: &mut Context<Self>) {
        let slug_key = slug.to_ascii_lowercase();
        if !self.state.community_join_last_synced.contains_key(&slug_key) {
            let orig = self
                .state
                .find_community(&slug)
                .map(|c| (c.is_joined, c.member_count, c.viewer_role))
                .unwrap_or((!join, 0, None));
            self.state
                .community_join_last_synced
                .insert(slug_key.clone(), orig);
        }
        self.state
            .community_join_pending_target
            .insert(slug_key.clone(), join);
        // Optimistically updates ONLY `is_joined` & `member_count` — NEVER `viewer_role`.
        self.state.apply_optimistic_join(&slug, join);
        cx.notify();
        if self.state.community_join_in_flight.contains_key(&slug_key) {
            self.state.community_join_dirty.insert(slug_key);
            return;
        }
        self.flush_community_join_sync(slug, cx);
    }

    fn flush_community_join_sync(&mut self, slug: String, cx: &mut Context<Self>) {
        let slug_key = slug.to_ascii_lowercase();
        let Some(target_join) = self
            .state
            .community_join_pending_target
            .get(&slug_key)
            .copied()
        else {
            self.state.community_join_dirty.remove(&slug_key);
            self.state.community_join_last_synced.remove(&slug_key);
            return;
        };
        self.state.community_join_dirty.remove(&slug_key);
        self.state.community_join_flight_seq =
            self.state.community_join_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.community_join_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let submitter_actor_id = self.state.active_actor_id.clone();
        self.state
            .community_join_in_flight
            .insert(slug_key.clone(), my_flight_seq);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let req_slug = slug.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                if target_join {
                    facade.join_community(&req_slug).await
                } else {
                    facade.leave_community(&req_slug).await
                }
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != submitter_actor_id
                {
                    return;
                }
                if this.state.community_join_in_flight.get(&slug_key).copied()
                    != Some(my_flight_seq)
                {
                    return;
                }
                this.state.community_join_in_flight.remove(&slug_key);
                match res {
                    Ok(Ok(resp)) => {
                        this.state.community_join_last_synced.insert(
                            slug_key.clone(),
                            (resp.is_joined, resp.member_count, resp.viewer_role),
                        );
                        let is_dirty = this.state.community_join_dirty.remove(&slug_key);
                        if is_dirty
                            && this
                                .state
                                .community_join_pending_target
                                .get(&slug_key)
                                .copied()
                                .is_some_and(|want| want != target_join)
                        {
                            this.flush_community_join_sync(slug, cx);
                            return;
                        }
                        this.state.community_join_pending_target.remove(&slug_key);
                        this.state.community_join_last_synced.remove(&slug_key);
                        this.state.reconcile_join(&resp);
                        this.reload_community_join_route_if_loading(&resp.community_slug, cx);
                    }
                    Ok(Err(e)) => {
                        this.state.community_join_dirty.remove(&slug_key);
                        this.state.community_join_pending_target.remove(&slug_key);
                        if let Some(snap) = this.state.community_join_last_synced.remove(&slug_key)
                        {
                            this.state.rollback_join(&slug, snap);
                        }
                        this.reload_community_join_route_if_loading(&slug, cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {
                        this.state.community_join_dirty.remove(&slug_key);
                        this.state.community_join_pending_target.remove(&slug_key);
                        if let Some(snap) = this.state.community_join_last_synced.remove(&slug_key)
                        {
                            this.state.rollback_join(&slug, snap);
                        }
                        this.reload_community_join_route_if_loading(&slug, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toggle_follow_actor(&mut self, actor_id: String, follow: bool, cx: &mut Context<Self>) {
        if actor_id == self.state.active_actor_id {
            return;
        }
        if self
            .state
            .actor_block_pending_target
            .get(&actor_id)
            .copied()
            == Some(true)
            || self.state.actor_block_chain_had_block.contains(&actor_id)
            || self
                .state
                .find_actor(&actor_id)
                .is_some_and(|a| a.is_blocked)
        {
            return;
        }
        if !self.state.actor_follow_last_synced.contains_key(&actor_id) {
            let orig = self
                .state
                .find_actor(&actor_id)
                .map(|a| (a.is_following, a.followers_count))
                .unwrap_or((!follow, 0));
            self.state
                .actor_follow_last_synced
                .insert(actor_id.clone(), orig);
        }
        self.state
            .actor_follow_pending_target
            .insert(actor_id.clone(), follow);
        self.state.apply_optimistic_follow(&actor_id, follow);
        cx.notify();
        if self.state.actor_follow_in_flight.contains_key(&actor_id)
            || self.state.actor_block_in_flight.contains_key(&actor_id)
            || self.state.actor_block_dirty.contains(&actor_id)
        {
            self.state.actor_follow_dirty.insert(actor_id);
            return;
        }
        self.flush_actor_follow_sync(actor_id, cx);
    }

    pub(super) fn flush_actor_follow_sync(&mut self, actor_id: String, cx: &mut Context<Self>) {
        let Some(target_follow) = self
            .state
            .actor_follow_pending_target
            .get(&actor_id)
            .copied()
        else {
            self.state.actor_follow_dirty.remove(&actor_id);
            self.state.actor_follow_last_synced.remove(&actor_id);
            return;
        };
        self.state.actor_follow_dirty.remove(&actor_id);
        self.state.actor_follow_flight_seq = self.state.actor_follow_flight_seq.wrapping_add(1);
        let my_flight_seq = self.state.actor_follow_flight_seq;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        self.state
            .actor_follow_in_flight
            .insert(actor_id.clone(), my_flight_seq);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let aid = actor_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                if target_follow {
                    facade.follow_actor(&aid).await
                } else {
                    facade.unfollow_actor(&aid).await
                }
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                if this.state.actor_follow_in_flight.get(&actor_id).copied()
                    != Some(my_flight_seq)
                {
                    return;
                }
                this.state.actor_follow_in_flight.remove(&actor_id);
                // Cross-lane Follow -> Block serialization: update block rollback baseline from
                // settled Follow, never set `is_following = true` onto a blocked actor, and launch
                // any deferred Block flight strictly after Follow settles.
                let block_superseding = this.state.actor_block_dirty.contains(&actor_id)
                    || this
                        .state
                        .actor_block_pending_target
                        .get(&actor_id)
                        .copied()
                        == Some(true)
                    || this.state.actor_block_chain_had_block.contains(&actor_id)
                    || this
                        .state
                        .find_actor(&actor_id)
                        .is_some_and(|a| a.is_blocked);
                if block_superseding {
                    if let Ok(Ok(ref resp)) = res {
                        if let Some(entry) = this.state.actor_block_last_synced.get_mut(&actor_id) {
                            entry.1 = resp.is_following;
                            entry.2 = resp.followers_count;
                        }
                    }
                    this.state.actor_follow_dirty.remove(&actor_id);
                    this.state.actor_follow_pending_target.remove(&actor_id);
                    this.state.actor_follow_last_synced.remove(&actor_id);
                    if !this.state.actor_block_in_flight.contains_key(&actor_id)
                        && this.state.actor_block_dirty.contains(&actor_id)
                    {
                        this.flush_actor_block_sync(actor_id, cx);
                    }
                    cx.notify();
                    return;
                }
                match res {
                    Ok(Ok(resp)) => {
                        this.state.actor_follow_last_synced.insert(
                            actor_id.clone(),
                            (resp.is_following, resp.followers_count),
                        );
                        let is_dirty = this.state.actor_follow_dirty.remove(&actor_id);
                        if is_dirty
                            && this
                                .state
                                .actor_follow_pending_target
                                .get(&actor_id)
                                .copied()
                                .is_some_and(|want| want != target_follow)
                        {
                            this.flush_actor_follow_sync(actor_id, cx);
                            return;
                        }
                        this.state.actor_follow_pending_target.remove(&actor_id);
                        this.state.actor_follow_last_synced.remove(&actor_id);
                        this.state.reconcile_follow(&resp);
                        this.reload_actor_route_if_loading(cx);
                    }
                    Ok(Err(e)) => {
                        this.state.actor_follow_dirty.remove(&actor_id);
                        this.state.actor_follow_pending_target.remove(&actor_id);
                        if let Some(snap) = this.state.actor_follow_last_synced.remove(&actor_id) {
                            this.state.rollback_follow(&actor_id, snap);
                        }
                        this.reload_actor_route_if_loading(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                    }
                    Err(_) => {
                        this.state.actor_follow_dirty.remove(&actor_id);
                        this.state.actor_follow_pending_target.remove(&actor_id);
                        if let Some(snap) = this.state.actor_follow_last_synced.remove(&actor_id) {
                            this.state.rollback_follow(&actor_id, snap);
                        }
                        this.reload_actor_route_if_loading(cx);
                    }
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
        let from_notif = std::mem::take(&mut self.state.detail_opened_from_notification);
        let is_same_post = self
            .state
            .detail_post
            .as_ref()
            .is_some_and(|d| d.post.post_id == post_id);
        if !is_same_post {
            self.state.detail_post = None;
            self.state.detail_editing_comment_id = None;
            self.state.detail_edit_comment_draft.clear();
            self.state.detail_edit_comment_saving = false;
            self.state.detail_edit_comment_save_seq =
                self.state.detail_edit_comment_save_seq.wrapping_add(1);
            self.state.detail_summon_feedback = None;
            self.state.detail_reply_parent_id = None;
            if !from_notif {
                self.state.detail_focused_comment_id = None;
            }
        }
        self.state.detail_comment_more_cursors.clear();
        self.state.detail_loading_more_comment_ids.clear();
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
                    Ok(Ok(mut det)) => {
                        this.state.overlay_in_flight_post_detail(&mut det);
                        this.state.detail_post = Some(det);
                        this.state.detail_error = None;
                    }
                    Ok(Err(e)) => {
                        let msg = e.to_string();
                        this.state.detail_post = None;
                        this.state.detail_error = Some(msg.clone());
                        this.state.detail_focused_comment_id = None;
                        if from_notif {
                            let lang = this.language_id.clone();
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: format!(
                                        "{} ({msg})",
                                        tr_str(
                                            &lang,
                                            "目标帖子已被删除或不可访问",
                                            "Target post has been deleted or is unavailable",
                                        )
                                    ),
                                    is_error: true,
                                },
                                cx,
                            );
                        }
                    }
                    Err(e) => {
                        this.state.detail_post = None;
                        this.state.detail_error = Some(e.clone());
                        this.state.detail_focused_comment_id = None;
                        if from_notif {
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: e,
                                    is_error: true,
                                },
                                cx,
                            );
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn submit_detail_reply(&mut self, cx: &mut Context<Self>) {
        let Some((post_id, is_locked, status)) = self
            .state
            .detail_post
            .as_ref()
            .map(|d| (d.post.post_id.clone(), d.post.is_locked, d.post.status))
        else {
            return;
        };
        let lang = self.language_id.clone();
        if is_locked || status == PostStatus::Deleted {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(
                        &lang,
                        "该帖子已锁定或已删除，无法发表新回复",
                        "Cannot reply to a locked or deleted post",
                    )
                    .to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        let body = self.state.detail_reply_draft.trim().to_string();
        if body.is_empty() {
            return;
        }
        let parent_id = self.state.detail_reply_parent_id.clone();
        let req = CreateCommentRequest {
            parent_comment_id: parent_id,
            body,
            media: Vec::new(),
            is_mod_distinguished: false,
        };
        if let Err(err) = validate_create_comment_request(&req) {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: err.to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        self.state.detail_reply_parent_id = None;
        self.state.detail_reply_draft.clear();
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_pid = post_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.create_comment(&post_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                match res {
                    Ok(Ok(_)) => {
                        if this.state.route == CommunityRoute::PostDetail(reload_pid.clone()) {
                            this.load_post_detail(reload_pid, cx);
                        }
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

    pub fn summon_bot_on_post(&mut self, post_id: String, cx: &mut Context<Self>) {
        let bot_id = self
            .state
            .recommended_actors
            .iter()
            .find(|a| a.actor_kind.is_bot_like())
            .map(|a| a.actor_id.clone());
        self.summon_bot_with_options(
            post_id,
            bot_id,
            Some("Join this discussion with a thoughtful perspective.".to_string()),
            None,
            cx,
        );
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
            self.state.community_selected_flair = None;
            self.state.community_wiki_editing = false;
            self.state.community_wiki_preview_rev = None;
            self.state.community_wiki_saving = false;
            self.state.community_wiki_save_seq =
                self.state.community_wiki_save_seq.wrapping_add(1);
            self.state.community_rules_saving = false;
            self.state.community_rules_save_seq =
                self.state.community_rules_save_seq.wrapping_add(1);
            self.state.community_next_cursor = None;
        }
        self.state.community_error = None;
        self.state.community_loading = true;
        self.state.community_loading_more = false;
        let sort = self.state.community_sort;
        let time_window = self.state.community_time_window;
        let actor_filter = self.state.community_actor_filter;
        let flair = self.state.community_selected_flair.clone();
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
                            sort,
                            time_window,
                            actor_filter,
                            flair,
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
                    Ok((Ok(mut detail), mut feed)) => {
                        this.state
                            .overlay_in_flight_community_mutations(&mut detail.community);
                        for p in &mut detail.pinned_posts {
                            this.state.overlay_in_flight_post_mutations(p);
                        }
                        for m in &mut detail.moderators {
                            this.state.overlay_in_flight_actor_mutations(m);
                        }
                        if let Some(ref mut f) = feed {
                            this.state.overlay_in_flight_feed_items(&mut f.items);
                        }
                        this.state.community_detail = Some(detail);
                        if let Some(f) = feed {
                            this.state.community_posts = f.items;
                            this.state.community_next_cursor = f.next_cursor;
                        } else {
                            this.state.community_posts.clear();
                            this.state.community_next_cursor = None;
                        }
                        this.state.community_error = None;
                    }
                    Ok((Err(e), _)) => {
                        this.state.community_detail = None;
                        this.state.community_posts.clear();
                        this.state.community_next_cursor = None;
                        this.state.community_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.community_detail = None;
                        this.state.community_posts.clear();
                        this.state.community_next_cursor = None;
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
                    Ok(Ok(mut prof)) => {
                        this.state.overlay_in_flight_actor_mutations(&mut prof.actor);
                        for p in &mut prof.recent_posts {
                            this.state.overlay_in_flight_post_mutations(p);
                        }
                        this.state
                            .overlay_in_flight_comments(&mut prof.recent_comments);
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
        self.state.search_error = None;
        let req = build_search_request(&query, tab, self.state.search_sort);
        let req_for_filter = req.clone();
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
                match res {
                    Ok(Ok(sr)) => {
                        let mut filtered = filter_search_result_by_operators(&req_for_filter, sr);
                        for p in &mut filtered.posts {
                            this.state.overlay_in_flight_post_mutations(p);
                        }
                        for a in &mut filtered.actors {
                            this.state.overlay_in_flight_actor_mutations(a);
                        }
                        for c in &mut filtered.communities {
                            this.state.overlay_in_flight_community_mutations(c);
                        }
                        this.state.search_result = Some(filtered);
                        this.state.search_error = None;
                    }
                    Ok(Err(e)) => {
                        this.state.search_error = Some(e.to_string());
                    }
                    Err(e) => {
                        this.state.search_error = Some(e);
                    }
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
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
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
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                match res {
                    Ok(Ok(resp)) => {
                        let unread = resp.unread_count.min(u32::MAX as u64) as u32;
                        if this.state.notifications_gen == my_notif_gen {
                            this.state.notifications_loading = false;
                            this.state.unread_notifications = unread;
                            for n in &mut this.state.notifications {
                                n.is_read = true;
                            }
                            this.emit_action(
                                CommunityWorkspaceAction::UnreadCountChanged(unread),
                                cx,
                            );
                        } else if this.state.notifications_loading
                            || this.state.unread_notifications != unread
                            || this.state.notifications.iter().any(|n| !n.is_read)
                        {
                            this.load_notifications(cx);
                        }
                        cx.notify();
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: e.to_string(),
                                is_error: true,
                            },
                            cx,
                        );
                        this.load_notifications(cx);
                    }
                    Err(_) => {
                        this.load_notifications(cx);
                    }
                }
            });
        })
        .detach();
    }

    pub fn load_saved_posts(&mut self, cx: &mut Context<Self>) {
        self.state.saved_gen = self.state.saved_gen.wrapping_add(1);
        let my_gen = self.state.saved_gen;
        self.state.lists_gen = self.state.lists_gen.wrapping_add(1);
        let my_lists_gen = self.state.lists_gen;
        let my_actor_gen = self.state.actor_switch_gen;
        let expected_actor_id = self.state.active_actor_id.clone();
        let my_bookmark_epoch = self.state.bookmark_state_epoch;
        self.state.saved_loading = true;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let saved = facade
                    .get_feed(&FeedRequestDto {
                        tab: FeedTab::Saved,
                        limit: 50,
                        ..Default::default()
                    })
                    .await
                    .ok();
                let lists = facade.list_lists().await.ok();
                (saved, lists)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.saved_gen != my_gen
                    || this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != expected_actor_id
                {
                    return;
                }
                this.state.saved_loading = false;
                if let Ok((saved, lists)) = res {
                    this.state.merge_refreshed_saved_items(
                        saved.map(|s| s.items).unwrap_or_default(),
                        my_bookmark_epoch,
                    );
                    if this.state.lists_gen == my_lists_gen {
                        if let Some(l) = lists {
                            this.state.merge_refreshed_lists(l.lists);
                        }
                    }
                    if this.state.bookmarks_section == BookmarksListsSection::Lists {
                        let sel = this
                            .state
                            .selected_list_id
                            .clone()
                            .filter(|id| this.state.lists.iter().any(|l| &l.list_id == id))
                            .or_else(|| this.state.lists.first().map(|l| l.list_id.clone()));
                        if let Some(lid) = sel {
                            this.select_list_for_preview(lid, cx);
                        }
                    }
                } else if !this.state.saved_loading {
                    this.state.bookmark_recent_overrides.clear();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_mod_queue(&mut self, slug: String, cx: &mut Context<Self>) {
        let same_slug = self
            .state
            .mod_slug
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(&slug));
        self.state.mod_gen = self.state.mod_gen.wrapping_add(1);
        let my_gen = self.state.mod_gen;
        let my_automod_rules_gen = self.state.automod_rules_gen;
        self.state.mod_slug = Some(slug.clone());
        self.state.mod_loading = true;
        self.state.mod_error = None;
        if !same_slug {
            self.state.automod_draft.saving = false;
            self.state.automod_in_flight.clear();
            self.state.automod_dirty.clear();
            self.state.automod_reset_draft_pending.clear();
            self.state.automod_last_synced.clear();
        }
        let status_filter = self.state.mod_status_filter;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let detail = facade.get_community(&slug).await.ok();
                let queue = facade.get_mod_queue(&slug, status_filter, Some(40)).await;
                let logs = facade.get_mod_logs(&slug, Some(40)).await.ok();
                (detail, queue, logs)
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.mod_gen != my_gen {
                    return;
                }
                this.state.mod_loading = false;
                if let Ok((detail, queue_res, logs_res)) = res {
                    if let Some(d) = detail {
                        let mut comm = d.community;
                        this.state.overlay_in_flight_community_mutations(&mut comm);
                        let slug_key = comm.slug.to_ascii_lowercase();
                        let preserve_rules = this.state.automod_in_flight.contains_key(&slug_key)
                            || this.state.automod_dirty.contains(&slug_key)
                            || this.state.automod_rules_gen != my_automod_rules_gen;
                        if preserve_rules {
                            if let Some(prev_rules) = this
                                .state
                                .mod_community
                                .as_ref()
                                .filter(|prev| prev.slug.eq_ignore_ascii_case(&comm.slug))
                                .map(|prev| prev.automod_rules.clone())
                                .or_else(|| {
                                    this.state
                                        .communities
                                        .iter()
                                        .find(|prev| prev.slug.eq_ignore_ascii_case(&comm.slug))
                                        .map(|prev| prev.automod_rules.clone())
                                })
                            {
                                comm.automod_rules = prev_rules;
                            }
                        }
                        if let Some(pos) = this
                            .state
                            .communities
                            .iter()
                            .position(|c| c.slug.eq_ignore_ascii_case(&comm.slug))
                        {
                            this.state.communities[pos] = comm.clone();
                        }
                        this.state.mod_community = Some(comm);
                    }
                    match queue_res {
                        Ok(q) => {
                            this.state.mod_queue = q.items;
                            this.state.mod_error = None;
                        }
                        Err(e) => {
                            this.state.mod_queue.clear();
                            this.state.mod_error = Some(e.to_string());
                        }
                    }
                    this.state.mod_logs = logs_res.map(|x| x.logs).unwrap_or_default();
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
        if !self.can_moderate_community_slug(&slug) {
            let lang = self.language_id.clone();
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(
                        &lang,
                        "你当前身份没有该社区的版主权限",
                        "Active identity does not have moderator permissions for this community",
                    )
                    .to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        let req = ModActionRequest {
            action,
            target_kind,
            target_id,
            reason: Some("Reviewed in Moderation Queue".to_string()),
            flair: None,
            queue_item_id: Some(item_id),
        };
        let my_actor_gen = self.state.actor_switch_gen;
        let submitter_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_slug = slug.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.execute_mod_action(&slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != submitter_actor_id
                {
                    return;
                }
                match res {
                    Ok(Ok(_)) => {
                        if this
                            .state
                            .mod_slug
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(&reload_slug))
                        {
                            this.load_mod_queue(reload_slug, cx);
                        }
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
}

impl CommunityWorkspace {
    pub(super) fn render_secondary_header(
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
}

impl CommunityWorkspace {
    fn render_route_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.state.route {
            CommunityRoute::Feed => render_feed_view(self, cx),
            CommunityRoute::PostDetail(_) => render_post_detail_view(self, cx),
            CommunityRoute::CommunityPage(slug) => render_community_page_view(self, slug, cx),
            CommunityRoute::ActorProfile(handle) => render_actor_profile_view(self, handle, cx),
            CommunityRoute::Explore { .. } => render_explore_search_view(self, cx),
            CommunityRoute::Notifications => render_notifications_view(self, cx),
            CommunityRoute::BookmarksLists => render_bookmarks_lists_view(self, cx),
            CommunityRoute::ModQueue(_) => render_mod_queue_view(self, cx),
            CommunityRoute::BotStudio => render_bot_studio_view(self, cx),
        }
    }

    fn render_active_modal_overlay(
        &self,
        modal: CommunityModal,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match modal {
            CommunityModal::Composer => render_composer_modal(self, cx),
            CommunityModal::CreateCommunity => render_create_community_modal(self, cx),
            CommunityModal::EditProfile => render_edit_profile_modal(self, cx),
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
            CommunityPopover::AwardComment(cid) => render_comment_award_popover(self, cid, cx),
            CommunityPopover::SummonBotPicker(pid) => render_summon_bot_picker_popover(self, pid, cx),
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
                    if matches!(
                        this.state.active_input,
                        ActiveCommunityInput::QuotePostNote
                            | ActiveCommunityInput::DetailSummonPrompt
                    ) {
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
            self.state.detail_comment_more_cursors.clear();
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
        let active_modal = self.state.active_modal;
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
            .when_some(active_modal, |root, modal| {
                root.child(self.render_active_modal_overlay(modal, cx))
            })
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
