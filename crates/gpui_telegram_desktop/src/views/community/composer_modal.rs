//! Feature F18 — Full Rich Multi-Format Composer Modal (`macos_modal_card`),
//! Drafts & Scheduling, Live `@/#`/`c/` Autocomplete, Markdown Formatting Toolbar,
//! and Persona-Aware Bot Co-Author Helper.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_community::{validate_create_post_request, DeterministicBotResponder};
use gpui_telegram_proto::community_contracts::{
    ActorKind, AutoModActionKind, CommunityActorDto, CommunityPostDto, CreatePollInputDto,
    CreatePostRequest, CreateThreadPartDto, FeedRequestDto, FeedSort, FeedTab, LinkPreviewDto,
    PostFormat, PostMediaAttachmentDto, PostStatus, SubCommunityDto, UpdatePostRequest,
    VerifiedTier,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_modal_backdrop_rgba, macos_modal_card, macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::composer::formatting_bar::{FormattingBar, TextStyleKind};
use crate::views::composer::popups_ui::{render_formatting_toolbar, ComposerPopupAction};
use crate::views::stage::entities_render::render_rich_message_body;

use super::shared_widgets::{
    format_relative_time, linkify_community_slugs, render_actor_avatar, render_flair_chip,
    render_input_display,
};
use super::state::{
    ActiveCommunityInput, CommunityModal, CommunityPopover, CommunityWorkspaceAction,
    ComposerFormatTab, ComposerMediaDraft, ComposerModalState,
};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(1_700_000_000_000)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutocompleteKind {
    Mention,
    Hashtag,
    Community,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteSuggestion {
    pub kind: AutocompleteKind,
    pub insert_token: String,
    pub display_label: String,
    pub subtitle: String,
}

/// Detects an active `@mention`, `#hashtag`, or `c/community` token at the end of `body`
/// using strictly UTF-8 char-boundary-safe slicing (handles English + CJK seamlessly).
pub fn detect_autocomplete_trigger(body: &str) -> Option<(usize, AutocompleteKind, String)> {
    if body.is_empty() {
        return None;
    }
    let last_ch = body.chars().last()?;
    if last_ch.is_whitespace() {
        return None;
    }
    let token_start = body
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '(' | '[' | '{' | ',' | ';' | '：' | '，'))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(0);
    let token = body.get(token_start..)?;
    if let Some(rest) = token.strip_prefix('@') {
        if rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Some((token_start, AutocompleteKind::Mention, rest.to_lowercase()));
        }
    } else if let Some(rest) = token.strip_prefix('#') {
        if rest.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Some((token_start, AutocompleteKind::Hashtag, rest.to_lowercase()));
        }
    } else if let Some(rest) = token.strip_prefix("c/").or_else(|| token.strip_prefix("C/")) {
        if rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Some((token_start, AutocompleteKind::Community, rest.to_lowercase()));
        }
    }
    None
}

pub fn compute_composer_autocomplete(ws: &CommunityWorkspace) -> Option<(usize, Vec<AutocompleteSuggestion>)> {
    if ws.state.composer.autocomplete_dismissed {
        return None;
    }
    let (start_idx, kind, query) = detect_autocomplete_trigger(&ws.state.composer.body)?;
    let mut items = Vec::new();
    match kind {
        AutocompleteKind::Mention => {
            let mut seen = HashSet::new();
            let mut pool: Vec<&CommunityActorDto> = Vec::new();
            if let Some(ids) = ws.state.identities.as_ref() {
                pool.extend(ids.all_actors.iter());
                pool.extend(ids.bot_actors.iter());
                if let Some(h) = ids.human_actor.as_ref() {
                    pool.push(h);
                }
            }
            pool.extend(ws.state.recommended_actors.iter());
            for a in pool {
                if !seen.insert(a.handle.to_lowercase()) {
                    continue;
                }
                if query.is_empty()
                    || a.handle.to_lowercase().contains(&query)
                    || a.display_name.to_lowercase().contains(&query)
                {
                    items.push(AutocompleteSuggestion {
                        kind: AutocompleteKind::Mention,
                        insert_token: format!("@{} ", a.handle),
                        display_label: format!("@{}", a.handle),
                        subtitle: a.display_name.clone(),
                    });
                    if items.len() >= 6 {
                        break;
                    }
                }
            }
        }
        AutocompleteKind::Hashtag => {
            let mut seen = HashSet::new();
            for t in &ws.state.trending {
                let raw = t.slug_or_tag.trim_start_matches('#');
                if raw.is_empty() || !seen.insert(raw.to_lowercase()) {
                    continue;
                }
                if query.is_empty() || raw.to_lowercase().contains(&query) {
                    items.push(AutocompleteSuggestion {
                        kind: AutocompleteKind::Hashtag,
                        insert_token: format!("#{raw} "),
                        display_label: format!("#{raw}"),
                        subtitle: format!("{} posts", t.posts_count_24h),
                    });
                    if items.len() >= 6 {
                        break;
                    }
                }
            }
            if items.is_empty() && !query.is_empty() {
                items.push(AutocompleteSuggestion {
                    kind: AutocompleteKind::Hashtag,
                    insert_token: format!("#{query} "),
                    display_label: format!("#{query}"),
                    subtitle: "New hashtag".to_string(),
                });
            }
        }
        AutocompleteKind::Community => {
            for c in &ws.state.communities {
                if query.is_empty()
                    || c.slug.to_lowercase().contains(&query)
                    || c.name.to_lowercase().contains(&query)
                {
                    items.push(AutocompleteSuggestion {
                        kind: AutocompleteKind::Community,
                        insert_token: format!("c/{} ", c.slug),
                        display_label: format!("c/{}", c.slug),
                        subtitle: c.name.clone(),
                    });
                    if items.len() >= 6 {
                        break;
                    }
                }
            }
        }
    }
    if items.is_empty() {
        None
    } else {
        Some((start_idx, items))
    }
}

fn apply_markdown_format_snippet(body: &mut String, kind: TextStyleKind) {
    let snippet = match kind {
        TextStyleKind::Bold => "**bold text**",
        TextStyleKind::Italic => "*italic text*",
        TextStyleKind::Underline => "__underlined__",
        TextStyleKind::Strikethrough => "~~strikethrough~~",
        TextStyleKind::Monospace => "`inline_code`",
        TextStyleKind::Spoiler => "||spoiler text||",
        TextStyleKind::Link => "[link label](https://example.com)",
    };
    if !body.is_empty() && !body.ends_with([' ', '\n']) {
        body.push(' ');
    }
    body.push_str(snippet);
}

fn extract_domain(url: &str) -> String {
    let trimmed = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    trimmed
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("link")
        .to_string()
}

impl CommunityWorkspace {
    pub fn open_composer_modal(
        &mut self,
        initial_community: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut st = ComposerModalState::default();
        st.community_slug = initial_community;
        self.state.composer = st;
        self.state.active_popover = None;
        self.state.active_modal = Some(CommunityModal::Composer);
        self.state.focus_input(ActiveCommunityInput::ComposerBody);
        self.load_composer_drafts(cx);
        cx.notify();
    }

    pub fn open_composer_from_quick_post(&mut self, cx: &mut Context<Self>) {
        let mut st = ComposerModalState::default();
        st.title = self.state.quick_post_title.clone();
        st.body = self.state.quick_post_body.clone();
        st.community_slug = self.state.quick_post_community.clone();
        if !st.title.trim().is_empty() {
            st.tab = ComposerFormatTab::Article;
        }
        self.state.composer = st;
        self.state.active_popover = None;
        self.state.active_modal = Some(CommunityModal::Composer);
        self.state.focus_input(ActiveCommunityInput::ComposerBody);
        self.load_composer_drafts(cx);
        cx.notify();
    }

    pub fn open_composer_quote(&mut self, origin: CommunityPostDto, cx: &mut Context<Self>) {
        let mut st = ComposerModalState::default();
        st.tab = ComposerFormatTab::Quote;
        st.quoted_post_id = Some(origin.post_id.clone());
        st.community_slug = origin.community_slug.clone();
        st.origin_preview = Some(origin);
        self.state.composer = st;
        self.state.active_popover = None;
        self.state.active_modal = Some(CommunityModal::Composer);
        self.state.focus_input(ActiveCommunityInput::ComposerBody);
        cx.notify();
    }

