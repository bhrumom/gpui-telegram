//! Feature F19 — Full `c/<slug>` Community Page (Banner, Human+Bot Stats, Join/Bell,
//! Pinned Posts, Flair Filters, Sort/Time/Actor Bars, Rules + Mod Rule Editor,
//! Versioned Markdown Wiki Viewer/Editor + Revision History, Mod Roster) and
//! "Create Community" Modal (`macos_modal_card`).

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_community::{normalize_community_slug, ModPermissionValidator};
use gpui_telegram_proto::community_contracts::{
    ActorFilter, CommunityRuleDto, CreateCommunityRequest, FeedItemDto, FeedRequestDto, FeedSort,
    FeedTab, TimeWindow, UpdateCommunityRequest, UpdateCommunityWikiRequest,
};

use crate::components::avatar::parse_fabushi_hex_color;
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_modal_backdrop_rgba, macos_modal_card, macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::stage::entities_render::render_rich_message_body;

use super::post_card::render_post_card;
use super::shared_widgets::{
    format_compact_count, format_relative_time, linkify_community_slugs, render_actor_avatar,
    render_actor_badge_row, render_empty_state, render_flair_chip, render_input_display,
    render_loading_skeleton,
};
use super::state::{
    ActiveCommunityInput, CommunityModal, CommunityPageTab, CommunityRoute,
    CommunityWorkspaceAction, CreateCommunityDraft,
};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

const COMMUNITY_CATEGORIES: [(&str, &str, &str); 8] = [
    ("tech", "技术", "Tech"),
    ("ai", "人工智能", "AI & Bots"),
    ("rust", "Rust", "Rust"),
    ("science", "科学", "Science"),
    ("gaming", "游戏", "Gaming"),
    ("art", "创意设计", "Design"),
    ("finance", "财经", "Finance"),
    ("general", "综合", "General"),
];

const COMMUNITY_ACCENTS: [&str; 8] = [
    "#3B82F6", "#8B5CF6", "#10B981", "#F59E0B", "#EF4444", "#EC4899", "#06B6D4", "#6366F1",
];

const COMMUNITY_EMOJIS: [&str; 8] = ["🚀", "🦀", "🤖", "⚡", "🧠", "🎨", "🌐", "🔬"];

impl CommunityWorkspace {
    pub fn set_community_tab(&mut self, tab: CommunityPageTab, cx: &mut Context<Self>) {
        if self.state.community_tab != tab {
            self.state.community_tab = tab;
            if tab != CommunityPageTab::Wiki {
                self.state.community_wiki_editing = false;
            }
            cx.notify();
        }
    }