    pub fn open_composer_crosspost(&mut self, origin: CommunityPostDto, cx: &mut Context<Self>) {
        let mut st = ComposerModalState::default();
        st.tab = ComposerFormatTab::Crosspost;
        st.crosspost_of_id = Some(origin.post_id.clone());
        st.title = origin.title.clone().unwrap_or_default();
        st.origin_preview = Some(origin);
        self.state.composer = st;
        self.state.active_popover = None;
        self.state.active_modal = Some(CommunityModal::Composer);
        self.state.focus_input(ActiveCommunityInput::ComposerTitle);
        cx.notify();
    }

    pub fn open_composer_edit(&mut self, post: CommunityPostDto, cx: &mut Context<Self>) {
        let mut st = ComposerModalState::default();
        st.editing_post_id = Some(post.post_id.clone());
        st.tab = match post.format {
            PostFormat::Article => ComposerFormatTab::Article,
            PostFormat::Media => ComposerFormatTab::Media,
            PostFormat::Link => ComposerFormatTab::Link,
            PostFormat::Poll => ComposerFormatTab::Poll,
            PostFormat::Quote => ComposerFormatTab::Quote,
            PostFormat::Crosspost => ComposerFormatTab::Crosspost,
            _ => ComposerFormatTab::Micro,
        };
        st.community_slug = post.community_slug.clone();
        st.flair = post.flair.clone();
        st.title = post.title.clone().unwrap_or_default();
        st.body = post.body.clone();
        st.is_nsfw = post.is_nsfw;
        st.is_spoiler = post.is_spoiler;
        if let Some(ts) = post.scheduled_at_ms {
            let mins = ((ts - now_unix_ms()) / 60_000).max(5);
            st.schedule_enabled = post.status == PostStatus::Scheduled;
            st.schedule_offset_mins = mins;
            st.schedule_minutes_input = mins.to_string();
        }
        self.state.composer = st;
        self.state.active_popover = None;
        self.state.active_modal = Some(CommunityModal::Composer);
        self.state.focus_input(ActiveCommunityInput::ComposerBody);
        cx.notify();
    }

    pub fn close_composer_modal(&mut self, cx: &mut Context<Self>) {
        if self.state.active_modal == Some(CommunityModal::Composer) {
            self.state.active_modal = None;
        }
        if self.state.active_input.is_composer_input() {
            self.state.blur_input();
        }
        cx.notify();
    }

    pub fn set_composer_tab(&mut self, tab: ComposerFormatTab, cx: &mut Context<Self>) {
        self.state.composer.tab = tab;
        self.state.composer.validation_error = None;
        if tab == ComposerFormatTab::Media && self.state.composer.media_items.is_empty() {
            self.state.composer.media_items.push(ComposerMediaDraft::default());
        }
        if tab == ComposerFormatTab::Poll && self.state.composer.poll_options.len() < 2 {
            while self.state.composer.poll_options.len() < 2 {
                self.state.composer.poll_options.push(String::new());
            }
        }
        if tab == ComposerFormatTab::Thread && self.state.composer.thread_parts.is_empty() {
            self.state.composer.thread_parts.push(String::new());
        }
        cx.notify();
    }

    pub fn apply_composer_autocomplete(
        &mut self,
        replace_start: usize,
        insert_token: String,
        cx: &mut Context<Self>,
    ) {
        if replace_start <= self.state.composer.body.len()
            && self.state.composer.body.is_char_boundary(replace_start)
        {
            self.state.composer.body.truncate(replace_start);
            self.state.composer.body.push_str(&insert_token);
            self.state.composer.autocomplete_dismissed = false;
            self.state.composer.validation_error = None;
            self.state.focus_input(ActiveCommunityInput::ComposerBody);
            cx.notify();
        }
    }

    pub fn apply_composer_format(&mut self, kind: TextStyleKind, cx: &mut Context<Self>) {
        if let Some(pos) = self.state.composer.active_formats.iter().position(|k| *k == kind) {
            self.state.composer.active_formats.remove(pos);
        } else {
            self.state.composer.active_formats.push(kind);
        }
        apply_markdown_format_snippet(&mut self.state.composer.body, kind);
        self.state.focus_input(ActiveCommunityInput::ComposerBody);
        cx.notify();
    }

    pub fn insert_composer_markdown_block(&mut self, snippet: &str, cx: &mut Context<Self>) {
        if !self.state.composer.body.is_empty() && !self.state.composer.body.ends_with('\n') {
            self.state.composer.body.push('\n');
        }
        self.state.composer.body.push_str(snippet);
        self.state.focus_input(ActiveCommunityInput::ComposerBody);
        cx.notify();
    }

    pub fn generate_bot_coauthor_suggestion(&mut self, cx: &mut Context<Self>) {
        let lang = self.language_id.clone();
        let bots = self.collect_composer_bots();
        let selected_bot = self
            .state
            .composer
            .co_author_bot_id
            .as_deref()
            .and_then(|bid| bots.iter().find(|b| b.actor_id == bid))
            .or_else(|| bots.first())
            .cloned();
        let Some(bot) = selected_bot else {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(&lang, "暂无可用的 Bot 协作者，请先在 Bot 工作室创建", "No Bot co-author available yet. Create one in Bot Studio.")
                        .to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        };
        self.state.composer.co_author_bot_id = Some(bot.actor_id.clone());
        let seed_title = if self.state.composer.title.trim().is_empty() {
            "Community Discussion".to_string()
        } else {
            self.state.composer.title.clone()
        };
        let seed_body = if self.state.composer.body.trim().is_empty() {
            self.state.composer.bot_prompt.trim().to_string()
        } else {
            self.state.composer.body.clone()
        };
        let prompt = if self.state.composer.bot_prompt.trim().is_empty() {
            format!("Co-author a clear insight on {seed_title}")
        } else {
            self.state.composer.bot_prompt.trim().to_string()
        };
        let synthetic_post = CommunityPostDto {
            post_id: "draft_coauthor".to_string(),
            community_id: None,
            community_slug: self.state.composer.community_slug.clone(),
            author_actor_id: self.state.active_actor_id.clone(),
            author_handle: self
                .state
                .active_actor()
                .map(|a| a.handle.clone())
                .unwrap_or_else(|| "author".to_string()),
            author_display_name: self
                .state
                .active_actor()
                .map(|a| a.display_name.clone())
                .unwrap_or_else(|| "Author".to_string()),
            author_kind: ActorKind::Human,
            author_verified_tier: VerifiedTier::None,
            author_bot_badge: None,
            author_avatar_media_id: None,
            author_avatar_shape: "classic".to_string(),
            author_avatar_color: "blue".to_string(),
            co_author_actor_id: Some(bot.actor_id.clone()),
            format: PostFormat::Article,
            status: PostStatus::Draft,
            title: Some(seed_title.clone()),
            body: if seed_body.is_empty() { seed_title } else { seed_body },
            flair: self.state.composer.flair.clone(),
            tags: Vec::new(),
            mentions: Vec::new(),
            media: Vec::new(),
            link_preview: None,
            poll: None,
            quoted_post_id: None,
            quoted_post: None,
            reposted_post_id: None,
            reposted_post: None,
            crosspost_of_id: None,
            crosspost_origin: None,
            thread_root_id: None,
            parent_post_id: None,
            thread_index: 0,
            thread_total: 1,
            is_nsfw: false,
            is_spoiler: false,
            is_pinned: false,
            is_locked: false,
            is_removed: false,
            is_mod_distinguished: false,
            upvotes: 0,
            downvotes: 0,
            score: 0,
            likes_count: 0,
            reposts_count: 0,
            quotes_count: 0,
            comments_count: 0,
            bookmarks_count: 0,
            views_count: 0,
            awards: Vec::new(),
            viewer_vote: 0,
            viewer_liked: false,
            viewer_reposted: false,
            viewer_bookmarked: false,
            viewer_bookmark_folder: None,
            edit_history: Vec::new(),
            scheduled_at_ms: None,
            created_at_ms: now_unix_ms(),
            updated_at_ms: now_unix_ms(),
        };
        let draft_text = DeterministicBotResponder::generate_reply(
            &bot,
            &synthetic_post,
            &[],
            &prompt,
        )
        .unwrap_or_else(|| {
            format!(
                "**[Co-authored with @{}]** Key takeaway: break the discussion into verifiable claims, surface concrete benchmarks, and invite counterexamples.",
                bot.handle
            )
        });
        self.state.composer.bot_coauthor_suggestion = Some(draft_text);
        cx.notify();
    }

    pub(crate) fn collect_composer_bots(&self) -> Vec<CommunityActorDto> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        if let Some(ids) = self.state.identities.as_ref() {
            for b in &ids.bot_actors {
                if b.actor_kind.is_bot_like() && seen.insert(b.actor_id.clone()) {
                    out.push(b.clone());
                }
            }
            for a in &ids.all_actors {
                if a.actor_kind.is_bot_like() && seen.insert(a.actor_id.clone()) {
                    out.push(a.clone());
                }
            }
        }
        for a in &self.state.recommended_actors {
            if a.actor_kind.is_bot_like() && seen.insert(a.actor_id.clone()) {
                out.push(a.clone());
            }
        }
        out
    }

    pub fn load_composer_drafts(&mut self, cx: &mut Context<Self>) {
        let actor_id = self.state.active_actor_id.clone();
        if actor_id.is_empty() {
            return;
        }
        self.state.composer.drafts_gen = self.state.composer.drafts_gen.wrapping_add(1);
        let my_gen = self.state.composer.drafts_gen;
        self.state.composer.drafts_loading = true;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let req = FeedRequestDto {
                tab: FeedTab::ActorPosts(actor_id),
                sort: FeedSort::New,
                limit: 100,
                ..Default::default()
            };
            let res = run_on_community_tokio(&handle, async move {
                facade.get_feed(&req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.composer.drafts_gen != my_gen {
                    return;
                }
                this.state.composer.drafts_loading = false;
                if let Ok(Ok(feed)) = res {
                    this.state.composer.drafts = feed
                        .items
                        .into_iter()
                        .map(|i| i.post)
                        .filter(|p| matches!(p.status, PostStatus::Draft | PostStatus::Scheduled))
                        .collect();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn resume_draft_into_composer(&mut self, draft: CommunityPostDto, cx: &mut Context<Self>) {
        let prev_drafts = self.state.composer.drafts.clone();
        self.open_composer_edit(draft, cx);
        self.state.composer.drafts = prev_drafts;
        self.state.composer.show_drafts_drawer = false;
        cx.notify();
    }

    pub fn publish_draft_now(&mut self, post_id: String, cx: &mut Context<Self>) {
        let req = UpdatePostRequest {
            status: Some(PostStatus::Published),
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_post(&pid, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(_)) => {
                    this.state.composer.drafts.retain(|d| d.post_id != post_id);
                    let lang = this.language_id.clone();
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "草稿已立即发布！", "Draft published!").to_string(),
                            is_error: false,
                        },
                        cx,
                    );
                    this.load_feed(cx);
                }
                Ok(Err(e)) => {
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast { text: e.to_string(), is_error: true },
                        cx,
                    );
                }
                Err(_) => {}
            });
        })
        .detach();
    }

    fn resolve_community_for_composer(&self, slug: &str) -> Option<SubCommunityDto> {
        let target = slug.trim();
        if target.is_empty() {
            return None;
        }
        if let Some(det) = self.state.community_detail.as_ref() {
            if det.community.slug.eq_ignore_ascii_case(target) {
                return Some(det.community.clone());
            }
        }
        self.state
            .communities
            .iter()
            .find(|cm| cm.slug.eq_ignore_ascii_case(target))
            .cloned()
    }

    /// Builds and pre-validates a [`CreatePostRequest`] from `self.state.composer`.
    pub fn build_composer_create_request(
        &self,
        requested_status: PostStatus,
        now_ms: i64,
    ) -> Result<CreatePostRequest, String> {
        let c = &self.state.composer;
        let lang = self.language_id.as_str();

        // Community bot policy guard (prefers freshest community_detail before cached list)
        if let Some(slug) = c.community_slug.as_deref() {
            let is_bot_actor = self
                .state
                .active_actor()
                .is_some_and(|a| a.actor_kind.is_bot_like());
            if is_bot_actor {
                let disallows_bots = self
                    .resolve_community_for_composer(slug)
                    .is_some_and(|cm| !cm.allow_bots);
                if disallows_bots {
                    return Err(tr_str(
                        lang,
                        "所选社区禁止 Bot 发帖 (allow_bots = false)，请切换为人类身份或更换社区。",
                        "Selected community disallows Bots (allow_bots = false). Switch to Human identity or choose another c/.",
                    )
                    .to_string());
                }
            }
        }

        let scheduled_at_ms = if requested_status == PostStatus::Scheduled {
            let parsed_mins = c.schedule_minutes_input.trim().parse::<i64>().unwrap_or(0);
            if parsed_mins <= 0 {
                return Err(tr_str(
                    lang,
                    "定时发布时间必须在未来（请设置大于 0 分钟的延后时间）。",
                    "Scheduled publish time must be in the future (minutes > 0).",
                )
                .to_string());
            }
            let ts = now_ms.saturating_add(parsed_mins.saturating_mul(60_000));
            if ts <= now_ms {
                return Err(tr_str(
                    lang,
                    "定时发布时间不能早于当前时间。",
                    "Scheduled timestamp cannot be in the past.",
                )
                .to_string());
            }
            Some(ts)
        } else {
            None
        };

        let format = match c.tab {
            ComposerFormatTab::Micro | ComposerFormatTab::Thread => PostFormat::Micro,
            ComposerFormatTab::Article => PostFormat::Article,
            ComposerFormatTab::Media => PostFormat::Media,
            ComposerFormatTab::Link => PostFormat::Link,
            ComposerFormatTab::Poll => PostFormat::Poll,
            ComposerFormatTab::Crosspost => PostFormat::Crosspost,
            ComposerFormatTab::Quote => PostFormat::Quote,
        };

        let media: Vec<PostMediaAttachmentDto> = if c.tab == ComposerFormatTab::Media {
            c.media_items
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.url_or_id.trim().is_empty())
                .map(|(i, m)| {
                    let raw = m.url_or_id.trim().to_string();
                    let is_url = raw.starts_with("http://")
                        || raw.starts_with("https://")
                        || raw.starts_with("file://");
                    PostMediaAttachmentDto {
                        media_id: if is_url { format!("media_{i}") } else { raw.clone() },
                        kind: if m.kind.trim().is_empty() { "image".to_string() } else { m.kind.clone() },
                        url: if is_url { Some(raw) } else { None },
                        thumbnail_url: None,
                        alt_text: Some(m.alt_text.trim().to_string()).filter(|s| !s.is_empty()),
                        width: None,
                        height: None,
                        duration_secs: None,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let link_preview = if c.tab == ComposerFormatTab::Link {
            let url = c.link_url.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(tr_str(
                    lang,
                    "请输入以 http:// 或 https:// 开头的有效链接地址。",
                    "Please enter a valid Link URL starting with http:// or https://.",
                )
                .to_string());
            }
            let domain = extract_domain(url);
            let title = if c.link_title.trim().is_empty() {
                domain.clone()
            } else {
                c.link_title.trim().to_string()
            };
            Some(LinkPreviewDto {
                url: url.to_string(),
                domain,
                title,
                description: Some(c.link_description.trim().to_string()).filter(|s| !s.is_empty()),
                image_url: None,
            })
        } else {
            None
        };

        let poll = if c.tab == ComposerFormatTab::Poll {
            let opts: Vec<String> = c
                .poll_options
                .iter()
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty())
                .collect();
            Some(CreatePollInputDto {
                question: c.poll_question.trim().to_string(),
                options: opts,
                multi_select: c.poll_multi_select,
                duration_secs: c.poll_duration_secs.clamp(300, 2_592_000),
            })
        } else {
            None
        };

        let thread_parts = if c.tab == ComposerFormatTab::Thread {
            if c.thread_parts.is_empty() {
                return Err(tr_str(
                    lang,
                    "串贴至少需要 2 节内容，请添加后续分节。",
                    "A thread requires at least 2 parts. Add a follow-up segment.",
                )
                .to_string());
            }
            let mut parts = Vec::new();
            for (idx, part_body) in c.thread_parts.iter().enumerate() {
                let trimmed = part_body.trim();
                if trimmed.is_empty() {
                    return Err(format!(
                        "{} {}/{}",
                        tr_str(lang, "串贴分节不能为空：第", "Thread segment cannot be empty: part"),
                        idx + 2,
                        c.thread_parts.len() + 1
                    ));
                }
                parts.push(CreateThreadPartDto {
                    body: trimmed.to_string(),
                    media: Vec::new(),
                    link_preview: None,
                });
            }
            parts
        } else {
            Vec::new()
        };

        let req = CreatePostRequest {
            community_slug: c.community_slug.clone(),
            format,
            title: Some(c.title.trim().to_string()).filter(|s| !s.is_empty()),
            body: c.body.trim().to_string(),
            flair: c.flair.clone(),
            tags: Vec::new(),
            media,
            link_preview,
            poll,
            quoted_post_id: if c.tab == ComposerFormatTab::Quote {
                c.quoted_post_id.clone()
            } else {
                None
            },
            reposted_post_id: None,
            crosspost_of_id: if c.tab == ComposerFormatTab::Crosspost {
                c.crosspost_of_id.clone()
            } else {
                None
            },
            thread_parts,
            co_author_actor_id: c.co_author_bot_id.clone(),
            is_nsfw: c.is_nsfw,
            is_spoiler: c.is_spoiler,
            status: requested_status,
            scheduled_at_ms,
        };
        validate_create_post_request(&req, now_ms).map_err(|e| e.to_string())?;
        Ok(req)
    }

    pub fn submit_composer(&mut self, requested_status: PostStatus, cx: &mut Context<Self>) {
        if self.state.composer.submitting {
            return;
        }
        let now_ms = now_unix_ms();
        if let Some(edit_id) = self.state.composer.editing_post_id.clone() {
            let body = self.state.composer.body.trim().to_string();
            let title = Some(self.state.composer.title.trim().to_string()).filter(|s| !s.is_empty());
            if body.is_empty() && title.is_none() {
                let lang = self.language_id.clone();
                self.state.composer.validation_error = Some(
                    tr_str(&lang, "标题和正文不能同时为空", "Title and body cannot both be empty")
                        .to_string(),
                );
                cx.notify();
                return;
            }
            let scheduled_at_ms = if requested_status == PostStatus::Scheduled {
                let mins = self
                    .state
                    .composer
                    .schedule_minutes_input
                    .trim()
                    .parse::<i64>()
                    .unwrap_or(0);
                if mins <= 0 {
                    let lang = self.language_id.clone();
                    self.state.composer.validation_error = Some(
                        tr_str(&lang, "定时时间必须大于 0 分钟", "Schedule minutes must be > 0")
                            .to_string(),
                    );
                    cx.notify();
                    return;
                }
                Some(now_ms.saturating_add(mins.saturating_mul(60_000)))
            } else {
                None
            };
            let req = UpdatePostRequest {
                title,
                body: Some(body),
                flair: self.state.composer.flair.clone(),
                is_nsfw: Some(self.state.composer.is_nsfw),
                is_spoiler: Some(self.state.composer.is_spoiler),
                status: Some(requested_status),
                scheduled_at_ms,
            };
            self.state.composer.submitting = true;
            self.state.composer.validation_error = None;
            let facade = self.facade.clone();
            let handle = facade.runtime_handle().clone();
            cx.notify();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                let res = run_on_community_tokio(&handle, async move {
                    facade.update_post(&edit_id, &req).await
                })
                .await;
                let _ = weak.update(app, |this, cx| {
                    this.state.composer.submitting = false;
                    let lang = this.language_id.clone();
                    match res {
                        Ok(Ok(updated)) => {
                            this.state.visit_post_mut(&updated.post_id, |p| *p = updated.clone());
                            this.close_composer_modal(cx);
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: tr_str(&lang, "帖子已保存！", "Post saved!").to_string(),
                                    is_error: false,
                                },
                                cx,
                            );
                            this.load_feed(cx);
                        }
                        Ok(Err(e)) => {
                            this.state.composer.validation_error = Some(e.to_string());
                            cx.notify();
                        }
                        Err(e) => {
                            this.state.composer.validation_error = Some(e);
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
            return;
        }

        let req = match self.build_composer_create_request(requested_status, now_ms) {
            Ok(r) => r,
            Err(err) => {
                self.state.composer.validation_error = Some(err);
                cx.notify();
                return;
            }
        };
        self.state.composer.submitting = true;
        self.state.composer.validation_error = None;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.create_post(&req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                this.state.composer.submitting = false;
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(resp)) => {
                        let toast = match requested_status {
                            PostStatus::Draft => tr_str(&lang, "草稿已保存", "Draft saved").to_string(),
                            PostStatus::Scheduled => {
                                tr_str(&lang, "定时帖子已排期", "Post scheduled").to_string()
                            }
                            _ => match resp.automod_action {
                                AutoModActionKind::FlagForReview => resp.automod_note.unwrap_or_else(|| {
                                    tr_str(&lang, "已发布并进入社区版务复核队列", "Published & flagged for mod review")
                                        .to_string()
                                }),
                                AutoModActionKind::Remove => resp.automod_note.unwrap_or_else(|| {
                                    tr_str(&lang, "内容被 AutoMod 规则拦截", "Blocked by AutoMod rule")
                                        .to_string()
                                }),
                                _ => tr_str(&lang, "帖子已发布到社区！", "Post published to Community!")
                                    .to_string(),
                            },
                        };
                        this.state.quick_post_title.clear();
                        this.state.quick_post_body.clear();
                        this.close_composer_modal(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: toast,
                                is_error: resp.automod_action == AutoModActionKind::Remove,
                            },
                            cx,
                        );
                        if requested_status == PostStatus::Published {
                            this.load_feed(cx);
                            if let Some(slug) = resp.post.community_slug.clone() {
                                if this
                                    .state
                                    .community_detail
                                    .as_ref()
                                    .is_some_and(|d| d.community.slug.eq_ignore_ascii_case(&slug))
                                {
                                    this.load_community_page(slug, cx);
                                }
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        this.state.composer.validation_error = Some(e.to_string());
                        cx.notify();
                    }
                    Err(e) => {
                        this.state.composer.validation_error = Some(e);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// Renders the centered `macos_modal_card` Rich Multi-Format Composer modal overlay.
pub fn render_composer_modal(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &ws.state.composer;
    let active_actor = ws.state.active_actor().cloned();
    let is_editing = c.editing_post_id.is_some();
    let drafts_count = c.drafts.len();
    let show_drafts = c.show_drafts_drawer;
    let hover_bg = pal.row_hover_rgba();

    let format_tabs: [(ComposerFormatTab, &'static str, &'static str); 8] = [
        (ComposerFormatTab::Micro, "动态", "Micro"),
        (ComposerFormatTab::Article, "长文 Markdown", "Article"),
        (ComposerFormatTab::Media, "图集/视频", "Media"),
        (ComposerFormatTab::Link, "链接", "Link"),
        (ComposerFormatTab::Poll, "投票", "Poll"),
        (ComposerFormatTab::Thread, "串贴 1/N", "Thread 1/N"),
        (ComposerFormatTab::Crosspost, "跨社区", "Crosspost"),
        (ComposerFormatTab::Quote, "引用", "Quote"),
    ];

    let selected_comm = c
        .community_slug
        .as_deref()
        .and_then(|slug| ws.resolve_community_for_composer(slug));
    let flairs = selected_comm
        .as_ref()
        .map(|cm| cm.post_flairs.clone())
        .unwrap_or_default();
    let bot_policy_warn = selected_comm.as_ref().is_some_and(|cm| !cm.allow_bots)
        && active_actor.as_ref().is_some_and(|a| a.actor_kind.is_bot_like());

    let card = macos_modal_card("community-composer-modal-card", pal)
        .w(px(pal.scaled_px(680.0).min(740.0)))
        .max_w_full()
        .max_h(px(680.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Modal Header: Title + Act-As Switcher + Drafts Toggle + Close
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon(IconName::ComposePen, 15.0, pal.accent))
                        .child(
                            div()
                                .text_size(px(14.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(if is_editing {
                                    tr_str(lang, "编辑社区帖子", "Edit Community Post")
                                } else {
                                    tr_str(lang, "富文本多格式创作器", "Rich Community Composer")
                                }),
                        )
                        .child(
                            div()
                                .id("composer-act-as-pill")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .hover(move |s| s.bg(hover_bg))
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
                                    }),
                                )
                                .when_some(active_actor, |row, actor| {
                                    row.child(render_actor_avatar(
                                        &actor.actor_id,
                                        &actor.display_name,
                                        actor.actor_kind,
                                        actor.avatar_media_id.as_deref(),
                                        &actor.avatar_shape,
                                        &actor.avatar_color,
                                        18.0,
                                        pal.modal_bg,
                                        &ws.media_base_url,
                                        true,
                                        &pal,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(format!("@{}", actor.handle)),
                                    )
                                })
                                .child(tg_icon(IconName::ChevronDown, 11.0, pal.text_secondary)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("composer-drafts-toggle")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.5))
                                .rounded_full()
                                .bg(if show_drafts {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .hover(move |s| s.bg(hover_bg))
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if show_drafts { pal.accent } else { pal.text_primary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.composer.show_drafts_drawer =
                                            !this.state.composer.show_drafts_drawer;
                                        if this.state.composer.show_drafts_drawer {
                                            this.load_composer_drafts(cx);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(format!(
                                    "{} ({drafts_count})",
                                    tr_str(lang, "草稿与定时", "Drafts & Scheduled")
                                )),
                        )
                        .child(
                            div()
                                .id("composer-modal-close")
                                .cursor_pointer()
                                .p(px(5.0))
                                .rounded_full()
                                .hover(move |s| s.bg(hover_bg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.close_composer_modal(cx);
                                    }),
                                )
                                .child(tg_icon(IconName::Close, 14.0, pal.text_secondary)),
                        ),
                ),
        )
        // Scrollable Modal Body
        .child(
            div()
                .id("composer-modal-body-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                // Drafts & Scheduled Drawer (when toggled)
                .when(show_drafts, |col| col.child(render_drafts_drawer(ws, cx)))
                // Format Tabs
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(5.0))
                        .children(format_tabs.into_iter().map(|(tab, zh, en)| {
                            let active = c.tab == tab;
                            div()
                                .id(SharedString::from(format!("composer-tab-{tab:?}")))
                                .cursor_pointer()
                                .px(px(9.5))
                                .py(px(5.0))
                                .rounded_full()
                                .bg(if active {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .hover(move |s| {
                                    if active { s.bg(rgb(pal.accent_hover)) } else { s.bg(hover_bg) }
                                })
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if active {
                                    pal.active_button_fg
                                } else {
                                    pal.text_primary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_composer_tab(tab, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        })),
                )
                // Community c/<slug> picker chips + Post Flair chips
                .child(render_community_and_flair_pickers(ws, &flairs, cx))
                .when(bot_policy_warn, |col| {
                    col.child(
                        div()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                            .text_size(px(11.5))
                            .text_color(rgb(pal.danger))
                            .child(tr_str(
                                lang,
                                "⚠️ 当前社区禁止 Bot 身份发帖 (allow_bots = false)，请切换为人类身份或更换社区。",
                                "⚠️ This community disallows Bots (allow_bots = false). Switch to Human or pick another c/.",
                            )),
                    )
                })
                // Main format-specific inputs
                .child(render_composer_fields_section(ws, cx))
                // Bot Co-Author Helper
                .child(render_bot_coauthor_section(ws, cx))
                // Flags (NSFW / Spoiler) & Schedule Picker
                .child(render_flags_and_schedule_bar(ws, cx)),
        )
        // Footer: Validation Error + Save Draft + Schedule/Publish CTA
        .child(render_composer_footer(ws, cx));

    div()
        .id("community-composer-modal-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .bg(macos_modal_backdrop_rgba(pal))
        .p(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_composer_modal(cx);
            }),
        )
        .child(card)
        .into_any_element()
}

fn render_drafts_drawer(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let drafts = ws.state.composer.drafts.clone();
    let loading = ws.state.composer.drafts_loading;

    div()
        .p(px(10.0))
        .rounded(px(10.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(lang, "我的草稿与定时帖子", "My Drafts & Scheduled Posts")),
        )
        .when(loading, |col| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(lang, "正在加载草稿…", "Loading drafts…")),
            )
        })
        .when(!loading && drafts.is_empty(), |col| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(lang, "暂无保存的草稿或定时帖子。", "No drafts or scheduled posts yet.")),
            )
        })
        .children(drafts.into_iter().map(|draft| {
            let pid = draft.post_id.clone();
            let pub_pid = pid.clone();
            let del_pid = pid.clone();
            let resume_draft = draft.clone();
            let headline = draft
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| draft.body.chars().take(64).collect());
            let badge = if draft.status == PostStatus::Scheduled {
                tr_str(lang, "定时", "Scheduled")
            } else {
                tr_str(lang, "草稿", "Draft")
            };
            div()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(rgb(pal.dialogs_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(render_flair_chip(badge, Some("#3B82F6"), &pal))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(headline),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format_relative_time(draft.updated_at_ms, lang)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("draft-resume-{pid}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(pal.service_pill_rgba())
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.resume_draft_into_composer(resume_draft.clone(), cx);
                                    }),
                                )
                                .child(tr_str(lang, "继续编辑", "Resume")),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("draft-pub-{pid}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.accent))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.publish_draft_now(pub_pid.clone(), cx);
                                    }),
                                )
                                .child(tr_str(lang, "立即发布", "Publish Now")),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("draft-del-{pid}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                                .text_size(px(11.0))
                                .text_color(rgb(pal.danger))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.delete_post_action(del_pid.clone(), false, cx);
                                    }),
                                )
                                .child(tr_str(lang, "删除", "Delete")),
                        ),
                )
        }))
}