    pub fn reload_community_posts(&mut self, slug: String, cx: &mut Context<Self>) {
        self.state.community_gen = self.state.community_gen.wrapping_add(1);
        let my_gen = self.state.community_gen;
        self.state.community_loading = true;
        self.state.community_loading_more = false;
        let req = FeedRequestDto {
            tab: FeedTab::Community(slug),
            sort: self.state.community_sort,
            time_window: self.state.community_time_window,
            actor_filter: self.state.community_actor_filter,
            flair: self.state.community_selected_flair.clone(),
            limit: 25,
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.get_feed(&req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.community_gen != my_gen {
                    return;
                }
                this.state.community_loading = false;
                if let Ok(Ok(mut feed)) = res {
                    this.state.overlay_in_flight_feed_items(&mut feed.items);
                    this.state.community_posts = feed.items;
                    this.state.community_next_cursor = feed.next_cursor;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn load_more_community_posts(&mut self, slug: String, cx: &mut Context<Self>) {
        if self.state.community_loading || self.state.community_loading_more {
            return;
        }
        let Some(cursor) = self.state.community_next_cursor.clone() else {
            return;
        };
        let my_gen = self.state.community_gen;
        self.state.community_loading_more = true;
        let req = FeedRequestDto {
            tab: FeedTab::Community(slug),
            sort: self.state.community_sort,
            time_window: self.state.community_time_window,
            actor_filter: self.state.community_actor_filter,
            flair: self.state.community_selected_flair.clone(),
            cursor: Some(cursor),
            limit: 25,
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.get_feed(&req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.community_gen != my_gen {
                    return;
                }
                this.state.community_loading_more = false;
                if let Ok(Ok(mut feed)) = res {
                    this.state.overlay_in_flight_feed_items(&mut feed.items);
                    let existing: HashSet<String> = this
                        .state
                        .community_posts
                        .iter()
                        .map(|i| i.post.post_id.clone())
                        .collect();
                    for item in feed.items {
                        if !existing.contains(&item.post.post_id) {
                            this.state.community_posts.push(item);
                        }
                    }
                    this.state.community_next_cursor = feed.next_cursor;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_community_flair_filter(
        &mut self,
        slug: String,
        flair: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.state.community_selected_flair = flair;
        self.reload_community_posts(slug, cx);
    }

    pub fn set_community_sort(&mut self, slug: String, sort: FeedSort, cx: &mut Context<Self>) {
        if self.state.community_sort != sort {
            self.state.community_sort = sort;
            self.reload_community_posts(slug, cx);
        }
    }

    pub fn set_community_time_window(
        &mut self,
        slug: String,
        tw: TimeWindow,
        cx: &mut Context<Self>,
    ) {
        if self.state.community_time_window != tw {
            self.state.community_time_window = tw;
            self.reload_community_posts(slug, cx);
        }
    }

    pub fn set_community_actor_filter(
        &mut self,
        slug: String,
        af: ActorFilter,
        cx: &mut Context<Self>,
    ) {
        if self.state.community_actor_filter != af {
            self.state.community_actor_filter = af;
            self.reload_community_posts(slug, cx);
        }
    }

    pub fn toggle_community_bell(&mut self, slug: String, cx: &mut Context<Self>) {
        let key = slug.to_lowercase();
        let current = self
            .state
            .community_bell_enabled
            .get(&key)
            .copied()
            .or_else(|| {
                self.state
                    .community_detail
                    .as_ref()
                    .filter(|d| d.community.slug.eq_ignore_ascii_case(&slug))
                    .map(|d| d.community.notifications_enabled)
            })
            .unwrap_or(true);
        let next = !current;
        self.state.community_bell_enabled.insert(key, next);
        if let Some(ref mut det) = self.state.community_detail {
            if det.community.slug.eq_ignore_ascii_case(&slug) {
                det.community.notifications_enabled = next;
            }
        }
        let lang = self.language_id.clone();
        self.emit_action(
            CommunityWorkspaceAction::ShowToast {
                text: if next {
                    format!("{} c/{slug}", tr_str(&lang, "已开启社区动态通知：", "Notifications enabled for"))
                } else {
                    format!("{} c/{slug}", tr_str(&lang, "已关闭社区动态通知：", "Notifications muted for"))
                },
                is_error: false,
            },
            cx,
        );
    }

    pub fn start_edit_community_wiki(&mut self, initial_markdown: Option<String>, cx: &mut Context<Self>) {
        let lang = self.language_id.clone();
        let can_mod = self
            .state
            .community_detail
            .as_ref()
            .is_some_and(|d| ModPermissionValidator::can_moderate(d.community.viewer_role));
        if !can_mod {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(&lang, "仅社区版主或 Bot 版主可编辑 Wiki", "Only community moderators can edit Wiki").to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        let md = initial_markdown.unwrap_or_else(|| {
            self.state
                .community_detail
                .as_ref()
                .map(|d| d.community.wiki_markdown.clone())
                .unwrap_or_default()
        });
        self.state.community_wiki_draft = md;
        self.state.community_wiki_summary.clear();
        self.state.community_wiki_editing = true;
        self.state.focus_input(ActiveCommunityInput::CommunityWikiMarkdown);
        cx.notify();
    }

    pub fn save_community_wiki(&mut self, slug: String, cx: &mut Context<Self>) {
        let lang = self.language_id.clone();
        let can_mod = self
            .state
            .community_detail
            .as_ref()
            .filter(|d| d.community.slug.eq_ignore_ascii_case(&slug))
            .is_some_and(|d| ModPermissionValidator::can_moderate(d.community.viewer_role));
        if !can_mod {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(&lang, "权限不足：需要版主角色才能保存 Wiki", "Permission denied: moderator role required to edit Wiki")
                        .to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        if self.state.community_wiki_saving {
            return;
        }
        let markdown = self.state.community_wiki_draft.clone();
        let summary = if self.state.community_wiki_summary.trim().is_empty() {
            "Updated community wiki".to_string()
        } else {
            self.state.community_wiki_summary.trim().to_string()
        };
        let req = UpdateCommunityWikiRequest { markdown, summary };
        self.state.community_wiki_save_seq = self.state.community_wiki_save_seq.wrapping_add(1);
        let my_save_seq = self.state.community_wiki_save_seq;
        self.state.community_wiki_saving = true;
        let my_actor_gen = self.state.actor_switch_gen;
        let my_comm_gen = self.state.community_gen;
        let submitter_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_slug = slug.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_wiki(&slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != submitter_actor_id
                {
                    return;
                }
                let still_current_save = this.state.community_wiki_save_seq == my_save_seq;
                if still_current_save {
                    this.state.community_wiki_saving = false;
                }
                let still_same_comm_gen = this.state.community_gen == my_comm_gen;
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(rev)) => {
                        let on_same_comm_route = matches!(
                            &this.state.route,
                            CommunityRoute::CommunityPage(s) if s.eq_ignore_ascii_case(&reload_slug)
                        );
                        if still_current_save {
                            if still_same_comm_gen || on_same_comm_route {
                                this.state.community_wiki_editing = false;
                                this.state.community_wiki_preview_rev = None;
                                if matches!(
                                    this.state.active_input,
                                    ActiveCommunityInput::CommunityWikiMarkdown
                                        | ActiveCommunityInput::CommunityWikiSummary
                                ) {
                                    this.state.blur_input();
                                }
                            }
                            this.state.patch_community_wiki(
                                &reload_slug,
                                &rev.markdown,
                                rev.revision,
                            );
                            if still_same_comm_gen {
                                if let Some(ref mut det) = this.state.community_detail {
                                    if det.community.slug.eq_ignore_ascii_case(&reload_slug)
                                        && !det
                                            .wiki_revisions
                                            .iter()
                                            .any(|r| r.revision == rev.revision)
                                    {
                                        det.wiki_revisions.insert(0, rev);
                                    }
                                }
                            }
                        }
                        if this.state.communities_loading {
                            this.reload_communities_list(cx);
                        }
                        if on_same_comm_route
                            && (this.state.community_loading
                                || (still_current_save && !still_same_comm_gen))
                        {
                            this.load_community_page(reload_slug.clone(), cx);
                        }
                        if this.state.mod_loading
                            && this
                                .state
                                .mod_slug
                                .as_deref()
                                .is_some_and(|s| s.eq_ignore_ascii_case(&reload_slug))
                        {
                            this.load_mod_queue(reload_slug, cx);
                        }
                        if this.state.search_loading
                            && matches!(this.state.route, CommunityRoute::Explore { .. })
                        {
                            let q = this.state.explore_search_draft.trim().to_string();
                            this.execute_search(q, this.state.search_tab, cx);
                        }
                        if still_current_save {
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: tr_str(
                                        &lang,
                                        "社区 Wiki 新版本已保存！",
                                        "Community Wiki revision saved!",
                                    )
                                    .to_string(),
                                    is_error: false,
                                },
                                cx,
                            );
                        }
                        cx.notify();
                    }
                    Ok(Err(e)) => {
                        if still_current_save {
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: e.to_string(),
                                    is_error: true,
                                },
                                cx,
                            );
                        }
                        cx.notify();
                    }
                    Err(_) => {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub fn add_community_rule(&mut self, slug: String, cx: &mut Context<Self>) {
        let lang = self.language_id.clone();
        let Some(det) = self.state.community_detail.as_ref() else {
            return;
        };
        if !ModPermissionValidator::can_moderate(det.community.viewer_role) {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(&lang, "仅版主可修改社区规则", "Only moderators can edit rules").to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        let title = self.state.community_new_rule_title.trim().to_string();
        if title.is_empty() {
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(&lang, "请输入规则标题", "Rule title cannot be empty").to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        let desc = self.state.community_new_rule_desc.trim().to_string();
        let mut rules = det.community.rules.clone();
        let next_order = (rules.len() as u32) + 1;
        rules.push(CommunityRuleDto {
            id: format!("rule_{next_order}"),
            title,
            description: desc,
            order: next_order,
        });
        self.save_community_rules(slug, rules, cx);
    }

    pub fn remove_community_rule(&mut self, slug: String, rule_idx: usize, cx: &mut Context<Self>) {
        let Some(det) = self.state.community_detail.as_ref() else {
            return;
        };
        if !ModPermissionValidator::can_moderate(det.community.viewer_role) {
            return;
        }
        let mut rules = det.community.rules.clone();
        if rule_idx < rules.len() {
            rules.remove(rule_idx);
            for (i, r) in rules.iter_mut().enumerate() {
                r.order = (i as u32) + 1;
            }
            self.save_community_rules(slug, rules, cx);
        }
    }

    fn save_community_rules(
        &mut self,
        slug: String,
        rules: Vec<CommunityRuleDto>,
        cx: &mut Context<Self>,
    ) {
        if self.state.community_rules_saving {
            return;
        }
        self.state.community_rules_save_seq = self.state.community_rules_save_seq.wrapping_add(1);
        let my_save_seq = self.state.community_rules_save_seq;
        self.state.community_rules_saving = true;
        let my_actor_gen = self.state.actor_switch_gen;
        let my_comm_gen = self.state.community_gen;
        let submitter_actor_id = self.state.active_actor_id.clone();
        let req = UpdateCommunityRequest {
            rules: Some(rules),
            ..Default::default()
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_slug = slug.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_community(&slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != submitter_actor_id
                {
                    return;
                }
                let still_current_save = this.state.community_rules_save_seq == my_save_seq;
                if still_current_save {
                    this.state.community_rules_saving = false;
                }
                let still_same_comm_gen = this.state.community_gen == my_comm_gen;
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(updated)) => {
                        if still_current_save {
                            this.state.community_new_rule_title.clear();
                            this.state.community_new_rule_desc.clear();
                            this.state
                                .patch_community_rules(&updated.slug, &updated.rules);
                        }
                        if this.state.communities_loading {
                            this.reload_communities_list(cx);
                        }
                        let on_same_comm_route = matches!(
                            &this.state.route,
                            CommunityRoute::CommunityPage(s) if s.eq_ignore_ascii_case(&reload_slug)
                        );
                        if on_same_comm_route
                            && (this.state.community_loading
                                || (still_current_save && !still_same_comm_gen))
                        {
                            this.load_community_page(reload_slug.clone(), cx);
                        }
                        if this.state.mod_loading
                            && this
                                .state
                                .mod_slug
                                .as_deref()
                                .is_some_and(|s| s.eq_ignore_ascii_case(&reload_slug))
                        {
                            this.load_mod_queue(reload_slug, cx);
                        }
                        if this.state.search_loading
                            && matches!(this.state.route, CommunityRoute::Explore { .. })
                        {
                            let q = this.state.explore_search_draft.trim().to_string();
                            this.execute_search(q, this.state.search_tab, cx);
                        }
                        if still_current_save {
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: tr_str(&lang, "社区规则已更新", "Community rules updated")
                                        .to_string(),
                                    is_error: false,
                                },
                                cx,
                            );
                        }
                        cx.notify();
                    }
                    Ok(Err(e)) => {
                        if still_current_save {
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast {
                                    text: e.to_string(),
                                    is_error: true,
                                },
                                cx,
                            );
                        }
                        cx.notify();
                    }
                    Err(_) => {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub fn open_create_community_modal(&mut self, cx: &mut Context<Self>) {
        if self.state.create_community.submitting {
            return;
        }
        self.state.create_community = CreateCommunityDraft::default();
        self.state.active_popover = None;
        self.state.active_modal = Some(CommunityModal::CreateCommunity);
        self.state.focus_input(ActiveCommunityInput::CreateCommSlug);
        cx.notify();
    }

    pub fn close_create_community_modal(&mut self, cx: &mut Context<Self>) {
        if self.state.active_modal == Some(CommunityModal::CreateCommunity) {
            self.state.active_modal = None;
        }
        if self.state.active_input.is_create_comm_input() {
            self.state.blur_input();
        }
        cx.notify();
    }

    pub fn submit_create_community(&mut self, cx: &mut Context<Self>) {
        if self.state.create_community.submitting {
            return;
        }
        let lang = self.language_id.clone();
        let raw_slug_trimmed = self.state.create_community.slug.trim();
        let raw_slug_body = raw_slug_trimmed
            .strip_prefix("c/")
            .unwrap_or(raw_slug_trimmed);
        if raw_slug_trimmed.starts_with("C/")
            || raw_slug_body.bytes().any(|b| b.is_ascii_uppercase())
        {
            self.state.create_community.error = Some(
                tr_str(
                    &lang,
                    "社区标识不能包含大写字母（仅限 2–32 位小写字母、数字或下划线 [a-z0-9_]）",
                    "Invalid slug: uppercase letters are not allowed (2–32 lowercase ASCII [a-z0-9_])",
                )
                .to_string(),
            );
            cx.notify();
            return;
        }
        let slug = match normalize_community_slug(raw_slug_trimmed) {
            Ok(s) => s,
            Err(e) => {
                self.state.create_community.error = Some(format!(
                    "{} ({e})",
                    tr_str(&lang, "社区标识无效（需 2–32 位小写字母、数字或下划线）", "Invalid slug (2–32 ASCII [a-z0-9_])")
                ));
                cx.notify();
                return;
            }
        };
        let name = self.state.create_community.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 80 {
            self.state.create_community.error = Some(
                tr_str(&lang, "社区名称需为 1–80 个字符", "Community title must be 1–80 characters").to_string(),
            );
            cx.notify();
            return;
        }
        let draft = self.state.create_community.clone();
        let req = CreateCommunityRequest {
            slug,
            name,
            description: draft.description.trim().to_string(),
            category: draft.category,
            banner_media_id: None,
            icon_media_id: None,
            icon_emoji: Some(draft.icon_emoji.trim().to_string()).filter(|s| !s.is_empty()),
            accent_color: Some(draft.accent_color),
            rules: Vec::new(),
            post_flairs: Vec::new(),
            user_flairs: Vec::new(),
            wiki_markdown: String::new(),
            automod_rules: Vec::new(),
            allow_bots: draft.allow_bots,
            require_bot_disclosure: draft.require_bot_disclosure,
            is_nsfw: draft.is_nsfw,
        };
        self.state.create_community.submitting = true;
        self.state.create_community.error = None;
        self.state.communities_gen = self.state.communities_gen.wrapping_add(1);
        self.state.communities_loading = true;
        let my_comm_gen = self.state.communities_gen;
        let my_actor_gen = self.state.actor_switch_gen;
        let submitter_actor_id = self.state.active_actor_id.clone();
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let created = facade.create_community(&req).await?;
                let list = facade.list_communities(None, Some(50)).await.ok();
                Ok::<_, gpui_telegram_core::TelegramError>((created, list))
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.actor_switch_gen != my_actor_gen
                    || this.state.active_actor_id != submitter_actor_id
                {
                    return;
                }
                this.state.create_community.submitting = false;
                let same_comm_gen = this.state.communities_gen == my_comm_gen;
                if same_comm_gen {
                    this.state.communities_loading = false;
                }
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok((created, list))) => {
                        if same_comm_gen {
                            if let Some(mut l) = list {
                                for comm in &mut l.communities {
                                    this.state.overlay_in_flight_community_mutations(comm);
                                }
                                this.state.communities = l.communities;
                            }
                        }
                        if !this
                            .state
                            .communities
                            .iter()
                            .any(|c| c.slug.eq_ignore_ascii_case(&created.slug))
                        {
                            this.state.communities.insert(0, created.clone());
                        }
                        if this.state.communities_loading {
                            this.reload_communities_list(cx);
                        }
                        this.close_create_community_modal(cx);
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast {
                                text: format!(
                                    "{} c/{}!",
                                    tr_str(&lang, "已创建社区", "Created community"),
                                    created.slug
                                ),
                                is_error: false,
                            },
                            cx,
                        );
                        this.navigate_to(CommunityRoute::CommunityPage(created.slug), cx);
                    }
                    Ok(Err(e)) => {
                        this.state.create_community.error = Some(e.to_string());
                        cx.notify();
                    }
                    Err(e) => {
                        this.state.create_community.error = Some(e);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// Renders the full F19 `c/<slug>` Community Page (`Posts | Rules | Wiki | Mods`).
pub fn render_community_page_view(
    ws: &CommunityWorkspace,
    slug: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let detail = ws.state.community_detail.clone();
    let active_tab = ws.state.community_tab;

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(
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
                        .max_w(px(760.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .when(ws.state.community_loading && detail.is_none(), |col| {
                            col.child(render_loading_skeleton(2, &pal))
                        })
                        .when(!ws.state.community_loading && detail.is_none(), |col| {
                            let sub = ws.state.community_error.as_deref().unwrap_or_else(|| {
                                tr_str(
                                    lang,
                                    "未找到对应的 c/ 社区，请检查社区名称或新建社区。",
                                    "Could not find this c/ community. Check the slug or create a new one.",
                                )
                            });
                            col.child(render_empty_state(
                                IconName::CommunityGlobe,
                                tr_str(lang, "未找到该社区", "Community not found"),
                                sub,
                                &pal,
                            ))
                        })
                        .when_some(detail, |col, det| {
                            let can_mod = ModPermissionValidator::can_moderate(det.community.viewer_role);
                            col.child(render_community_banner_card(ws, &det, can_mod, cx))
                                .child(render_community_subtabs(ws, &det, cx))
                                .when(active_tab == CommunityPageTab::Posts, |c| {
                                    c.child(render_community_posts_tab(ws, &det, cx))
                                })
                                .when(active_tab == CommunityPageTab::Rules, |c| {
                                    c.child(render_community_rules_tab(ws, &det, can_mod, cx))
                                })
                                .when(active_tab == CommunityPageTab::Wiki, |c| {
                                    c.child(render_community_wiki_tab(ws, &det, can_mod, cx))
                                })
                                .when(active_tab == CommunityPageTab::Mods, |c| {
                                    c.child(render_community_mods_tab(ws, &det, can_mod, cx))
                                })
                        }),
                ),
        )
        .into_any_element()
}

fn render_community_banner_card(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::CommunityDetailResponse,
    can_mod: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &det.community;
    let accent_rgb = parse_fabushi_hex_color(&c.accent_color).unwrap_or(pal.accent);
    let joined = c.is_joined;
    let bell_on = ws
        .state
        .community_bell_enabled
        .get(&c.slug.to_lowercase())
        .copied()
        .unwrap_or(c.notifications_enabled);
    let active_is_bot = ws
        .state
        .active_actor()
        .is_some_and(|a| a.actor_kind.is_bot_like());
    let bot_blocked = !c.allow_bots && active_is_bot;
    let join_slug = c.slug.clone();
    let bell_slug = c.slug.clone();
    let post_slug = c.slug.clone();
    let mod_slug = c.slug.clone();
    let hover_bg = pal.row_hover_rgba();

    div()
        .w_full()
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .overflow_hidden()
        .flex()
        .flex_col()
        // Accent Banner Header Strip
        .child(
            div()
                .w_full()
                .h(px(64.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(accent_rgb, 0.28))
                .border_b_1()
                .border_color(TdesktopThemePalette::rgba_with_alpha(accent_rgb, 0.40))
                .px(px(14.0))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .w(px(38.0))
                                .h(px(38.0))
                                .rounded(px(10.0))
                                .bg(rgb(pal.card_bg))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(20.0))
                                .child(if c.icon_emoji.trim().is_empty() {
                                    "🌐".to_string()
                                } else {
                                    c.icon_emoji.clone()
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(15.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(c.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("c/{} · {}", c.slug, c.category)),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("comm-header-create-btn")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.5))
                        .rounded_full()
                        .bg(pal.card_surface_rgba())
                        .hover(move |s| s.bg(hover_bg))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_create_community_modal(cx);
                            }),
                        )
                        .child(tr_str(lang, "+ 新建社区", "+ Create c/")),
                ),
        )
        // Body: Description, Stats, Policy Badges, Action Buttons
        .child(
            div()
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(9.0))
                .when(!c.description.trim().is_empty(), |col| {
                    col.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(pal.text_primary))
                            .child(c.description.clone()),
                    )
                })
                // Stats & Policy Badges
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(6.0))
                        .child(render_flair_chip(
                            &format!(
                                "{} {}",
                                format_compact_count(c.human_member_count as i64),
                                tr_str(lang, "人类成员", "Humans")
                            ),
                            Some("#3B82F6"),
                            &pal,
                        ))
                        .child(render_flair_chip(
                            &format!(
                                "{} {}",
                                format_compact_count(c.bot_member_count as i64),
                                tr_str(lang, "Bot 公民", "Bots")
                            ),
                            Some("#8B5CF6"),
                            &pal,
                        ))
                        .child(render_flair_chip(
                            &format!(
                                "{} {}",
                                format_compact_count(c.posts_count as i64),
                                tr_str(lang, "帖子", "Posts")
                            ),
                            None,
                            &pal,
                        ))
                        .child(render_flair_chip(
                            if c.allow_bots {
                                tr_str(lang, "🤖 欢迎 Bot", "🤖 Bots Allowed")
                            } else {
                                tr_str(lang, "🚫 仅限人类 (禁 Bot)", "🚫 Humans Only (No Bots)")
                            },
                            if c.allow_bots { Some("#10B981") } else { Some("#EF4444") },
                            &pal,
                        ))
                        .when(c.require_bot_disclosure, |row| {
                            row.child(render_flair_chip(
                                tr_str(lang, "要求 Bot 透明标识", "Bot Disclosure Required"),
                                Some("#F59E0B"),
                                &pal,
                            ))
                        })
                        .when(c.is_nsfw, |row| {
                            row.child(render_flair_chip("18+ NSFW", Some("#EF4444"), &pal))
                        }),
                )
                // Actions: Join/Leave, Bell, New Post in c/<slug>, Mod Console
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(7.0))
                        .child(
                            div()
                                .id("comm-page-join-btn")
                                .cursor_pointer()
                                .px(px(14.0))
                                .py(px(6.0))
                                .rounded_full()
                                .bg(if joined {
                                    pal.service_pill_rgba()
                                } else {
                                    rgb(pal.accent).into()
                                })
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if joined {
                                    pal.text_primary
                                } else {
                                    pal.active_button_fg
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_join_community(join_slug.clone(), !joined, cx);
                                    }),
                                )
                                .child(if joined {
                                    tr_str(lang, "已加入", "Joined")
                                } else {
                                    tr_str(lang, "加入社区", "Join")
                                }),
                        )
                        .child(
                            div()
                                .id("comm-page-bell-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded_full()
                                .bg(if bell_on {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(if bell_on { pal.accent } else { pal.text_secondary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_community_bell(bell_slug.clone(), cx);
                                    }),
                                )
                                .child(if bell_on {
                                    tr_str(lang, "🔔 通知已开", "🔔 Bell On")
                                } else {
                                    tr_str(lang, "🔕 通知已关", "🔕 Bell Off")
                                }),
                        )
                        .child(
                            div()
                                .id("comm-page-new-post-btn")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(6.0))
                                .rounded_full()
                                .bg(if bot_blocked {
                                    pal.service_pill_rgba()
                                } else {
                                    rgb(pal.accent).into()
                                })
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if bot_blocked {
                                    pal.text_secondary
                                } else {
                                    pal.active_button_fg
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if bot_blocked {
                                            let lang = this.language_id.clone();
                                            this.emit_action(
                                                CommunityWorkspaceAction::ShowToast {
                                                    text: tr_str(
                                                        &lang,
                                                        "该社区禁止 Bot 发帖，请先切换为人类身份。",
                                                        "Bots cannot post in this community (allow_bots = false). Switch to Human first.",
                                                    )
                                                    .to_string(),
                                                    is_error: true,
                                                },
                                                cx,
                                            );
                                            return;
                                        }
                                        this.open_composer_modal(Some(post_slug.clone()), cx);
                                    }),
                                )
                                .child(tg_icon(
                                    IconName::ComposePen,
                                    12.5,
                                    if bot_blocked { pal.text_secondary } else { pal.active_button_fg },
                                ))
                                .child(format!("{} c/{}", tr_str(lang, "发帖到", "Post in"), c.slug)),
                        )
                        .when(can_mod, |row| {
                            row.child(
                                div()
                                    .id("comm-page-mod-console-btn")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded_full()
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.16))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.success))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.navigate_to(
                                                CommunityRoute::ModQueue(Some(mod_slug.clone())),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(tg_icon(IconName::ShieldMod, 12.5, pal.success))
                                    .child(tr_str(lang, "版务台", "Mod Console")),
                            )
                        }),
                )
                .when(bot_blocked, |col| {
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
                                "当前以 Bot 身份浏览，而该社区已开启「仅限人类发帖 (allow_bots = false)」。请通过左下角身份切换器切回人类身份再参与发布。",
                                "You are acting as a Bot, and this community has allow_bots = false. Switch to your Human identity to post here.",
                            )),
                    )
                }),
        )
}