fn render_community_and_flair_pickers(
    ws: &CommunityWorkspace,
    flairs: &[gpui_telegram_proto::community_contracts::CommunityFlairDto],
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let selected_slug = ws.state.composer.community_slug.clone();
    let selected_flair = ws.state.composer.flair.clone();
    let hover_bg = pal.row_hover_rgba();

    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "发布到社区:", "Community:")),
                )
                .child(
                    div()
                        .id("composer-comm-none")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded_full()
                        .bg(if selected_slug.is_none() {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .text_color(rgb(if selected_slug.is_none() { pal.accent } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.composer.community_slug = None;
                                this.state.composer.flair = None;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "个人动态", "Profile Feed")),
                )
                .children(ws.state.communities.iter().take(10).map(|cm| {
                    let is_sel = selected_slug
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&cm.slug));
                    let slug = cm.slug.clone();
                    div()
                        .id(SharedString::from(format!("composer-comm-{}", cm.slug)))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded_full()
                        .bg(if is_sel {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .hover(move |s| s.bg(hover_bg))
                        .text_size(px(11.0))
                        .font_weight(if is_sel { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(rgb(if is_sel { pal.accent } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.composer.community_slug = Some(slug.clone());
                                this.state.composer.flair = None;
                                cx.notify();
                            }),
                        )
                        .child(format!("c/{}", cm.slug))
                })),
        )
        .when(!flairs.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(5.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(lang, "帖子标签 (Flair):", "Flair:")),
                    )
                    .child(
                        div()
                            .id("composer-flair-none")
                            .cursor_pointer()
                            .px(px(7.0))
                            .py(px(3.0))
                            .rounded_full()
                            .bg(if selected_flair.is_none() {
                                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                            } else {
                                pal.service_pill_rgba()
                            })
                            .text_size(px(10.5))
                            .text_color(rgb(pal.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.composer.flair = None;
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "无", "None")),
                    )
                    .children(flairs.iter().map(|f| {
                        let active = selected_flair
                            .as_deref()
                            .is_some_and(|sf| sf.eq_ignore_ascii_case(&f.text));
                        let ft = f.text.clone();
                        div()
                            .id(SharedString::from(format!("composer-flair-{}", f.id)))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded_full()
                            .bg(if active {
                                rgb(pal.accent).into()
                            } else {
                                pal.service_pill_rgba()
                            })
                            .text_size(px(10.5))
                            .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                            .text_color(rgb(if active { pal.active_button_fg } else { pal.text_primary }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.composer.flair = Some(ft.clone());
                                    cx.notify();
                                }),
                            )
                            .child(f.text.clone())
                    })),
            )
        })
}

fn render_composer_input_box(
    id: impl Into<SharedString>,
    text: &str,
    placeholder: &str,
    target: ActiveCommunityInput,
    min_h_px: f32,
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let pal = ws.palette;
    let focused = ws.state.active_input == target;
    let preedit = if focused { ws.state.ime_preedit.as_deref() } else { None };
    div()
        .id(id.into())
        .cursor_text()
        .w_full()
        .min_h(px(min_h_px))
        .px(px(10.0))
        .py(px(7.5))
        .rounded(px(8.0))
        .bg(rgb(pal.input_bg))
        .border_1()
        .border_color(if focused {
            rgb(pal.accent).into()
        } else {
            pal.hairline_border_rgba()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.state.focus_input(target);
                cx.notify();
            }),
        )
        .child(render_input_display(text, preedit, placeholder, focused, &pal))
}

fn render_composer_fields_section(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &ws.state.composer;
    let tab = c.tab;
    let body_chars = c.body.chars().count();
    let max_chars = if tab == ComposerFormatTab::Article { 40_000 } else { 2_000 };
    let title_chars = c.title.chars().count();
    let autocomplete = compute_composer_autocomplete(ws);
    let hover_bg = pal.row_hover_rgba();

    let fmt_bar = FormattingBar {
        is_visible: true,
        active_styles: c.active_formats.clone(),
    };
    let show_preview = c.show_markdown_preview;
    let linked_preview = if show_preview {
        Some(linkify_community_slugs(&c.body))
    } else {
        None
    };

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        // Origin Post Preview for Quote / Crosspost
        .when_some(
            c.origin_preview
                .as_ref()
                .filter(|_| matches!(tab, ComposerFormatTab::Quote | ComposerFormatTab::Crosspost)),
            |col, orig| {
                let snippet: String = orig.body.chars().take(160).collect();
                col.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(9.0))
                        .bg(pal.card_surface_rgba())
                        .border_l_2()
                        .border_color(rgb(pal.accent))
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "{}: {} (@{})",
                                    if tab == ComposerFormatTab::Quote {
                                        tr_str(lang, "引用原帖", "Quoting post")
                                    } else {
                                        tr_str(lang, "跨社区转自", "Crossposting from")
                                    },
                                    orig.author_display_name,
                                    orig.author_handle
                                )),
                        )
                        .when_some(orig.title.as_deref().filter(|t| !t.trim().is_empty()), |b, t| {
                            b.child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(t.to_string()),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(snippet),
                        ),
                )
            },
        )
        // Title Input (required for Article, optional for others)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(render_composer_input_box(
                    "composer-title-input",
                    &c.title,
                    if tab == ComposerFormatTab::Article {
                        tr_str(lang, "文章标题（必填，最多 300 字）…", "Article title (required, max 300 chars)…")
                    } else {
                        tr_str(lang, "标题（可选，最多 300 字）…", "Title (optional, max 300 chars)…")
                    },
                    ActiveCommunityInput::ComposerTitle,
                    34.0,
                    ws,
                    cx,
                ))
                .when(title_chars > 0 || tab == ComposerFormatTab::Article, |col| {
                    col.child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(if title_chars > 300 { pal.danger } else { pal.text_secondary }))
                            .child(format!("{title_chars} / 300")),
                    )
                }),
        )
        // Theme-aware Interactive Markdown Formatting Toolbar (Article / Micro / Thread / Quote)
        .when(
            matches!(
                tab,
                ComposerFormatTab::Article
                    | ComposerFormatTab::Micro
                    | ComposerFormatTab::Thread
                    | ComposerFormatTab::Quote
            ),
            |col| {
                col.child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(5.0))
                        .child(render_formatting_toolbar(
                            &fmt_bar,
                            &pal,
                            cx,
                            |this, action, cx| {
                                if let ComposerPopupAction::ApplyFormat(kind) = action {
                                    this.apply_composer_format(kind, cx);
                                }
                            },
                        ))
                        .child(render_md_extra_btn("cm-md-h2", "H2", "## Heading\n", &pal, cx))
                        .child(render_md_extra_btn("cm-md-quote", "> Quote", "> Quoted point\n", &pal, cx))
                        .child(render_md_extra_btn("cm-md-list", "• List", "- Item 1\n- Item 2\n", &pal, cx))
                        .child(render_md_extra_btn("cm-md-codeblk", "```", "```rust\n// code\n```\n", &pal, cx))
                        .child(
                            div()
                                .id("composer-md-preview-toggle")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(if show_preview {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if show_preview { pal.accent } else { pal.text_secondary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.composer.show_markdown_preview =
                                            !this.state.composer.show_markdown_preview;
                                        cx.notify();
                                    }),
                                )
                                .child(if show_preview {
                                    tr_str(lang, "隐藏预览", "Hide Preview")
                                } else {
                                    tr_str(lang, "Markdown 预览", "Preview")
                                }),
                        ),
                )
            },
        )
        // Main Body IME Box + Char Counter
        .child(render_composer_input_box(
            "composer-body-input",
            &c.body,
            match tab {
                ComposerFormatTab::Thread => tr_str(
                    lang,
                    "第 1 节 (1/N) 串贴正文… 支持 @提及、#话题、c/社区 自动补全",
                    "Part 1/N thread opener… Supports @mention, #hashtag, c/community autocomplete",
                ),
                ComposerFormatTab::Quote => tr_str(
                    lang,
                    "写下你的引用评论观点…",
                    "Add your quote commentary…",
                ),
                _ => tr_str(
                    lang,
                    "撰写正文内容，支持 Markdown、@提及人类或 Bot、#话题、c/社区… (Shift+Enter 换行)",
                    "Write your post with Markdown, @mentions, #hashtags, c/communities… (Shift+Enter newline)",
                ),
            },
            ActiveCommunityInput::ComposerBody,
            if tab == ComposerFormatTab::Article { 120.0 } else { 82.0 },
            ws,
            cx,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(10.5))
                .text_color(rgb(if body_chars > max_chars { pal.danger } else { pal.text_secondary }))
                .child(tr_str(lang, "输入 @ / # / c/ 触发实时自动补全", "Type @ / # / c/ for live autocomplete"))
                .child(format!("{body_chars} / {max_chars}")),
        )
        // Live @ / # / c/ Autocomplete Popover List
        .when_some(autocomplete, |col, (start_idx, suggestions)| {
            col.child(
                div()
                    .id("composer-autocomplete-popup")
                    .p(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.menu_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(suggestions.into_iter().enumerate().map(|(si, item)| {
                        let tok = item.insert_token.clone();
                        div()
                            .id(SharedString::from(format!("composer-ac-item-{si}")))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.apply_composer_autocomplete(start_idx, tok.clone(), cx);
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .child(item.display_label),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(item.subtitle),
                            )
                    })),
            )
        })
        // Optional Live Markdown Preview
        .when_some(linked_preview.filter(|s| !s.trim().is_empty()), |col, md| {
            col.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(pal.card_surface_rgba())
                    .border_1()
                    .border_color(pal.hairline_border_rgba())
                    .child(render_rich_message_body(
                        0xC0DE_5A5A,
                        &md,
                        &[],
                        &ws.state.revealed_spoilers,
                        &ws.state.expanded_quotes,
                        ws.state.copied_code_msg,
                        &pal,
                        lang,
                        false,
                        cx,
                        |this, action, cx| this.handle_rich_text_action(action, cx),
                    )),
            )
        })
        // Format-Specific Sub-Panels (Media / Link / Poll / Thread)
        .when(tab == ComposerFormatTab::Media, |col| col.child(render_media_subpanel(ws, cx)))
        .when(tab == ComposerFormatTab::Link, |col| col.child(render_link_subpanel(ws, cx)))
        .when(tab == ComposerFormatTab::Poll, |col| col.child(render_poll_subpanel(ws, cx)))
        .when(tab == ComposerFormatTab::Thread, |col| col.child(render_thread_subpanel(ws, cx)))
}