fn render_community_subtabs(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::CommunityDetailResponse,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active = ws.state.community_tab;
    let tabs: [(CommunityPageTab, String); 4] = [
        (CommunityPageTab::Posts, tr_str(lang, "帖子", "Posts").to_string()),
        (CommunityPageTab::Rules, format!("{} ({})", tr_str(lang, "规则", "Rules"), det.community.rules.len())),
        (CommunityPageTab::Wiki, format!("Wiki v{}", det.community.wiki_revision)),
        (CommunityPageTab::Mods, format!("{} ({})", tr_str(lang, "版主团队", "Mods"), det.moderators.len())),
    ];

    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .pb(px(4.0))
        .border_b_1()
        .border_color(pal.separator_rgba())
        .children(tabs.into_iter().map(|(t, lbl)| {
            let sel = active == t;
            div()
                .id(SharedString::from(format!("comm-tab-{t:?}")))
                .cursor_pointer()
                .px(px(12.0))
                .py(px(6.0))
                .rounded_full()
                .bg(if sel { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if sel { pal.active_button_fg } else { pal.text_primary }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_community_tab(t, cx);
                    }),
                )
                .child(lbl)
        }))
}

fn render_community_posts_tab(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::CommunityDetailResponse,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let slug = det.community.slug.clone();
    let sel_flair = ws.state.community_selected_flair.clone();
    let active_sort = ws.state.community_sort;
    let active_tw = ws.state.community_time_window;
    let active_af = ws.state.community_actor_filter;
    let hover_bg = pal.row_hover_rgba();

    let sorts: [(FeedSort, &'static str, &'static str); 6] = [
        (FeedSort::Hot, "热门", "Hot"),
        (FeedSort::Best, "精选", "Best"),
        (FeedSort::Top, "高赞", "Top"),
        (FeedSort::New, "最新", "New"),
        (FeedSort::Controversial, "热议", "Controversial"),
        (FeedSort::Rising, "上升", "Rising"),
    ];
    let windows: [(TimeWindow, &'static str, &'static str); 5] = [
        (TimeWindow::Hour, "1小时", "1h"),
        (TimeWindow::Day, "今天", "Day"),
        (TimeWindow::Week, "本周", "Week"),
        (TimeWindow::Month, "本月", "Month"),
        (TimeWindow::All, "全部", "All"),
    ];
    let filters: [(ActorFilter, &'static str, &'static str); 4] = [
        (ActorFilter::AllCitizens, "全部", "All"),
        (ActorFilter::HumansOnly, "人类", "Humans"),
        (ActorFilter::BotsOnly, "Bot", "Bots"),
        (ActorFilter::HumanBotDebates, "人机辩论", "Debates"),
    ];

    // Deduplicate pinned posts from the main post stream so pinned cards never render twice.
    let show_pinned = sel_flair.is_none() && !det.pinned_posts.is_empty();
    let pinned_ids: HashSet<&str> = if show_pinned {
        det.pinned_posts.iter().map(|p| p.post_id.as_str()).collect()
    } else {
        HashSet::new()
    };
    let stream_posts: Vec<FeedItemDto> = ws
        .state
        .community_posts
        .iter()
        .filter(|it| !pinned_ids.contains(it.post.post_id.as_str()))
        .cloned()
        .collect();
    let clear_slug = slug.clone();
    let more_slug = slug.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        // Sort + TimeWindow + ActorFilter + Flair Filter Card
        .child(
            div()
                .p(px(10.0))
                .rounded(px(10.0))
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
                        .flex_wrap()
                        .gap(px(5.0))
                        .children(sorts.into_iter().map(|(s, zh, en)| {
                            let active = active_sort == s;
                            let sl = slug.clone();
                            div()
                                .id(SharedString::from(format!("comm-sort-{s:?}")))
                                .cursor_pointer()
                                .px(px(8.5))
                                .py(px(3.5))
                                .rounded_full()
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .hover(move |st| st.bg(hover_bg))
                                .text_size(px(11.0))
                                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                .text_color(rgb(if active { pal.accent } else { pal.text_primary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_community_sort(sl.clone(), s, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        }))
                        .when(matches!(active_sort, FeedSort::Top | FeedSort::Controversial), |row| {
                            row.child(div().w(px(1.0)).h(px(12.0)).bg(pal.separator_rgba()))
                                .children(windows.into_iter().map(|(tw, zh, en)| {
                                    let active = active_tw == tw;
                                    let sl = slug.clone();
                                    div()
                                        .id(SharedString::from(format!("comm-tw-{tw:?}")))
                                        .cursor_pointer()
                                        .px(px(7.0))
                                        .py(px(3.0))
                                        .rounded_full()
                                        .bg(if active { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                                        .text_size(px(10.5))
                                        .text_color(rgb(if active { pal.active_button_fg } else { pal.text_secondary }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.set_community_time_window(sl.clone(), tw, cx);
                                            }),
                                        )
                                        .child(tr_str(lang, zh, en))
                                }))
                        })
                        .child(div().w(px(1.0)).h(px(12.0)).bg(pal.separator_rgba()))
                        .children(filters.into_iter().map(|(af, zh, en)| {
                            let active = active_af == af;
                            let sl = slug.clone();
                            div()
                                .id(SharedString::from(format!("comm-af-{af:?}")))
                                .cursor_pointer()
                                .px(px(7.5))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(10.5))
                                .text_color(rgb(if active { pal.accent } else { pal.text_secondary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_community_actor_filter(sl.clone(), af, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        })),
                )
                .when(!det.community.post_flairs.is_empty(), |col| {
                    let all_slug = slug.clone();
                    col.child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .id("comm-flair-all")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded_full()
                                    .bg(if sel_flair.is_none() {
                                        rgb(pal.accent).into()
                                    } else {
                                        pal.service_pill_rgba()
                                    })
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(if sel_flair.is_none() {
                                        pal.active_button_fg
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.set_community_flair_filter(all_slug.clone(), None, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "全部标签", "All Flairs")),
                            )
                            .children(det.community.post_flairs.iter().map(|f| {
                                let active = sel_flair
                                    .as_deref()
                                    .is_some_and(|sf| sf.eq_ignore_ascii_case(&f.text));
                                let sl = slug.clone();
                                let ft = f.text.clone();
                                div()
                                    .id(SharedString::from(format!("comm-flair-chip-{}", f.id)))
                                    .cursor_pointer()
                                    .px(px(8.5))
                                    .py(px(3.0))
                                    .rounded_full()
                                    .bg(if active {
                                        rgb(pal.accent).into()
                                    } else {
                                        pal.service_pill_rgba()
                                    })
                                    .text_size(px(10.5))
                                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                    .text_color(rgb(if active {
                                        pal.active_button_fg
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.set_community_flair_filter(sl.clone(), Some(ft.clone()), cx);
                                        }),
                                    )
                                    .child(f.text.clone())
                            })),
                    )
                }),
        )
        // Pinned Posts Section
        .when(show_pinned, |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_size(px(11.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.success))
                            .child(tg_icon(IconName::Pin, 12.5, pal.success))
                            .child(tr_str(lang, "置顶公告与精选帖", "Pinned Community Posts")),
                    )
                    .children(det.pinned_posts.iter().enumerate().map(|(i, p)| {
                        let item = FeedItemDto {
                            post: p.clone(),
                            explanation: None,
                            rank_score: 0.0,
                            is_exploration_slot: false,
                        };
                        render_post_card(ws, &item, 900 + i, cx)
                    })),
            )
        })
        // Main Post Stream or Clean Empty / Clear-Filter State
        .when(stream_posts.is_empty() && !show_pinned && !ws.state.community_loading, |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .child(render_empty_state(
                        IconName::CommunityGlobe,
                        if sel_flair.is_some() {
                            tr_str(lang, "该标签下暂无帖子", "No posts match this flair")
                        } else {
                            tr_str(lang, "该社区暂无帖子", "No posts in this community yet")
                        },
                        tr_str(lang, "尝试切换筛选条件或成为第一位发帖者！", "Try clearing filters or publish the first post!"),
                        &pal,
                    ))
                    .when(sel_flair.is_some(), |c| {
                        c.child(
                            div()
                                .id("comm-clear-flair-btn")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(6.0))
                                .rounded_full()
                                .bg(rgb(pal.accent))
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_community_flair_filter(clear_slug.clone(), None, cx);
                                    }),
                                )
                                .child(tr_str(lang, "清除标签筛选", "Clear filter")),
                        )
                    }),
            )
        })
        .children(stream_posts.iter().enumerate().map(|(i, it)| render_post_card(ws, it, i, cx)))
        .when(ws.state.community_next_cursor.is_some(), |col| {
            col.child(
                div()
                    .id("comm-load-more-posts")
                    .cursor_pointer()
                    .w_full()
                    .py(px(9.0))
                    .rounded(px(9.0))
                    .bg(pal.card_surface_rgba())
                    .border_1()
                    .border_color(pal.hairline_border_rgba())
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.load_more_community_posts(more_slug.clone(), cx);
                        }),
                    )
                    .child(if ws.state.community_loading_more {
                        tr_str(lang, "正在加载更多…", "Loading more…")
                    } else {
                        tr_str(lang, "加载更多社区帖子", "Load More Posts")
                    }),
            )
        })
}