fn render_md_extra_btn(
    id: &'static str,
    label: &'static str,
    snippet: &'static str,
    pal: &TdesktopThemePalette,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let hover_bg = pal.row_hover_rgba();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(7.5))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(pal.service_pill_rgba())
        .hover(move |s| s.bg(hover_bg))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(pal.text_primary))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.insert_composer_markdown_block(snippet, cx);
            }),
        )
        .child(label)
}

fn render_media_subpanel(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let count = ws.state.composer.media_items.len();

    div()
        .p(px(10.0))
        .rounded(px(9.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!("{} ({count}/4)", tr_str(lang, "媒体附件 (1–4 项)", "Media Attachments (1–4)"))),
                )
                .when(count < 4, |row| {
                    row.child(
                        div()
                            .id("composer-media-add")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.5))
                            .rounded(px(6.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    if this.state.composer.media_items.len() < 4 {
                                        this.state.composer.media_items.push(ComposerMediaDraft::default());
                                        cx.notify();
                                    }
                                }),
                            )
                            .child(tr_str(lang, "+ 添加媒体", "+ Add Media")),
                    )
                }),
        )
        .children(ws.state.composer.media_items.iter().enumerate().map(|(i, item)| {
            let is_video = item.kind == "video";
            div()
                .p(px(8.0))
                .rounded(px(7.0))
                .bg(rgb(pal.dialogs_bg))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("#{}", i + 1)),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("composer-media-kind-{i}")))
                                        .cursor_pointer()
                                        .px(px(7.0))
                                        .py(px(2.5))
                                        .rounded(px(5.0))
                                        .bg(pal.service_pill_rgba())
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.accent))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if let Some(m) = this.state.composer.media_items.get_mut(i) {
                                                    m.kind = if m.kind == "video" {
                                                        "image".to_string()
                                                    } else {
                                                        "video".to_string()
                                                    };
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(if is_video {
                                            tr_str(lang, "视频 (video)", "Video")
                                        } else {
                                            tr_str(lang, "图片 (image)", "Image")
                                        }),
                                ),
                        )
                        .when(count > 1, |row| {
                            row.child(
                                div()
                                    .id(SharedString::from(format!("composer-media-rm-{i}")))
                                    .cursor_pointer()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(5.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.danger))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if this.state.composer.media_items.len() > 1 && i < this.state.composer.media_items.len() {
                                                this.state.composer.media_items.remove(i);
                                                if matches!(
                                                    this.state.active_input,
                                                    ActiveCommunityInput::ComposerMediaUrl(_)
                                                        | ActiveCommunityInput::ComposerMediaAlt(_)
                                                ) {
                                                    this.state.blur_input();
                                                }
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child(tr_str(lang, "移除", "Remove")),
                            )
                        }),
                )
                .child(render_composer_input_box(
                    format!("composer-media-url-{i}"),
                    &item.url_or_id,
                    tr_str(lang, "图片/视频 URL 或 media_id…", "Image/video URL or media_id…"),
                    ActiveCommunityInput::ComposerMediaUrl(i),
                    32.0,
                    ws,
                    cx,
                ))
                .child(render_composer_input_box(
                    format!("composer-media-alt-{i}"),
                    &item.alt_text,
                    tr_str(lang, "图片说明 / Alt 文本（可选）…", "Caption / Alt text (optional)…"),
                    ActiveCommunityInput::ComposerMediaAlt(i),
                    30.0,
                    ws,
                    cx,
                ))
        }))
}