fn render_community_rules_tab(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::CommunityDetailResponse,
    can_mod: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &det.community;
    let slug = c.slug.clone();
    let add_slug = slug.clone();
    let title_focused = ws.state.active_input == ActiveCommunityInput::CommunityNewRuleTitle;
    let desc_focused = ws.state.active_input == ActiveCommunityInput::CommunityNewRuleDesc;

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        // Bot & Governance Policy Summary
        .child(
            div()
                .p(px(12.0))
                .rounded(px(10.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "社区治理与 Bot 公民政策", "Community Governance & Bot Policy")),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "• {} : {}\n• {} : {}",
                            tr_str(lang, "允许 Bot 参与发帖与评论", "Bots allowed to participate"),
                            if c.allow_bots { tr_str(lang, "允许 (Yes)", "Yes") } else { tr_str(lang, "禁止 (No)", "No") },
                            tr_str(lang, "要求 Bot 明确身份标识", "Require Bot identity disclosure"),
                            if c.require_bot_disclosure { tr_str(lang, "是 (Required)", "Required") } else { tr_str(lang, "否 (Optional)", "Optional") },
                        )),
                ),
        )
        // Numbered Rules List
        .when(c.rules.is_empty(), |col| {
            col.child(render_empty_state(
                IconName::FileDoc,
                tr_str(lang, "暂无社区规则", "No rules defined yet"),
                tr_str(lang, "版主可以在下方添加社区准则。", "Moderators can add community guidelines below."),
                &pal,
            ))
        })
        .children(c.rules.iter().enumerate().map(|(i, rule)| {
            let rm_slug = slug.clone();
            div()
                .p(px(11.0))
                .rounded(px(10.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .items_start()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!("{}. {}", i + 1, rule.title)),
                        )
                        .when(!rule.description.trim().is_empty(), |col| {
                            col.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(rule.description.clone()),
                            )
                        }),
                )
                .when(can_mod, |row| {
                    row.child(
                        div()
                            .id(SharedString::from(format!("comm-rule-rm-{i}")))
                            .cursor_pointer()
                            .px(px(7.0))
                            .py(px(3.5))
                            .rounded(px(6.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                            .text_size(px(11.0))
                            .text_color(rgb(pal.danger))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove_community_rule(rm_slug.clone(), i, cx);
                                }),
                            )
                            .child(tr_str(lang, "删除", "Remove")),
                    )
                })
        }))
        // Mod-Only Add Rule Form
        .when(can_mod, |col| {
            col.child(
                div()
                    .p(px(12.0))
                    .rounded(px(10.0))
                    .bg(pal.card_surface_rgba())
                    .border_1()
                    .border_color(pal.hairline_border_rgba())
                    .flex()
                    .flex_col()
                    .gap(px(7.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.accent))
                            .child(tr_str(lang, "添加社区规则（仅版主可见）", "Add Community Rule (Moderator)")),
                    )
                    .child(
                        div()
                            .id("comm-new-rule-title")
                            .cursor_text()
                            .w_full()
                            .px(px(9.0))
                            .py(px(6.5))
                            .rounded(px(7.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(if title_focused { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.focus_input(ActiveCommunityInput::CommunityNewRuleTitle);
                                    cx.notify();
                                }),
                            )
                            .child(render_input_display(
                                &ws.state.community_new_rule_title,
                                if title_focused { ws.state.ime_preedit.as_deref() } else { None },
                                tr_str(lang, "规则标题…", "Rule title…"),
                                title_focused,
                                &pal,
                            )),
                    )
                    .child(
                        div()
                            .id("comm-new-rule-desc")
                            .cursor_text()
                            .w_full()
                            .min_h(px(44.0))
                            .px(px(9.0))
                            .py(px(6.5))
                            .rounded(px(7.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(if desc_focused { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.focus_input(ActiveCommunityInput::CommunityNewRuleDesc);
                                    cx.notify();
                                }),
                            )
                            .child(render_input_display(
                                &ws.state.community_new_rule_desc,
                                if desc_focused { ws.state.ime_preedit.as_deref() } else { None },
                                tr_str(lang, "规则详细说明（可选）…", "Rule description (optional)…"),
                                desc_focused,
                                &pal,
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_end()
                            .child(
                                div()
                                    .id("comm-add-rule-save")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(5.5))
                                    .rounded_full()
                                    .bg(rgb(pal.accent))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.add_community_rule(add_slug.clone(), cx);
                                        }),
                                    )
                                    .child(if ws.state.community_rules_saving {
                                        tr_str(lang, "保存中…", "Saving…")
                                    } else {
                                        tr_str(lang, "添加并保存规则", "Add Rule")
                                    }),
                            ),
                    ),
            )
        })
}

fn render_community_wiki_tab(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::CommunityDetailResponse,
    can_mod: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let c = &det.community;
    let slug = c.slug.clone();
    let save_slug = slug.clone();
    let editing = ws.state.community_wiki_editing && can_mod;
    let preview_rev = ws.state.community_wiki_preview_rev;
    let active_rev_dto = preview_rev.and_then(|rn| det.wiki_revisions.iter().find(|r| r.revision == rn));
    let display_md = active_rev_dto
        .map(|r| r.markdown.as_str())
        .unwrap_or(c.wiki_markdown.as_str());
    let display_ver = active_rev_dto.map(|r| r.revision).unwrap_or(c.wiki_revision);
    let linked_md = linkify_community_slugs(display_md);
    let hover_bg = pal.row_hover_rgba();

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        // Wiki Header & Controls
        .child(
            div()
                .p(px(12.0))
                .rounded(px(10.0))
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
                        .flex_wrap()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(format!("c/{} Wiki · v{display_ver}", c.slug)),
                                )
                                .when(preview_rev.is_some(), |r| {
                                    r.child(render_flair_chip(
                                        tr_str(lang, "历史版本预览", "Historical Revision"),
                                        Some("#F59E0B"),
                                        &pal,
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .when(preview_rev.is_some(), |row| {
                                    row.child(
                                        div()
                                            .id("wiki-back-latest-btn")
                                            .cursor_pointer()
                                            .px(px(9.0))
                                            .py(px(4.5))
                                            .rounded_full()
                                            .bg(pal.service_pill_rgba())
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.state.community_wiki_preview_rev = None;
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(lang, "返回最新版", "Latest")),
                                    )
                                })
                                .when(can_mod && !editing, |row| {
                                    let restore_md = active_rev_dto.map(|r| r.markdown.clone());
                                    row.child(
                                        div()
                                            .id("comm-wiki-edit-btn")
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(4.5))
                                            .rounded_full()
                                            .bg(rgb(pal.accent))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.active_button_fg))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.start_edit_community_wiki(restore_md.clone(), cx);
                                                }),
                                            )
                                            .child(if preview_rev.is_some() {
                                                tr_str(lang, "以此版本编辑", "Edit / Restore")
                                            } else {
                                                tr_str(lang, "编辑 Wiki", "Edit Wiki")
                                            }),
                                    )
                                }),
                        ),
                )
                .when(!editing, |col| {
                    col.child(if linked_md.trim().is_empty() {
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(lang, "该社区尚未编写 Wiki 文档。", "No Wiki content yet."))
                            .into_any_element()
                    } else {
                        render_rich_message_body(
                            gpui_telegram_community::fnv1a_64(c.community_id.as_bytes()).wrapping_add(777),
                            &linked_md,
                            &[],
                            &ws.state.revealed_spoilers,
                            &ws.state.expanded_quotes,
                            ws.state.copied_code_msg,
                            &pal,
                            lang,
                            false,
                            cx,
                            |this, action, cx| this.handle_rich_text_action(action, cx),
                        )
                    })
                })
                .when(editing, |col| {
                    let md_focused = ws.state.active_input == ActiveCommunityInput::CommunityWikiMarkdown;
                    let sum_focused = ws.state.active_input == ActiveCommunityInput::CommunityWikiSummary;
                    col.child(
                        div()
                            .id("comm-wiki-md-input")
                            .cursor_text()
                            .w_full()
                            .min_h(px(140.0))
                            .p(px(9.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(if md_focused { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.focus_input(ActiveCommunityInput::CommunityWikiMarkdown);
                                    cx.notify();
                                }),
                            )
                            .child(render_input_display(
                                &ws.state.community_wiki_draft,
                                if md_focused { ws.state.ime_preedit.as_deref() } else { None },
                                tr_str(lang, "使用 Markdown 编写社区 Wiki…", "Write community Wiki in Markdown…"),
                                md_focused,
                                &pal,
                            )),
                    )
                    .child(
                        div()
                            .id("comm-wiki-summary-input")
                            .cursor_text()
                            .w_full()
                            .px(px(9.0))
                            .py(px(6.5))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(if sum_focused { rgb(pal.accent).into() } else { pal.hairline_border_rgba() })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.focus_input(ActiveCommunityInput::CommunityWikiSummary);
                                    cx.notify();
                                }),
                            )
                            .child(render_input_display(
                                &ws.state.community_wiki_summary,
                                if sum_focused { ws.state.ime_preedit.as_deref() } else { None },
                                tr_str(lang, "版本变更摘要（如：补充入门 FAQ）…", "Revision summary (e.g. added starter FAQ)…"),
                                sum_focused,
                                &pal,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                macos_secondary_pill_btn("comm-wiki-cancel", tr_str(lang, "取消", "Cancel"), pal)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.state.community_wiki_editing = false;
                                            this.state.blur_input();
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .child(
                                macos_primary_pill_btn(
                                    "comm-wiki-save",
                                    if ws.state.community_wiki_saving {
                                        tr_str(lang, "保存中…", "Saving…")
                                    } else {
                                        tr_str(lang, "发布新版本", "Save Revision")
                                    },
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.save_community_wiki(save_slug.clone(), cx);
                                    }),
                                ),
                            ),
                    )
                }),
        )
        // Revision History Card
        .child(
            div()
                .p(px(12.0))
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
                        .child(format!(
                            "{} ({})",
                            tr_str(lang, "Wiki 历史版本记录", "Revision History"),
                            det.wiki_revisions.len()
                        )),
                )
                .children(det.wiki_revisions.iter().map(|rev| {
                    let rev_num = rev.revision;
                    let is_cur = preview_rev == Some(rev_num)
                        || (preview_rev.is_none() && rev_num == c.wiki_revision);
                    div()
                        .id(SharedString::from(format!("wiki-rev-row-{rev_num}")))
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(6.5))
                        .rounded(px(7.0))
                        .bg(if is_cur {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14)
                        } else {
                            rgb(pal.dialogs_bg).into()
                        })
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.community_wiki_preview_rev = Some(rev_num);
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(format!("v{rev_num} · {}", rev.summary)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "@{} · {}",
                                            rev.editor_handle,
                                            format_relative_time(rev.created_at_ms, lang)
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.accent))
                                .child(tr_str(lang, "查看", "Inspect")),
                        )
                })),
        )
}