fn render_link_subpanel(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &ws.state.composer;
    div()
        .p(px(10.0))
        .rounded(px(9.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(lang, "外部链接卡片", "Link Preview Card")),
        )
        .child(render_composer_input_box(
            "composer-link-url",
            &c.link_url,
            "https://example.com/article",
            ActiveCommunityInput::ComposerLinkUrl,
            34.0,
            ws,
            cx,
        ))
        .child(render_composer_input_box(
            "composer-link-title",
            &c.link_title,
            tr_str(lang, "链接预览标题（可选）…", "Link preview title (optional)…"),
            ActiveCommunityInput::ComposerLinkTitle,
            32.0,
            ws,
            cx,
        ))
        .child(render_composer_input_box(
            "composer-link-desc",
            &c.link_description,
            tr_str(lang, "摘要描述（可选）…", "Link description (optional)…"),
            ActiveCommunityInput::ComposerLinkDesc,
            32.0,
            ws,
            cx,
        ))
}

fn render_poll_subpanel(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &ws.state.composer;
    let opt_count = c.poll_options.len();
    let multi = c.poll_multi_select;
    let dur = c.poll_duration_secs;
    let durations: [(i64, &'static str); 4] = [
        (3_600, "1h"),
        (86_400, "24h"),
        (259_200, "3d"),
        (604_800, "7d"),
    ];

    div()
        .p(px(10.0))
        .rounded(px(9.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!("{} ({opt_count}/6)", tr_str(lang, "交互式投票 (2–6 项)", "Interactive Poll (2–6 options)"))),
                )
                .when(opt_count < 6, |row| {
                    row.child(
                        div()
                            .id("composer-poll-add-opt")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.5))
                            .rounded(px(6.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    if this.state.composer.poll_options.len() < 6 {
                                        this.state.composer.poll_options.push(String::new());
                                        cx.notify();
                                    }
                                }),
                            )
                            .child(tr_str(lang, "+ 添加选项", "+ Add Option")),
                    )
                }),
        )
        .child(render_composer_input_box(
            "composer-poll-question",
            &c.poll_question,
            tr_str(lang, "投票问题（必填）…", "Poll question (required)…"),
            ActiveCommunityInput::ComposerPollQuestion,
            34.0,
            ws,
            cx,
        ))
        .children(c.poll_options.iter().enumerate().map(|(i, opt)| {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div().flex_1().min_w(px(0.0)).child(render_composer_input_box(
                        format!("composer-poll-opt-{i}"),
                        opt,
                        if i == 0 {
                            tr_str(lang, "选项 1", "Option 1")
                        } else if i == 1 {
                            tr_str(lang, "选项 2", "Option 2")
                        } else {
                            tr_str(lang, "更多选项…", "Option…")
                        },
                        ActiveCommunityInput::ComposerPollOption(i),
                        32.0,
                        ws,
                        cx,
                    )),
                )
                .when(opt_count > 2, |row| {
                    row.child(
                        div()
                            .id(SharedString::from(format!("composer-poll-rm-{i}")))
                            .cursor_pointer()
                            .px(px(7.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                            .text_size(px(11.0))
                            .text_color(rgb(pal.danger))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if this.state.composer.poll_options.len() > 2 && i < this.state.composer.poll_options.len() {
                                        this.state.composer.poll_options.remove(i);
                                        if matches!(this.state.active_input, ActiveCommunityInput::ComposerPollOption(_)) {
                                            this.state.blur_input();
                                        }
                                        cx.notify();
                                    }
                                }),
                            )
                            .child("×"),
                    )
                })
        }))
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .id("composer-poll-multi-toggle")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if multi {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .text_color(rgb(if multi { pal.accent } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.composer.poll_multi_select = !this.state.composer.poll_multi_select;
                                cx.notify();
                            }),
                        )
                        .child(if multi {
                            tr_str(lang, "☑ 允许多选", "☑ Multi-select")
                        } else {
                            tr_str(lang, "☐ 单选投票", "☐ Single choice")
                        }),
                )
                .children(durations.into_iter().map(|(secs, lbl)| {
                    let active = dur == secs;
                    div()
                        .id(SharedString::from(format!("composer-poll-dur-{secs}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if active { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if active { pal.active_button_fg } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.composer.poll_duration_secs = secs;
                                cx.notify();
                            }),
                        )
                        .child(lbl)
                })),
        )
}

fn render_thread_subpanel(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let total = ws.state.composer.thread_parts.len() + 1;

    div()
        .p(px(10.0))
        .rounded(px(9.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!("{} (1/{total} .. {total}/{total})", tr_str(lang, "后续串贴分节", "Thread Continuation Parts"))),
                )
                .child(
                    div()
                        .id("composer-thread-add-part")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded(px(6.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                if this.state.composer.thread_parts.len() < 15 {
                                    this.state.composer.thread_parts.push(String::new());
                                    cx.notify();
                                }
                            }),
                        )
                        .child(tr_str(lang, "+ 添加下一节", "+ Add Thread Part")),
                ),
        )
        .children(ws.state.composer.thread_parts.iter().enumerate().map(|(i, part)| {
            let num = i + 2;
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .child(format!("{num}/{total}")),
                        )
                        .when(ws.state.composer.thread_parts.len() > 1, |row| {
                            row.child(
                                div()
                                    .id(SharedString::from(format!("composer-thread-rm-{i}")))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.danger))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if this.state.composer.thread_parts.len() > 1
                                                && i < this.state.composer.thread_parts.len()
                                            {
                                                this.state.composer.thread_parts.remove(i);
                                                if matches!(
                                                    this.state.active_input,
                                                    ActiveCommunityInput::ComposerThreadPart(_)
                                                ) {
                                                    this.state.blur_input();
                                                }
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child(tr_str(lang, "删除此节", "Remove")),
                            )
                        }),
                )
                .child(render_composer_input_box(
                    format!("composer-thread-part-{i}"),
                    part,
                    tr_str(lang, "撰写该节串贴内容…", "Write this thread segment…"),
                    ActiveCommunityInput::ComposerThreadPart(i),
                    52.0,
                    ws,
                    cx,
                ))
        }))
}