fn render_community_mods_tab(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::CommunityDetailResponse,
    can_mod: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let slug = det.community.slug.clone();
    let hover_bg = pal.row_hover_rgba();

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when(can_mod, |col| {
            col.child(
                div()
                    .p(px(10.0))
                    .rounded(px(9.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.success))
                            .child(tr_str(lang, "您拥有本社区版务管理权限", "You have moderator privileges here")),
                    )
                    .child(
                        div()
                            .id("comm-mods-open-queue")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded_full()
                            .bg(rgb(pal.success))
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.active_button_fg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.navigate_to(CommunityRoute::ModQueue(Some(slug.clone())), cx);
                                }),
                            )
                            .child(tr_str(lang, "打开审核与 AutoMod 台", "Open Mod Queue")),
                    ),
            )
        })
        .when(det.moderators.is_empty(), |col| {
            col.child(render_empty_state(
                IconName::ShieldMod,
                tr_str(lang, "暂无版主记录", "No moderators listed"),
                tr_str(lang, "社区创建者将成为首位版主。", "Community founders become the initial moderators."),
                &pal,
            ))
        })
        .children(det.moderators.iter().map(|m| {
            let handle = m.handle.clone();
            let is_bot_mod = m.actor_kind.is_bot_like();
            div()
                .id(SharedString::from(format!("comm-mod-row-{}", m.actor_id)))
                .cursor_pointer()
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_actor_profile(handle.clone(), cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .child(render_actor_avatar(
                            &m.actor_id,
                            &m.display_name,
                            m.actor_kind,
                            m.avatar_media_id.as_deref(),
                            &m.avatar_shape,
                            &m.avatar_color,
                            34.0,
                            pal.card_bg,
                            &ws.media_base_url,
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
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(m.display_name.clone()),
                                        )
                                        .child(render_actor_badge_row(
                                            m.actor_kind,
                                            m.verified_tier,
                                            m.bot_badge.as_deref(),
                                            true,
                                            false,
                                            &m.avatar_color,
                                            &pal,
                                            lang,
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("@{}", m.handle)),
                                ),
                        ),
                )
                .child(render_flair_chip(
                    if is_bot_mod {
                        tr_str(lang, "🤖 Bot 版主 (bot_mod)", "🤖 Bot Mod")
                    } else {
                        tr_str(lang, "🛡 人类版主", "🛡 Human Mod")
                    },
                    if is_bot_mod { Some("#8B5CF6") } else { Some("#10B981") },
                    &pal,
                ))
        }))
}

/// Renders the centered `macos_modal_card` "Create Community" modal overlay (F19).
pub fn render_create_community_modal(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let d = &ws.state.create_community;
    let err = d.error.clone();
    let submitting = d.submitting;
    let hover_bg = pal.row_hover_rgba();

    let field_box = |id: &'static str,
                     text: &str,
                     placeholder: &str,
                     target: ActiveCommunityInput,
                     min_h_px: f32,
                     cx: &mut Context<CommunityWorkspace>| {
        let focused = ws.state.active_input == target;
        let preedit = if focused { ws.state.ime_preedit.as_deref() } else { None };
        div()
            .id(id)
            .cursor_text()
            .w_full()
            .min_h(px(min_h_px))
            .px(px(10.0))
            .py(px(7.0))
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
    };

    let card = macos_modal_card("community-create-modal-card", pal)
        .w(px(pal.scaled_px(540.0).min(600.0)))
        .max_w_full()
        .max_h(px(640.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(tg_icon(IconName::CommunityGlobe, 16.0, pal.accent))
                        .child(
                            div()
                                .text_size(px(14.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "创建新的 c/ 社区", "Create a New c/ Community")),
                        ),
                )
                .child(
                    div()
                        .id("create-comm-close")
                        .cursor_pointer()
                        .p(px(5.0))
                        .rounded_full()
                        .hover(move |s| s.bg(hover_bg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_create_community_modal(cx);
                            }),
                        )
                        .child(tg_icon(IconName::Close, 14.0, pal.text_secondary)),
                ),
        )
        .child(
            div()
                .id("create-comm-body-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(9.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "社区标识 (slug，仅限 2–32 位小写字母/数字/下划线)", "Community Slug (2–32 lowercase [a-z0-9_])")),
                )
                .child(field_box(
                    "create-comm-slug-input",
                    &d.slug,
                    "rust_agents",
                    ActiveCommunityInput::CreateCommSlug,
                    34.0,
                    cx,
                ))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "社区展示名称", "Display Title")),
                )
                .child(field_box(
                    "create-comm-name-input",
                    &d.name,
                    tr_str(lang, "Rust 与智能体工程", "Rust & Autonomous Agents"),
                    ActiveCommunityInput::CreateCommName,
                    34.0,
                    cx,
                ))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "社区简介", "Description")),
                )
                .child(field_box(
                    "create-comm-desc-input",
                    &d.description,
                    tr_str(lang, "介绍社区主题、讨论规范与人机协作愿景…", "Describe your community topic and human+bot collaboration…"),
                    ActiveCommunityInput::CreateCommDescription,
                    64.0,
                    cx,
                ))
                // Category Picker
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
                                .child(tr_str(lang, "分类:", "Category:")),
                        )
                        .children(COMMUNITY_CATEGORIES.into_iter().map(|(cat, zh, en)| {
                            let active = d.category == cat;
                            div()
                                .id(SharedString::from(format!("create-comm-cat-{cat}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.5))
                                .rounded_full()
                                .bg(if active { rgb(pal.accent).into() } else { pal.service_pill_rgba() })
                                .text_size(px(11.0))
                                .text_color(rgb(if active { pal.active_button_fg } else { pal.text_primary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.create_community.category = cat.to_string();
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        })),
                )
                // Emoji & Accent Swatches
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(6.0))
                        .children(COMMUNITY_EMOJIS.into_iter().enumerate().map(|(i, em)| {
                            let active = d.icon_emoji == em;
                            div()
                                .id(SharedString::from(format!("create-comm-emoji-{i}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(7.0))
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(14.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.create_community.icon_emoji = em.to_string();
                                        cx.notify();
                                    }),
                                )
                                .child(em)
                        }))
                        .child(div().w(px(1.0)).h(px(16.0)).bg(pal.separator_rgba()))
                        .children(COMMUNITY_ACCENTS.into_iter().enumerate().map(|(i, hex)| {
                            let col = parse_fabushi_hex_color(hex).unwrap_or(pal.accent);
                            let active = d.accent_color.eq_ignore_ascii_case(hex);
                            div()
                                .id(SharedString::from(format!("create-comm-accent-{i}")))
                                .cursor_pointer()
                                .w(px(22.0))
                                .h(px(22.0))
                                .rounded_full()
                                .bg(rgb(col))
                                .border_2()
                                .border_color(if active {
                                    rgb(pal.text_primary).into()
                                } else {
                                    pal.hairline_border_rgba()
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.create_community.accent_color = hex.to_string();
                                        cx.notify();
                                    }),
                                )
                        })),
                )
                // Policy Toggles: allow_bots, require_bot_disclosure, is_nsfw
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("create-comm-toggle-bots")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(5.0))
                                .rounded_full()
                                .bg(if d.allow_bots {
                                    TdesktopThemePalette::rgba_with_alpha(pal.success, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if d.allow_bots { pal.success } else { pal.text_primary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.create_community.allow_bots = !this.state.create_community.allow_bots;
                                        cx.notify();
                                    }),
                                )
                                .child(if d.allow_bots {
                                    tr_str(lang, "☑ 允许 Bot 参与", "☑ Allow Bots")
                                } else {
                                    tr_str(lang, "☐ 允许 Bot 参与", "☐ Allow Bots")
                                }),
                        )
                        .child(
                            div()
                                .id("create-comm-toggle-disclosure")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(5.0))
                                .rounded_full()
                                .bg(if d.require_bot_disclosure {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if d.require_bot_disclosure { pal.accent } else { pal.text_primary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.create_community.require_bot_disclosure =
                                            !this.state.create_community.require_bot_disclosure;
                                        cx.notify();
                                    }),
                                )
                                .child(if d.require_bot_disclosure {
                                    tr_str(lang, "☑ 要求 Bot 披露身份", "☑ Require Bot Disclosure")
                                } else {
                                    tr_str(lang, "☐ 要求 Bot 披露身份", "☐ Require Bot Disclosure")
                                }),
                        )
                        .child(
                            div()
                                .id("create-comm-toggle-nsfw")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(5.0))
                                .rounded_full()
                                .bg(if d.is_nsfw {
                                    TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if d.is_nsfw { pal.danger } else { pal.text_primary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.create_community.is_nsfw = !this.state.create_community.is_nsfw;
                                        cx.notify();
                                    }),
                                )
                                .child(if d.is_nsfw { "☑ 18+ NSFW" } else { "☐ 18+ NSFW" }),
                        ),
                ),
        )
        .child(
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
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            macos_secondary_pill_btn("create-comm-cancel", tr_str(lang, "取消", "Cancel"), pal)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.close_create_community_modal(cx);
                                    }),
                                ),
                        )
                        .child(
                            macos_primary_pill_btn(
                                "create-comm-submit",
                                if submitting {
                                    tr_str(lang, "创建中…", "Creating…")
                                } else {
                                    tr_str(lang, "创建并进入社区", "Create Community")
                                },
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.submit_create_community(cx);
                                }),
                            ),
                        ),
                ),
        );

    div()
        .id("community-create-modal-backdrop")
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
                this.close_create_community_modal(cx);
            }),
        )
        .child(card)
        .into_any_element()
}