fn render_bot_coauthor_section(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let bots = ws.collect_composer_bots();
    let selected_bot_id = ws.state.composer.co_author_bot_id.clone();
    let suggestion = ws.state.composer.bot_coauthor_suggestion.clone();
    let (_, bot_bg, bot_fg) = TdesktopThemePalette::bot_hue_colors("violet", pal.mode.is_dark());

    div()
        .p(px(10.0))
        .rounded(px(9.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tg_icon(IconName::BotRobot, 13.5, bot_fg))
                        .child(tr_str(lang, "邀请 Bot 协作创作 (Bot Co-Author)", "Ask Bot Co-Author")),
                )
                .child(
                    div()
                        .id("composer-bot-coauthor-run")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(rgb(bot_bg))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(bot_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.generate_bot_coauthor_suggestion(cx);
                            }),
                        )
                        .child(tr_str(lang, "✨ 生成协作润色草稿", "✨ Draft with Bot")),
                ),
        )
        .when(!bots.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(5.0))
                    .child(
                        div()
                            .id("composer-coauthor-none")
                            .cursor_pointer()
                            .px(px(7.0))
                            .py(px(3.0))
                            .rounded_full()
                            .bg(if selected_bot_id.is_none() {
                                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                            } else {
                                pal.service_pill_rgba()
                            })
                            .text_size(px(10.5))
                            .text_color(rgb(pal.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.composer.co_author_bot_id = None;
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "无协作者", "None")),
                    )
                    .children(bots.into_iter().take(6).map(|b| {
                        let active = selected_bot_id.as_deref() == Some(b.actor_id.as_str());
                        let bid = b.actor_id.clone();
                        div()
                            .id(SharedString::from(format!("composer-coauthor-{}", b.actor_id)))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded_full()
                            .bg(if active { rgb(bot_bg).into() } else { pal.service_pill_rgba() })
                            .text_size(px(10.5))
                            .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                            .text_color(rgb(if active { bot_fg } else { pal.text_primary }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.composer.co_author_bot_id = Some(bid.clone());
                                    cx.notify();
                                }),
                            )
                            .child(format!("@{}", b.handle))
                    })),
            )
        })
        .child(render_composer_input_box(
            "composer-bot-prompt",
            &ws.state.composer.bot_prompt,
            tr_str(lang, "可选：给 Bot 协作者的指令（如：补充性能基准对比）…", "Optional Bot prompt (e.g. add benchmark tradeoffs)…"),
            ActiveCommunityInput::ComposerBotPrompt,
            30.0,
            ws,
            cx,
        ))
        .when_some(suggestion, |col, text| {
            let append_txt = text.clone();
            col.child(
                div()
                    .p(px(8.0))
                    .rounded(px(7.0))
                    .bg(rgb(pal.dialogs_bg))
                    .border_l_2()
                    .border_color(rgb(bot_fg))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(pal.text_primary))
                            .child(text),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("composer-coauthor-insert")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded(px(6.0))
                                    .bg(rgb(pal.accent))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.insert_composer_markdown_block(&append_txt, cx);
                                            this.state.composer.bot_coauthor_suggestion = None;
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "插入正文", "Insert into Body")),
                            )
                            .child(
                                div()
                                    .id("composer-coauthor-dismiss")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.5))
                                    .rounded(px(6.0))
                                    .bg(pal.service_pill_rgba())
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.state.composer.bot_coauthor_suggestion = None;
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "忽略", "Dismiss")),
                            ),
                    ),
            )
        })
}

fn render_flags_and_schedule_bar(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &ws.state.composer;
    let presets: [(i64, &'static str); 4] = [
        (15, "+15m"),
        (60, "+1h"),
        (360, "+6h"),
        (1_440, "+24h"),
    ];

    div()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .id("composer-flag-nsfw")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.5))
                        .rounded_full()
                        .bg(if c.is_nsfw {
                            TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.18)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if c.is_nsfw { pal.danger } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.composer.is_nsfw = !this.state.composer.is_nsfw;
                                cx.notify();
                            }),
                        )
                        .child(if c.is_nsfw { "☑ 18+ NSFW" } else { "☐ 18+ NSFW" }),
                )
                .child(
                    div()
                        .id("composer-flag-spoiler")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.5))
                        .rounded_full()
                        .bg(if c.is_spoiler {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if c.is_spoiler { pal.accent } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.composer.is_spoiler = !this.state.composer.is_spoiler;
                                cx.notify();
                            }),
                        )
                        .child(if c.is_spoiler {
                            tr_str(lang, "☑ 剧透遮罩", "☑ Spoiler")
                        } else {
                            tr_str(lang, "☐ 剧透遮罩", "☐ Spoiler")
                        }),
                )
                .child(
                    div()
                        .id("composer-schedule-toggle")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.5))
                        .rounded_full()
                        .bg(if c.schedule_enabled {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if c.schedule_enabled { pal.accent } else { pal.text_primary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.composer.schedule_enabled = !this.state.composer.schedule_enabled;
                                cx.notify();
                            }),
                        )
                        .child(tg_icon(IconName::ClockSending, 12.0, if c.schedule_enabled { pal.accent } else { pal.text_secondary }))
                        .child(tr_str(lang, "定时发布", "Schedule")),
                ),
        )
        .when(c.schedule_enabled, |col| {
            col.child(
                div()
                    .p(px(8.0))
                    .rounded(px(8.0))
                    .bg(pal.card_surface_rgba())
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(presets.into_iter().map(|(mins, lbl)| {
                        let active = c.schedule_offset_mins == mins;
                        div()
                            .id(SharedString::from(format!("composer-sched-preset-{mins}")))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded_full()
                            .bg(if active { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if active { pal.active_button_fg } else { pal.text_primary }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.composer.schedule_offset_mins = mins;
                                    this.state.composer.schedule_minutes_input = mins.to_string();
                                    cx.notify();
                                }),
                            )
                            .child(lbl)
                    }))
                    .child(
                        div().w(px(130.0)).child(render_composer_input_box(
                            "composer-schedule-mins-input",
                            &c.schedule_minutes_input,
                            tr_str(lang, "自定义分钟数", "Minutes from now"),
                            ActiveCommunityInput::ComposerScheduleMinutes,
                            28.0,
                            ws,
                            cx,
                        )),
                    ),
            )
        })
}

fn render_composer_footer(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &ws.state.composer;
    let err = c.validation_error.clone();
    let is_sched = c.schedule_enabled;
    let is_edit = c.editing_post_id.is_some();
    let submitting = c.submitting;

    div()
        .px(px(16.0))
        .py(px(11.0))
        .border_t_1()
        .border_color(pal.separator_rgba())
        .flex()
        .flex_col()
        .gap(px(7.0))
        .when_some(err, |col, msg| {
            col.child(
                div()
                    .px(px(9.0))
                    .py(px(6.0))
                    .rounded(px(7.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                    .text_size(px(11.5))
                    .text_color(rgb(pal.danger))
                    .child(msg),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    macos_secondary_pill_btn(
                        "composer-save-draft-btn",
                        tr_str(lang, "存为草稿", "Save Draft"),
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.submit_composer(PostStatus::Draft, cx);
                        }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            macos_secondary_pill_btn(
                                "composer-cancel-btn",
                                tr_str(lang, "取消", "Cancel"),
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_composer_modal(cx);
                                }),
                            ),
                        )
                        .child(
                            macos_primary_pill_btn(
                                "composer-publish-btn",
                                if submitting {
                                    tr_str(lang, "提交中…", "Submitting…")
                                } else if is_sched {
                                    tr_str(lang, "定时排期发布", "Schedule Post")
                                } else if is_edit {
                                    tr_str(lang, "保存修改", "Save Changes")
                                } else {
                                    tr_str(lang, "立即发布", "Publish")
                                },
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let status = if is_sched {
                                        PostStatus::Scheduled
                                    } else {
                                        PostStatus::Published
                                    };
                                    this.submit_composer(status, cx);
                                }),
                            ),
                        ),
                ),
        )
}
