//! Feature F17 — Dual-Mode Post Detail (Reddit Collapsible Nested Comment Tree +
//! X Linear Thread & Quote Chain) and 1-Click "Summon Bot" Picker.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_community::ModPermissionValidator;
use gpui_telegram_proto::community_contracts::{
    CommentSort, CommentsQueryParams, CommunityActorDto, CommunityCommentDto, CommunityMemberRole,
    CommunityPostDto, EngagementSignalKind, FeedItemDto, GiveAwardRequest, ModActionKind,
    ModActionRequest, PostStatus, RecordSignalRequest, SummonBotRequest, UpdateCommentRequest,
    VoteRequest,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::macos_popover_surface;
use crate::views::stage::entities_render::render_rich_message_body;

use super::post_card::render_post_card_expanded;
use super::shared_widgets::{
    format_compact_count, format_relative_time, linkify_community_slugs, render_actor_avatar,
    render_actor_badge_row, render_empty_state, render_flair_chip, render_input_display,
    render_loading_skeleton, render_vote_stepper_pill,
};
use super::state::{
    ActiveCommunityInput, CommunityPopover, CommunityRoute, CommunityWorkspaceAction,
    PostDetailMode,
};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

const DEPTH_RAIL_HUES: [&str; 6] = ["blue", "violet", "green", "orange", "magenta", "cyan"];

fn count_descendants(comment: &CommunityCommentDto) -> usize {
    let mut total = comment.children.len() + comment.more_children_count as usize;
    for ch in &comment.children {
        total += count_descendants(ch);
    }
    total
}

fn depth_rail_color(depth: u32, is_bot: bool, pal: &TdesktopThemePalette) -> Rgba {
    if is_bot && depth == 0 {
        let (_, _, text_rgb) = TdesktopThemePalette::bot_hue_colors("violet", pal.mode.is_dark());
        return TdesktopThemePalette::rgba_with_alpha(text_rgb, 0.75);
    }
    let hue = DEPTH_RAIL_HUES[(depth as usize) % DEPTH_RAIL_HUES.len()];
    let (_, _, text_rgb) = TdesktopThemePalette::bot_hue_colors(hue, pal.mode.is_dark());
    TdesktopThemePalette::rgba_with_alpha(text_rgb, if pal.mode.is_dark() { 0.65 } else { 0.55 })
}

fn viewer_role_for_community(ws: &CommunityWorkspace, slug: Option<&str>) -> Option<CommunityMemberRole> {
    let target = slug?.trim();
    if target.is_empty() {
        return None;
    }
    if let Some(det) = ws.state.community_detail.as_ref() {
        if det.community.slug.eq_ignore_ascii_case(target) {
            if det.community.viewer_role.is_some() {
                return det.community.viewer_role;
            }
        }
    }
    ws.state
        .communities
        .iter()
        .find(|c| c.slug.eq_ignore_ascii_case(target))
        .and_then(|c| c.viewer_role)
}

fn community_allows_bots(ws: &CommunityWorkspace, slug: Option<&str>) -> bool {
    let Some(target) = slug.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    if let Some(det) = ws.state.community_detail.as_ref() {
        if det.community.slug.eq_ignore_ascii_case(target) {
            return det.community.allow_bots;
        }
    }
    ws.state
        .communities
        .iter()
        .find(|c| c.slug.eq_ignore_ascii_case(target))
        .map(|c| c.allow_bots)
        .unwrap_or(true)
}

fn collect_available_bots(ws: &CommunityWorkspace) -> Vec<CommunityActorDto> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    if let Some(ids) = ws.state.identities.as_ref() {
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
    for a in &ws.state.recommended_actors {
        if a.actor_kind.is_bot_like() && seen.insert(a.actor_id.clone()) {
            out.push(a.clone());
        }
    }
    out
}

impl CommunityWorkspace {
    pub fn set_detail_mode(&mut self, mode: PostDetailMode, cx: &mut Context<Self>) {
        if self.state.detail_mode != mode {
            self.state.detail_mode = mode;
            cx.notify();
        }
    }

    pub fn toggle_comment_collapsed(&mut self, comment_id: String, cx: &mut Context<Self>) {
        if !self.state.detail_collapsed_comments.remove(&comment_id) {
            self.state.detail_collapsed_comments.insert(comment_id);
        }
        cx.notify();
    }

    pub fn start_edit_comment(&mut self, comment_id: String, current_body: String, cx: &mut Context<Self>) {
        self.state.detail_editing_comment_id = Some(comment_id);
        self.state.detail_edit_comment_draft = current_body;
        self.state.focus_input(ActiveCommunityInput::DetailEditComment);
        cx.notify();
    }

    pub fn cancel_edit_comment(&mut self, cx: &mut Context<Self>) {
        self.state.detail_editing_comment_id = None;
        self.state.detail_edit_comment_draft.clear();
        if self.state.active_input == ActiveCommunityInput::DetailEditComment {
            self.state.blur_input();
        }
        cx.notify();
    }

    pub fn submit_edit_comment(&mut self, cx: &mut Context<Self>) {
        let Some(cid) = self.state.detail_editing_comment_id.clone() else {
            return;
        };
        let body = self.state.detail_edit_comment_draft.trim().to_string();
        if body.is_empty() {
            let lang = self.language_id.clone();
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: tr_str(&lang, "评论内容不能为空", "Comment body cannot be empty").to_string(),
                    is_error: true,
                },
                cx,
            );
            return;
        }
        let req = UpdateCommentRequest { body };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.update_comment(&cid, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(updated)) => {
                    this.state.visit_comment_mut(&updated.comment_id, |c| {
                        c.body = updated.body.clone();
                        c.updated_at_ms = updated.updated_at_ms;
                    });
                    this.cancel_edit_comment(cx);
                    let lang = this.language_id.clone();
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "评论已更新", "Comment updated").to_string(),
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
            });
        })
        .detach();
    }

    pub fn vote_comment_action(&mut self, comment_id: String, clicked_dir: i8, cx: &mut Context<Self>) {
        let (next_dir, snap) = self.state.apply_optimistic_comment_vote(&comment_id, clicked_dir);
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let cid = comment_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.vote_comment(&cid, &VoteRequest { direction: next_dir }).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(resp)) => {
                    this.state.reconcile_comment_vote(&resp);
                    cx.notify();
                }
                Ok(Err(e)) => {
                    if let Some(s) = snap {
                        this.state.rollback_comment_vote(&comment_id, s);
                    }
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast { text: e.to_string(), is_error: true },
                        cx,
                    );
                }
                Err(_) => {
                    if let Some(s) = snap {
                        this.state.rollback_comment_vote(&comment_id, s);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub fn toggle_comment_like_action(&mut self, comment_id: String, cx: &mut Context<Self>) {
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.toggle_comment_like(&comment_id).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(resp)) => {
                    this.state.reconcile_comment_like(&resp);
                    cx.notify();
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

    pub fn give_comment_award_action(&mut self, comment_id: String, award_kind: String, cx: &mut Context<Self>) {
        self.state.active_popover = None;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let req = GiveAwardRequest { award_kind, message: None };
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.give_comment_award(&comment_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(resp)) => {
                    this.state.reconcile_comment_awards(&resp);
                    let lang = this.language_id.clone();
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "已为评论颁发徽章！", "Award granted to comment!").to_string(),
                            is_error: false,
                        },
                        cx,
                    );
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

    pub fn delete_comment_action(&mut self, comment_id: String, cx: &mut Context<Self>) {
        let reload_pid = self.state.detail_post.as_ref().map(|d| d.post.post_id.clone());
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.delete_comment(&comment_id).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(_)) => {
                    if let Some(pid) = reload_pid {
                        if this.state.route == CommunityRoute::PostDetail(pid.clone()) {
                            this.load_post_detail(pid, cx);
                        }
                    }
                    let lang = this.language_id.clone();
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "评论已删除", "Comment deleted").to_string(),
                            is_error: false,
                        },
                        cx,
                    );
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

    pub fn load_more_comment_children(&mut self, post_id: String, parent_comment_id: String, cx: &mut Context<Self>) {
        if !self.state.detail_loading_more_comment_ids.insert(parent_comment_id.clone()) {
            return;
        }
        let my_gen = self.state.detail_gen;
        let sort = self.state.detail_comment_sort;
        let cursor = self
            .state
            .detail_comment_more_cursors
            .get(&parent_comment_id)
            .cloned();
        let params = CommentsQueryParams {
            sort: Some(sort),
            parent_id: Some(parent_comment_id.clone()),
            max_depth: None,
            limit: Some(50),
            cursor,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pcid = parent_comment_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.get_comments(&post_id, &params).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                if this.state.detail_gen != my_gen {
                    return;
                }
                this.state.detail_loading_more_comment_ids.remove(&pcid);
                match res {
                    Ok(Ok(resp)) => {
                        if resp.has_more {
                            if let Some(next) = resp.next_cursor.filter(|s| !s.trim().is_empty()) {
                                this.state
                                    .detail_comment_more_cursors
                                    .insert(pcid.clone(), next);
                            } else {
                                this.state.detail_comment_more_cursors.remove(&pcid);
                            }
                        } else {
                            this.state.detail_comment_more_cursors.remove(&pcid);
                        }
                        this.state
                            .merge_loaded_comment_children(&pcid, resp.comments, resp.has_more);
                        cx.notify();
                    }
                    Ok(Err(e)) => {
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast { text: e.to_string(), is_error: true },
                            cx,
                        );
                    }
                    Err(_) => {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub fn execute_comment_mod_action(
        &mut self,
        community_slug: String,
        comment_id: String,
        action: ModActionKind,
        cx: &mut Context<Self>,
    ) {
        let reload_pid = self.state.detail_post.as_ref().map(|d| d.post.post_id.clone());
        let req = ModActionRequest {
            action,
            target_kind: "comment".to_string(),
            target_id: comment_id,
            reason: Some("Moderator action from Post Detail".to_string()),
            flair: None,
            queue_item_id: None,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.execute_mod_action(&community_slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(_)) => {
                    if let Some(pid) = reload_pid {
                        if this.state.route == CommunityRoute::PostDetail(pid.clone()) {
                            this.load_post_detail(pid, cx);
                        }
                    }
                    let lang = this.language_id.clone();
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "版务操作已生效", "Moderator action applied").to_string(),
                            is_error: false,
                        },
                        cx,
                    );
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

    pub fn execute_post_mod_action(
        &mut self,
        community_slug: String,
        post_id: String,
        action: ModActionKind,
        cx: &mut Context<Self>,
    ) {
        let req = ModActionRequest {
            action,
            target_kind: "post".to_string(),
            target_id: post_id.clone(),
            reason: Some("Moderator action from Post Detail".to_string()),
            flair: None,
            queue_item_id: None,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.execute_mod_action(&community_slug, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(_)) => {
                    if this.state.route == CommunityRoute::PostDetail(post_id.clone()) {
                        this.load_post_detail(post_id, cx);
                    }
                    let lang = this.language_id.clone();
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "帖子版务操作已更新", "Post moderation updated").to_string(),
                            is_error: false,
                        },
                        cx,
                    );
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

    pub fn delete_post_action(&mut self, post_id: String, navigate_back: bool, cx: &mut Context<Self>) {
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let pid = post_id.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.delete_post(&pid).await
            })
            .await;
            let _ = weak.update(app, |this, cx| match res {
                Ok(Ok(_)) => {
                    this.state.remove_post_from_caches(&post_id);
                    if this.state.detail_post.as_ref().is_some_and(|d| d.post.post_id == post_id) {
                        this.state.detail_post = None;
                    }
                    let lang = this.language_id.clone();
                    this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "帖子已删除", "Post deleted").to_string(),
                            is_error: false,
                        },
                        cx,
                    );
                    if navigate_back {
                        this.go_back(cx);
                    } else {
                        cx.notify();
                    }
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

    pub fn report_post_or_comment(
        &mut self,
        post_id: String,
        target_actor_id: Option<String>,
        community_slug: Option<String>,
        reason: String,
        cx: &mut Context<Self>,
    ) {
        let req = RecordSignalRequest {
            post_id,
            kind: EngagementSignalKind::Report,
            dwell_ms: None,
            reason: Some(reason),
            target_actor_id,
            community_slug,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.record_signal(&req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                let lang = this.language_id.clone();
                match res {
                    Ok(Ok(_)) => this.emit_action(
                        CommunityWorkspaceAction::ShowToast {
                            text: tr_str(&lang, "已提交举报，交由社区版务审核", "Report submitted to moderation queue")
                                .to_string(),
                            is_error: false,
                        },
                        cx,
                    ),
                    Ok(Err(e)) => this.emit_action(
                        CommunityWorkspaceAction::ShowToast { text: e.to_string(), is_error: true },
                        cx,
                    ),
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    pub fn summon_bot_with_options(
        &mut self,
        post_id: String,
        bot_actor_id: Option<String>,
        prompt: Option<String>,
        parent_comment_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let lang = self.language_id.clone();
        if self.state.detail_summon_pending {
            let msg = tr_str(
                &lang,
                "Bot 召唤请求正在进行中，请稍候…",
                "Bot summon already in progress…",
            )
            .to_string();
            self.state.detail_summon_feedback = Some((msg.clone(), false));
            self.emit_action(
                CommunityWorkspaceAction::ShowToast {
                    text: msg,
                    is_error: false,
                },
                cx,
            );
            return;
        }
        if let Some(det) = self.state.detail_post.as_ref() {
            if det.post.post_id == post_id {
                if det.post.is_locked || det.post.status == PostStatus::Deleted {
                    let msg = tr_str(&lang, "该帖子已锁定或已删除，无法召唤 Bot", "Cannot summon Bot on a locked or deleted post").to_string();
                    self.state.detail_summon_feedback = Some((msg.clone(), true));
                    self.emit_action(CommunityWorkspaceAction::ShowToast { text: msg, is_error: true }, cx);
                    return;
                }
                if !community_allows_bots(self, det.post.community_slug.as_deref()) {
                    let msg = tr_str(&lang, "该社区禁止 Bot 参与互动 (allow_bots = false)", "This community disallows Bots (allow_bots = false)").to_string();
                    self.state.detail_summon_feedback = Some((msg.clone(), true));
                    self.emit_action(CommunityWorkspaceAction::ShowToast { text: msg, is_error: true }, cx);
                    return;
                }
            }
        }
        self.state.detail_summon_pending = true;
        self.state.active_popover = None;
        if self.state.active_input == ActiveCommunityInput::DetailSummonPrompt {
            self.state.blur_input();
        }
        let req = SummonBotRequest {
            bot_actor_id,
            bot_handle: None,
            prompt: prompt.filter(|s| !s.trim().is_empty()).or_else(|| {
                Some("Join this discussion with a thoughtful perspective.".to_string())
            }),
            parent_comment_id,
        };
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        let reload_pid = post_id.clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade.summon_bot(&post_id, &req).await
            })
            .await;
            let _ = weak.update(app, |this, cx| {
                this.state.detail_summon_pending = false;
                let lang = this.language_id.clone();
                let still_on_post = this.state.route == CommunityRoute::PostDetail(reload_pid.clone());
                match res {
                    Ok(Ok(r)) => {
                        if r.triggered {
                            let ok_msg = tr_str(&lang, "已召唤 Bot 参与讨论！", "Bot summoned to discussion!").to_string();
                            if still_on_post {
                                this.state.detail_summon_feedback = Some((ok_msg.clone(), false));
                            }
                            if let (Some(created), Some(det)) =
                                (r.created_comment, &mut this.state.detail_post)
                            {
                                if det.post.post_id == reload_pid && created.parent_comment_id.is_none() {
                                    if !det.comments.iter().any(|c| c.comment_id == created.comment_id) {
                                        det.comments.insert(0, created);
                                        det.total_comments = det.total_comments.saturating_add(1);
                                        det.post.comments_count = det.post.comments_count.saturating_add(1);
                                    }
                                }
                            }
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast { text: ok_msg, is_error: false },
                                cx,
                            );
                            if still_on_post {
                                this.load_post_detail(reload_pid, cx);
                            }
                        } else {
                            let reason = if r.reason.trim().is_empty() {
                                tr_str(&lang, "Bot 当前处于冷却期或防循环保护中", "Bot declined due to cooldown or anti-loop guard").to_string()
                            } else {
                                r.reason
                            };
                            if still_on_post {
                                this.state.detail_summon_feedback = Some((reason.clone(), true));
                            }
                            this.emit_action(
                                CommunityWorkspaceAction::ShowToast { text: reason, is_error: true },
                                cx,
                            );
                        }
                    }
                    Ok(Err(e)) => {
                        let msg = e.to_string();
                        if still_on_post {
                            this.state.detail_summon_feedback = Some((msg.clone(), true));
                        }
                        this.emit_action(
                            CommunityWorkspaceAction::ShowToast { text: msg, is_error: true },
                            cx,
                        );
                    }
                    Err(e) => {
                        if still_on_post {
                            this.state.detail_summon_feedback = Some((e, true));
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}

/// Renders the full F17 Dual-Mode Post Detail view (`Reddit Nested Tree` vs `X Linear Thread & Quote Chain`).
pub fn render_post_detail_view(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let det = ws.state.detail_post.clone();
    let header_title = det
        .as_ref()
        .and_then(|d| d.post.community_slug.as_deref())
        .map(|s| format!("c/{s} · {}", tr_str(lang, "帖子详情", "Post Detail")))
        .unwrap_or_else(|| tr_str(lang, "帖子详情与讨论树", "Post Detail & Thread").to_string());
    let header_sub = det.as_ref().map(|d| {
        format!(
            "{} {} · {} {} · {}",
            d.total_comments,
            tr_str(lang, "条评论", "comments"),
            d.thread_chain.len().max(1),
            tr_str(lang, "条串贴", "thread parts"),
            format_relative_time(d.post.created_at_ms, lang)
        )
    });

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(header_title, header_sub, cx))
        .child(
            div()
                .id("community-detail-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .track_scroll(&ws.detail_scroll_handle)
                .p(px(14.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .when(ws.state.detail_loading && det.is_none(), |col| {
                            col.child(render_loading_skeleton(2, &pal))
                        })
                        .when(!ws.state.detail_loading && det.is_none(), |col| {
                            let sub = ws.state.detail_error.as_deref().unwrap_or_else(|| {
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
                            let is_mod = ModPermissionValidator::can_moderate(
                                viewer_role_for_community(ws, d.post.community_slug.as_deref()),
                            );
                            let is_owner = ws.is_viewer_owned_actor(&d.post.author_actor_id);
                            let mode = ws.state.detail_mode;
                            let root_item = FeedItemDto {
                                post: d.post.clone(),
                                explanation: None,
                                rank_score: 0.0,
                                is_exploration_slot: false,
                            };

                            col.child(render_post_card_expanded(ws, &root_item, 0, cx))
                                .child(render_post_management_bar(ws, &d.post, is_owner, is_mod, cx))
                                .child(render_mode_and_sort_bar(ws, &d, cx))
                                .when(mode == PostDetailMode::RedditTree, |tree_col| {
                                    tree_col
                                        .child(render_inline_reply_box(ws, &d.post, cx))
                                        .when(d.comments.is_empty(), |c| {
                                            c.child(render_empty_state(
                                                IconName::Reply,
                                                tr_str(lang, "暂无评论", "No comments yet"),
                                                tr_str(
                                                    lang,
                                                    "成为第一个发表观点的人类或 Bot，或点击上方按钮召唤 Bot 参与讨论！",
                                                    "Be the first Human or Bot to reply, or summon a Bot above!",
                                                ),
                                                &pal,
                                            ))
                                        })
                                        .children(d.comments.iter().map(|cmt| {
                                            render_comment_tree_node(ws, &d.post, cmt, is_mod, cx)
                                        }))
                                })
                                .when(mode == PostDetailMode::XThread, |x_col| {
                                    x_col
                                        .child(render_x_thread_and_quote_panel(ws, &d, cx))
                                        .child(render_inline_reply_box(ws, &d.post, cx))
                                })
                        }),
                ),
        )
        .into_any_element()
}

fn render_post_management_bar(
    ws: &CommunityWorkspace,
    post: &CommunityPostDto,
    is_owner: bool,
    is_mod: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let pid = post.post_id.clone();
    let slug = post.community_slug.clone();
    let edit_post = post.clone();
    let del_pid = pid.clone();
    let rep_pid = pid.clone();
    let rep_author = Some(post.author_actor_id.clone());
    let rep_slug = slug.clone();
    let cross_post = post.clone();
    let hover_bg = pal.row_hover_rgba();

    div()
        .w_full()
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(9.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(6.0))
        .child(
            div()
                .id("detail-post-crosspost-btn")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(7.0))
                .bg(pal.service_pill_rgba())
                .hover(move |s| s.bg(hover_bg))
                .text_size(px(11.0))
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_composer_crosspost(cross_post.clone(), cx);
                    }),
                )
                .child(tr_str(lang, "跨社区转发", "Crosspost")),
        )
        .when(is_owner && post.status != PostStatus::Deleted, |row| {
            row.child(
                div()
                    .id("detail-post-edit-btn")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(7.0))
                    .bg(pal.service_pill_rgba())
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(11.0))
                    .text_color(rgb(pal.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_composer_edit(edit_post.clone(), cx);
                        }),
                    )
                    .child(tg_icon(IconName::Edit, 11.5, pal.accent))
                    .child(tr_str(lang, "编辑帖子", "Edit")),
            )
            .child(
                div()
                    .id("detail-post-delete-btn")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(7.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                    .hover(move |s| s.bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.22)))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(11.0))
                    .text_color(rgb(pal.danger))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_post_action(del_pid.clone(), true, cx);
                        }),
                    )
                    .child(tg_icon(IconName::DeleteTrash, 11.5, pal.danger))
                    .child(tr_str(lang, "删除", "Delete")),
            )
        })
        .child(
            div()
                .id("detail-post-report-btn")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(7.0))
                .bg(pal.service_pill_rgba())
                .hover(move |s| s.bg(hover_bg))
                .text_size(px(11.0))
                .text_color(rgb(pal.text_secondary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.report_post_or_comment(
                            rep_pid.clone(),
                            rep_author.clone(),
                            rep_slug.clone(),
                            "User reported post".to_string(),
                            cx,
                        );
                    }),
                )
                .child(tr_str(lang, "举报", "Report")),
        )
        .when_some(slug.filter(|_| is_mod), |row, mod_slug| {
            let pin_act = if post.is_pinned { ModActionKind::Unpin } else { ModActionKind::Pin };
            let lock_act = if post.is_locked { ModActionKind::Unlock } else { ModActionKind::Lock };
            let dist_act = if post.is_mod_distinguished {
                ModActionKind::Undistinguish
            } else {
                ModActionKind::Distinguish
            };
            let s1 = mod_slug.clone();
            let s2 = mod_slug.clone();
            let s3 = mod_slug.clone();
            let s4 = mod_slug;
            let p1 = pid.clone();
            let p2 = pid.clone();
            let p3 = pid.clone();
            let p4 = pid;
            row.child(div().w(px(1.0)).h(px(14.0)).bg(pal.separator_rgba()))
                .child(render_small_mod_btn(
                    "detail-mod-pin",
                    if post.is_pinned { tr_str(lang, "取消置顶", "Unpin") } else { tr_str(lang, "置顶", "Pin") },
                    pal.accent,
                    &pal,
                    cx,
                    move |this, cx| this.execute_post_mod_action(s1.clone(), p1.clone(), pin_act, cx),
                ))
                .child(render_small_mod_btn(
                    "detail-mod-lock",
                    if post.is_locked { tr_str(lang, "解锁", "Unlock") } else { tr_str(lang, "锁定", "Lock") },
                    0xF59E0B,
                    &pal,
                    cx,
                    move |this, cx| this.execute_post_mod_action(s2.clone(), p2.clone(), lock_act, cx),
                ))
                .child(render_small_mod_btn(
                    "detail-mod-dist",
                    if post.is_mod_distinguished { tr_str(lang, "取消版务标识", "Undistinguish") } else { tr_str(lang, "版务标识", "Distinguish") },
                    pal.success,
                    &pal,
                    cx,
                    move |this, cx| this.execute_post_mod_action(s3.clone(), p3.clone(), dist_act, cx),
                ))
                .child(render_small_mod_btn(
                    "detail-mod-remove",
                    tr_str(lang, "移除", "Remove"),
                    pal.danger,
                    &pal,
                    cx,
                    move |this, cx| this.execute_post_mod_action(s4.clone(), p4.clone(), ModActionKind::Remove, cx),
                ))
        })
}

fn render_small_mod_btn(
    id: impl Into<SharedString>,
    label: &str,
    fg_rgb: u32,
    pal: &TdesktopThemePalette,
    cx: &mut Context<CommunityWorkspace>,
    on_press: impl Fn(&mut CommunityWorkspace, &mut Context<CommunityWorkspace>) + 'static,
) -> Stateful<Div> {
    let bg = TdesktopThemePalette::rgba_with_alpha(fg_rgb, if pal.mode.is_dark() { 0.18 } else { 0.11 });
    div()
        .id(id.into())
        .cursor_pointer()
        .px(px(7.0))
        .py(px(3.5))
        .rounded(px(6.0))
        .bg(bg)
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(fg_rgb))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_press(this, cx);
            }),
        )
        .child(label.to_string())
}

fn render_mode_and_sort_bar(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::PostDetailResponse,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let mode = ws.state.detail_mode;
    let active_sort = ws.state.detail_comment_sort;
    let hover_bg = pal.row_hover_rgba();

    let sorts: [(CommentSort, &'static str, &'static str); 6] = [
        (CommentSort::Best, "最佳", "Best"),
        (CommentSort::Top, "高赞", "Top"),
        (CommentSort::New, "最新", "New"),
        (CommentSort::Controversial, "热议", "Controversial"),
        (CommentSort::Old, "最早", "Old"),
        (CommentSort::Qa, "问答", "Q&A"),
    ];

    div()
        .w_full()
        .p(px(10.0))
        .rounded(px(10.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        // Dual Mode Switcher: Reddit Tree vs X Thread & Quote Chain
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .id("detail-mode-reddit-tree")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded_full()
                        .bg(if mode == PostDetailMode::RedditTree {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if mode == PostDetailMode::RedditTree {
                            pal.active_button_fg
                        } else {
                            pal.text_primary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_detail_mode(PostDetailMode::RedditTree, cx);
                            }),
                        )
                        .child(format!(
                            "{} ({})",
                            tr_str(lang, "Reddit 嵌套评论树", "Reddit Comment Tree"),
                            det.total_comments
                        )),
                )
                .child(
                    div()
                        .id("detail-mode-x-thread")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded_full()
                        .bg(if mode == PostDetailMode::XThread {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if mode == PostDetailMode::XThread {
                            pal.active_button_fg
                        } else {
                            pal.text_primary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_detail_mode(PostDetailMode::XThread, cx);
                            }),
                        )
                        .child(format!(
                            "{} ({} / {})",
                            tr_str(lang, "X 串贴与引用链", "X Thread & Quote Chain"),
                            det.thread_chain.len().max(1),
                            det.quote_chain.len()
                        )),
                ),
        )
        // 6-variant Reddit CommentSort bar
        .when(mode == PostDetailMode::RedditTree, |box_el| {
            box_el.child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(5.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(lang, "排序:", "Sort:")),
                    )
                    .children(sorts.into_iter().map(|(s, zh, en)| {
                        let selected = active_sort == s;
                        div()
                            .id(SharedString::from(format!("detail-sort-{}", s.as_str())))
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(4.0))
                            .rounded_full()
                            .bg(if selected {
                                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.18)
                            } else {
                                pal.service_pill_rgba()
                            })
                            .hover(move |st| st.bg(hover_bg))
                            .text_size(px(11.0))
                            .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                            .text_color(rgb(if selected { pal.accent } else { pal.text_secondary }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_detail_comment_sort(s, cx);
                                }),
                            )
                            .child(tr_str(lang, zh, en))
                    })),
            )
        })
}

fn render_inline_reply_box(
    ws: &CommunityWorkspace,
    post: &CommunityPostDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let is_locked = post.is_locked || post.status == PostStatus::Deleted;
    let reply_focused = ws.state.active_input == ActiveCommunityInput::DetailReply;
    let pid = post.post_id.clone();
    let summon_pid = pid.clone();
    let active_actor = ws.state.active_actor().cloned();
    let reply_parent_handle = ws
        .state
        .detail_reply_parent_id
        .as_deref()
        .and_then(|cid| ws.state.find_comment(cid))
        .map(|c| c.author_handle.clone());
    let bots_allowed = community_allows_bots(ws, post.community_slug.as_deref());
    let summon_pending = ws.state.detail_summon_pending;
    let summon_feedback = ws.state.detail_summon_feedback.clone();

    if is_locked {
        return div()
            .p(px(12.0))
            .rounded(px(10.0))
            .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.10))
            .border_1()
            .border_color(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.30))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(tg_icon(IconName::Lock, 15.0, pal.danger))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(pal.text_primary))
                    .child(if post.status == PostStatus::Deleted {
                        tr_str(lang, "该帖子已被删除，无法继续发表新回复。", "This post has been deleted. New replies are disabled.")
                    } else {
                        tr_str(lang, "该帖子已被版务锁定，禁止发表新评论或召唤 Bot。", "This post is locked by moderators. Replies and bot summons are disabled.")
                    }),
            );
    }

    div()
        .p(px(11.0))
        .rounded(px(10.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
        // Act-As Identity Switcher + Reply Target Indicator
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .id("detail-reply-act-as-pill")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(pal.service_pill_rgba())
                        .hover(move |s| s.bg(pal.row_hover_rgba()))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
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
                                pal.card_bg,
                                &ws.media_base_url,
                                true,
                                &pal,
                            ))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!(
                                        "{} @{}",
                                        tr_str(lang, "发言身份:", "Replying as:"),
                                        actor.handle
                                    )),
                            )
                        })
                        .child(tg_icon(IconName::ChevronDown, 11.0, pal.text_secondary)),
                )
                .when_some(ws.state.detail_reply_parent_id.clone(), |row, pcid| {
                    let target_lbl = reply_parent_handle
                        .map(|h| format!("@{h}"))
                        .unwrap_or_else(|| pcid.chars().take(12).collect());
                    row.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded_full()
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.accent))
                                    .child(format!("{} {target_lbl}", tr_str(lang, "回复给", "Replying to"))),
                            )
                            .child(
                                div()
                                    .id("detail-clear-reply-parent")
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.state.detail_reply_parent_id = None;
                                            cx.notify();
                                        }),
                                    )
                                    .child("×"),
                            ),
                    )
                }),
        )
        // Multiline IME input box
        .child(
            div()
                .id("detail-reply-input")
                .cursor_text()
                .w_full()
                .min_h(px(54.0))
                .p(px(9.0))
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
                    &ws.state.detail_reply_draft,
                    if reply_focused {
                        ws.state.ime_preedit.as_deref()
                    } else {
                        None
                    },
                    tr_str(
                        lang,
                        "写下你的回复，支持 Markdown 和 @Bot 提及… (Shift+Enter 换行，Enter 发送)",
                        "Write your reply with Markdown or @Bot mentions… (Shift+Enter newline, Enter send)",
                    ),
                    reply_focused,
                    &pal,
                )),
        )
        // Action row: Summon Bot Picker + Submit Reply
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap(px(8.0))
                .child(
                    div()
                        .id("detail-summon-bot-btn")
                        .cursor_pointer()
                        .px(px(11.0))
                        .py(px(5.5))
                        .rounded_full()
                        .bg(if bots_allowed {
                            let (_, pill_bg, _) =
                                TdesktopThemePalette::bot_hue_colors("violet", pal.mode.is_dark());
                            rgb(pill_bg).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if bots_allowed {
                            TdesktopThemePalette::bot_hue_colors("violet", pal.mode.is_dark()).2
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if this.state.detail_summon_pending {
                                    let lang = this.language_id.clone();
                                    let msg = tr_str(
                                        &lang,
                                        "Bot 召唤请求正在进行中，请稍候…",
                                        "Bot summon already in progress…",
                                    )
                                    .to_string();
                                    this.state.detail_summon_feedback = Some((msg.clone(), false));
                                    this.emit_action(
                                        CommunityWorkspaceAction::ShowToast {
                                            text: msg,
                                            is_error: false,
                                        },
                                        cx,
                                    );
                                    return;
                                }
                                if !bots_allowed {
                                    let lang = this.language_id.clone();
                                    this.emit_action(
                                        CommunityWorkspaceAction::ShowToast {
                                            text: tr_str(
                                                &lang,
                                                "该社区禁止 Bot 参与互动 (allow_bots = false)",
                                                "This community disallows Bots (allow_bots = false)",
                                            )
                                            .to_string(),
                                            is_error: true,
                                        },
                                        cx,
                                    );
                                    return;
                                }
                                this.toggle_popover(
                                    CommunityPopover::SummonBotPicker(summon_pid.clone()),
                                    cx,
                                );
                            }),
                        )
                        .child(tg_icon(
                            IconName::BotRobot,
                            13.0,
                            if bots_allowed {
                                TdesktopThemePalette::bot_hue_colors("violet", pal.mode.is_dark()).2
                            } else {
                                pal.text_secondary
                            },
                        ))
                        .child(if summon_pending {
                            tr_str(lang, "正在召唤 Bot…", "Summoning Bot…")
                        } else if !bots_allowed {
                            tr_str(lang, "该社区禁用 Bot", "Bots Disabled")
                        } else {
                            tr_str(lang, "一键召唤 Bot", "Summon Bot")
                        }),
                )
                .child(
                    div()
                        .id("detail-reply-submit-btn")
                        .cursor_pointer()
                        .px(px(15.0))
                        .py(px(6.0))
                        .rounded_full()
                        .bg(rgb(pal.accent))
                        .hover(move |s| s.bg(rgb(pal.accent_hover)))
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
        )
        .when_some(summon_feedback, |col, (msg, is_err)| {
            let tone = if is_err { pal.danger } else { pal.success };
            col.child(
                div()
                    .px(px(9.0))
                    .py(px(6.0))
                    .rounded(px(7.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(tone, 0.12))
                    .text_size(px(11.5))
                    .text_color(rgb(tone))
                    .child(msg),
            )
        })
}

fn render_comment_tree_node(
    ws: &CommunityWorkspace,
    post: &CommunityPostDto,
    comment: &CommunityCommentDto,
    is_mod: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    // Safely clamp visual indent for deep nesting (depth 12+ never overflows or panics).
    let indent_px = (comment.depth.min(6) as f32) * 16.0;
    let rail_col = depth_rail_color(comment.depth, comment.author_kind.is_bot_like(), &pal);
    let cid = comment.comment_id.clone();
    let is_collapsed = ws.state.detail_collapsed_comments.contains(&cid);
    let is_deleted = comment.is_deleted || comment.is_removed;
    let is_op = comment.is_op || comment.author_actor_id == post.author_actor_id;
    let is_owner = ws.is_viewer_owned_actor(&comment.author_actor_id);
    let is_editing = ws.state.detail_editing_comment_id.as_deref() == Some(cid.as_str());
    let is_focused_target = ws.state.detail_focused_comment_id.as_deref() == Some(cid.as_str());
    let hidden_count = count_descendants(comment);
    let hover_bg = pal.row_hover_rgba();

    let toggle_cid = cid.clone();
    let author_handle = comment.author_handle.clone();
    let vote_cid = cid.clone();
    let like_cid = cid.clone();
    let award_cid = cid.clone();
    let reply_cid = cid.clone();
    let edit_cid = cid.clone();
    let edit_body = comment.body.clone();
    let del_cid = cid.clone();
    let rep_pid = post.post_id.clone();
    let rep_author = Some(comment.author_actor_id.clone());
    let rep_slug = post.community_slug.clone();
    let load_more_pid = post.post_id.clone();
    let load_more_cid = cid.clone();
    let loading_more = ws.state.detail_loading_more_comment_ids.contains(&cid);

    let msg_id = gpui_telegram_community::fnv1a_64(comment.comment_id.as_bytes()).wrapping_add(101);
    let linked_body = linkify_community_slugs(&comment.body);
    let rendered_body = if is_deleted {
        div()
            .text_size(px(12.0))
            .text_color(rgb(pal.text_secondary))
            .child(tr_str(lang, "[已删除]", "[deleted]"))
            .into_any_element()
    } else if !linked_body.trim().is_empty() {
        render_rich_message_body(
            msg_id,
            &linked_body,
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
    } else {
        div().into_any_element()
    };

    let card = div()
        .w_full()
        .p(px(10.0))
        .rounded(px(9.0))
        .bg(if is_focused_target {
            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.08)
        } else {
            pal.card_surface_rgba()
        })
        .border_1()
        .border_color(if is_focused_target {
            rgb(pal.accent).into()
        } else {
            pal.hairline_border_rgba()
        })
        .border_l_2()
        .border_color(rail_col)
        .flex()
        .flex_col()
        .gap(px(5.0))
        // Header row: collapse toggle + avatar + author + OP/MOD/Stickied/Bot badges + time
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(5.0))
                .child(
                    div()
                        .id(SharedString::from(format!("cmt-collapse-{cid}")))
                        .cursor_pointer()
                        .px(px(5.0))
                        .py(px(1.5))
                        .rounded(px(5.0))
                        .bg(pal.service_pill_rgba())
                        .hover(move |s| s.bg(hover_bg))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_comment_collapsed(toggle_cid.clone(), cx);
                            }),
                        )
                        .child(if is_collapsed { "[+]" } else { "[-]" }),
                )
                .when(!is_deleted, |row| {
                    row.child(render_actor_avatar(
                        &comment.author_actor_id,
                        &comment.author_display_name,
                        comment.author_kind,
                        None,
                        &comment.author_avatar_shape,
                        &comment.author_avatar_color,
                        20.0,
                        pal.card_bg,
                        &ws.media_base_url,
                        true,
                        &pal,
                    ))
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
                        is_op,
                        &comment.author_avatar_color,
                        &pal,
                        lang,
                    ))
                })
                .when(is_deleted, |row| {
                    row.child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(lang, "[已删除用户]", "[deleted]")),
                    )
                })
                .when(comment.is_stickied, |row| {
                    row.child(render_flair_chip(
                        tr_str(lang, "置顶评论", "Stickied"),
                        Some("#10B981"),
                        &pal,
                    ))
                })
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "{}{} · {}",
                            if is_deleted { "" } else { "@" },
                            if is_deleted { "" } else { comment.author_handle.as_str() },
                            format_relative_time(comment.created_at_ms, lang)
                        )),
                )
                .when(is_collapsed, |row| {
                    row.child(
                        div()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded_full()
                            .bg(pal.service_pill_rgba())
                            .text_size(px(10.5))
                            .text_color(rgb(pal.accent))
                            .child(format!(
                                "[+] {} {}",
                                hidden_count,
                                tr_str(lang, "条已折叠回复", "hidden replies")
                            )),
                    )
                }),
        )
        // Body OR Inline Edit Box (when expanded)
        .when(!is_collapsed && !is_editing, |col| col.child(rendered_body))
        .when(!is_collapsed && is_editing, |col| {
            let edit_focused = ws.state.active_input == ActiveCommunityInput::DetailEditComment;
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("cmt-edit-input-{cid}")))
                            .cursor_text()
                            .w_full()
                            .min_h(px(48.0))
                            .p(px(8.0))
                            .rounded(px(7.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(if edit_focused {
                                rgb(pal.accent).into()
                            } else {
                                pal.hairline_border_rgba()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.focus_input(ActiveCommunityInput::DetailEditComment);
                                    cx.notify();
                                }),
                            )
                            .child(render_input_display(
                                &ws.state.detail_edit_comment_draft,
                                if edit_focused { ws.state.ime_preedit.as_deref() } else { None },
                                tr_str(lang, "编辑评论内容…", "Edit comment…"),
                                edit_focused,
                                &pal,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("cmt-edit-save-{cid}")))
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(pal.accent))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.submit_edit_comment(cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "保存", "Save")),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("cmt-edit-cancel-{cid}")))
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(pal.service_pill_rgba())
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.cancel_edit_comment(cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "取消", "Cancel")),
                            ),
                    ),
            )
        })
        // Per-comment action bar (Vote, Like, Award, Reply, Edit, Delete, Report, Mod)
        .when(!is_collapsed && !is_deleted, |col| {
            let award_label = if comment.awards.is_empty() {
                tr_str(lang, "奖励", "Award").to_string()
            } else {
                format!("{} {}", comment.awards[0].icon_emoji, comment.awards.len())
            };
            let mod_slug_opt = post.community_slug.clone().filter(|_| is_mod);
            col.child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.0))
                    .pt(px(2.0))
                    .child(render_vote_stepper_pill(
                        format!("cmt-vote-{cid}"),
                        comment.score,
                        comment.viewer_vote,
                        &pal,
                        cx,
                        move |this, dir, cx| this.vote_comment_action(vote_cid.clone(), dir, cx),
                    ))
                    .child(render_cmt_action_chip(
                        format!("cmt-like-{cid}"),
                        format!("♥ {}", format_compact_count(comment.likes_count as i64)),
                        comment.viewer_liked.then_some(pal.danger),
                        &pal,
                        cx,
                        move |this, cx| this.toggle_comment_like_action(like_cid.clone(), cx),
                    ))
                    .child(render_cmt_action_chip(
                        format!("cmt-award-{cid}"),
                        award_label,
                        (!comment.awards.is_empty()).then_some(0xF59E0B),
                        &pal,
                        cx,
                        move |this, cx| {
                            this.toggle_popover(
                                CommunityPopover::AwardComment(award_cid.clone()),
                                cx,
                            );
                        },
                    ))
                    .when(!post.is_locked && post.status != PostStatus::Deleted, |r| {
                        r.child(render_cmt_action_chip(
                            format!("cmt-reply-btn-{cid}"),
                            tr_str(lang, "回复", "Reply").to_string(),
                            Some(pal.accent),
                            &pal,
                            cx,
                            move |this, cx| {
                                this.state.detail_reply_parent_id = Some(reply_cid.clone());
                                this.state.focus_input(ActiveCommunityInput::DetailReply);
                                cx.notify();
                            },
                        ))
                    })
                    .when(is_owner, |r| {
                        r.child(render_cmt_action_chip(
                            format!("cmt-edit-btn-{cid}"),
                            tr_str(lang, "编辑", "Edit").to_string(),
                            None,
                            &pal,
                            cx,
                            move |this, cx| {
                                this.start_edit_comment(edit_cid.clone(), edit_body.clone(), cx);
                            },
                        ))
                        .child(render_cmt_action_chip(
                            format!("cmt-del-btn-{cid}"),
                            tr_str(lang, "删除", "Delete").to_string(),
                            Some(pal.danger),
                            &pal,
                            cx,
                            move |this, cx| this.delete_comment_action(del_cid.clone(), cx),
                        ))
                    })
                    .child(render_cmt_action_chip(
                        format!("cmt-report-btn-{cid}"),
                        tr_str(lang, "举报", "Report").to_string(),
                        None,
                        &pal,
                        cx,
                        move |this, cx| {
                            this.report_post_or_comment(
                                rep_pid.clone(),
                                rep_author.clone(),
                                rep_slug.clone(),
                                "User reported comment".to_string(),
                                cx,
                            );
                        },
                    ))
                    .when_some(mod_slug_opt, |r, ms| {
                        let c1 = cid.clone();
                        let c2 = cid.clone();
                        let c3 = cid.clone();
                        let c4 = cid.clone();
                        let s1 = ms.clone();
                        let s2 = ms.clone();
                        let s3 = ms.clone();
                        let pin_act = if comment.is_stickied { ModActionKind::Unpin } else { ModActionKind::Pin };
                        let dist_act = if comment.is_mod_distinguished {
                            ModActionKind::Undistinguish
                        } else {
                            ModActionKind::Distinguish
                        };
                        r.child(render_small_mod_btn(
                            format!("cmt-mod-approve-{cid}"),
                            tr_str(lang, "批准", "Approve"),
                            pal.success,
                            &pal,
                            cx,
                            move |this, cx| this.execute_comment_mod_action(s1.clone(), c1.clone(), ModActionKind::Approve, cx),
                        ))
                        .child(render_small_mod_btn(
                            format!("cmt-mod-pin-{cid}"),
                            if comment.is_stickied { tr_str(lang, "取消置顶", "Unpin") } else { tr_str(lang, "置顶", "Pin") },
                            pal.accent,
                            &pal,
                            cx,
                            move |this, cx| this.execute_comment_mod_action(s2.clone(), c2.clone(), pin_act, cx),
                        ))
                        .child(render_small_mod_btn(
                            format!("cmt-mod-dist-{cid}"),
                            if comment.is_mod_distinguished { "UNMOD" } else { "MOD" },
                            pal.success,
                            &pal,
                            cx,
                            move |this, cx| this.execute_comment_mod_action(s3.clone(), c3.clone(), dist_act, cx),
                        ))
                        .child(render_small_mod_btn(
                            format!("cmt-mod-rm-{cid}"),
                            tr_str(lang, "移除", "Remove"),
                            pal.danger,
                            &pal,
                            cx,
                            move |this, cx| this.execute_comment_mod_action(ms.clone(), c4.clone(), ModActionKind::Remove, cx),
                        ))
                    }),
            )
        });

    div()
        .w_full()
        .pl(px(indent_px))
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(card)
        .when(!is_collapsed, |tree| {
            tree.children(
                comment
                    .children
                    .iter()
                    .map(|ch| render_comment_tree_node(ws, post, ch, is_mod, cx)),
            )
            .when(comment.more_children_count > 0, |t| {
                t.child(
                    div()
                        .id(SharedString::from(format!("cmt-load-more-{cid}")))
                        .cursor_pointer()
                        .ml(px(16.0))
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .bg(pal.service_pill_rgba())
                        .hover(move |s| s.bg(hover_bg))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.load_more_comment_children(
                                    load_more_pid.clone(),
                                    load_more_cid.clone(),
                                    cx,
                                );
                            }),
                        )
                        .child(if loading_more {
                            tr_str(lang, "正在加载更多回复…", "Loading more replies…").to_string()
                        } else {
                            format!(
                                "{} {} {}",
                                tr_str(lang, "展开更多", "Load"),
                                comment.more_children_count,
                                tr_str(lang, "条回复", "more replies")
                            )
                        }),
                )
            })
        })
}

fn render_cmt_action_chip(
    id: impl Into<SharedString>,
    label: String,
    accent_rgb: Option<u32>,
    pal: &TdesktopThemePalette,
    cx: &mut Context<CommunityWorkspace>,
    on_press: impl Fn(&mut CommunityWorkspace, &mut Context<CommunityWorkspace>) + 'static,
) -> Stateful<Div> {
    let hover_bg = pal.row_hover_rgba();
    div()
        .id(id.into())
        .cursor_pointer()
        .px(px(7.0))
        .py(px(3.5))
        .rounded(px(6.0))
        .bg(pal.service_pill_rgba())
        .hover(move |s| s.bg(hover_bg))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(accent_rgb.unwrap_or(pal.text_secondary)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_press(this, cx);
            }),
        )
        .child(label)
}

fn render_x_thread_and_quote_panel(
    ws: &CommunityWorkspace,
    det: &gpui_telegram_proto::community_contracts::PostDetailResponse,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let current_pid = det.post.post_id.as_str();
    let thread_items: Vec<CommunityPostDto> = if det.thread_chain.is_empty() {
        vec![det.post.clone()]
    } else {
        det.thread_chain.clone()
    };
    let total_parts = thread_items.len();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        // Quote Lineage Chain (Ancestors)
        .when(!det.quote_chain.is_empty(), |col| {
            col.child(
                div()
                    .p(px(11.0))
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
                            .gap(px(6.0))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(tg_icon(IconName::Repost, 13.0, pal.accent))
                            .child(tr_str(lang, "引用溯源链 (Quote Chain)", "Quote Lineage Chain")),
                    )
                    .children(det.quote_chain.iter().enumerate().map(|(qi, qp)| {
                        let nav_pid = qp.post_id.clone();
                        let preview: String = qp.body.chars().take(180).collect();
                        div()
                            .id(SharedString::from(format!("quote-chain-node-{}-{qi}", qp.post_id)))
                            .cursor_pointer()
                            .p(px(9.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.dialogs_bg))
                            .border_l_2()
                            .border_color(rgb(pal.accent))
                            .hover(move |s| s.bg(pal.row_hover_rgba()))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_post_detail(nav_pid.clone(), cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(px(11.5))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(qp.author_display_name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(pal.text_secondary))
                                            .child(format!(
                                                "@{} · {}",
                                                qp.author_handle,
                                                format_relative_time(qp.created_at_ms, lang)
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(pal.text_primary))
                                    .child(preview),
                            )
                    })),
            )
        })
        // Linear X Thread 1/N .. N/N Vertical Spine
        .child(
            div()
                .p(px(11.0))
                .rounded(px(10.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .pb(px(4.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tg_icon(IconName::CommunityThread, 13.5, pal.accent))
                        .child(format!(
                            "{} ({total_parts} {})",
                            tr_str(lang, "X 线性串贴时间轴", "X Linear Thread Spine"),
                            tr_str(lang, "节", "parts")
                        )),
                )
                .children(thread_items.into_iter().enumerate().map(|(ti, tp)| {
                    let is_current = tp.post_id == current_pid;
                    let has_next = ti + 1 < total_parts;
                    let nav_pid = tp.post_id.clone();
                    let vote_pid = tp.post_id.clone();
                    let like_pid = tp.post_id.clone();
                    let msg_id = gpui_telegram_community::fnv1a_64(tp.post_id.as_bytes())
                        .wrapping_add(500 + ti as u64);
                    let linked = linkify_community_slugs(&tp.body);
                    let body_el = render_rich_message_body(
                        msg_id,
                        &linked,
                        &[],
                        &ws.state.revealed_spoilers,
                        &ws.state.expanded_quotes,
                        ws.state.copied_code_msg,
                        &pal,
                        lang,
                        false,
                        cx,
                        |this, action, cx| this.handle_rich_text_action(action, cx),
                    );
                    div()
                        .flex()
                        .gap(px(10.0))
                        .child(
                            div()
                                .w(px(30.0))
                                .flex_shrink_0()
                                .flex()
                                .flex_col()
                                .items_center()
                                .child(render_actor_avatar(
                                    &tp.author_actor_id,
                                    &tp.author_display_name,
                                    tp.author_kind,
                                    tp.author_avatar_media_id.as_deref(),
                                    &tp.author_avatar_shape,
                                    &tp.author_avatar_color,
                                    28.0,
                                    pal.card_bg,
                                    &ws.media_base_url,
                                    true,
                                    &pal,
                                ))
                                .when(has_next, |sp| {
                                    sp.child(div().flex_1().w(px(2.0)).min_h(px(24.0)).my(px(4.0)).bg(rgb(pal.accent)))
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("x-thread-part-{}-{ti}", tp.post_id)))
                                .flex_1()
                                .min_w(px(0.0))
                                .mb(px(8.0))
                                .p(px(10.0))
                                .rounded(px(9.0))
                                .bg(if is_current {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.08)
                                } else {
                                    rgb(pal.dialogs_bg).into()
                                })
                                .border_1()
                                .border_color(if is_current {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.hairline_border_rgba()
                                })
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        if !is_current {
                                            this.open_post_detail(nav_pid.clone(), cx);
                                        }
                                    }),
                                )
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
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(rgb(pal.text_primary))
                                                        .child(tp.author_display_name.clone()),
                                                )
                                                .child(render_actor_badge_row(
                                                    tp.author_kind,
                                                    tp.author_verified_tier,
                                                    tp.author_bot_badge.as_deref(),
                                                    tp.is_mod_distinguished,
                                                    false,
                                                    &tp.author_avatar_color,
                                                    &pal,
                                                    lang,
                                                )),
                                        )
                                        .child(
                                            div()
                                                .px(px(7.0))
                                                .py(px(1.5))
                                                .rounded_full()
                                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.accent))
                                                .child(format!("{}/{}", ti + 1, total_parts)),
                                        ),
                                )
                                .child(body_el)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(render_vote_stepper_pill(
                                            format!("xthread-vote-{}-{ti}", tp.post_id),
                                            tp.score,
                                            tp.viewer_vote,
                                            &pal,
                                            cx,
                                            move |this, dir, cx| this.vote_post(vote_pid.clone(), dir, cx),
                                        ))
                                        .child(render_cmt_action_chip(
                                            format!("xthread-like-{}-{ti}", tp.post_id),
                                            format!("♥ {}", format_compact_count(tp.likes_count as i64)),
                                            tp.viewer_liked.then_some(pal.danger),
                                            &pal,
                                            cx,
                                            move |this, cx| this.toggle_like(like_pid.clone(), cx),
                                        )),
                                ),
                        )
                })),
        )
}

/// Renders the Award Picker popover targeting a specific comment (`give_comment_award`).
pub fn render_comment_award_popover(
    ws: &CommunityWorkspace,
    comment_id: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let awards: [(&str, &str, &str, &str); 4] = [
        ("gold", "🏆", "金星", "Gold"),
        ("insightful", "💡", "洞见", "Insightful"),
        ("silver", "🥈", "银星", "Silver"),
        ("rocket", "🚀", "助推", "Boost"),
    ];
    macos_popover_surface("community-comment-award-popover", pal)
        .w(px(260.0))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(lang, "为该评论颁发徽章", "Award This Comment")),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .children(awards.into_iter().map(|(kind, emoji, zh, en)| {
                    let cid = comment_id.to_string();
                    let ak = kind.to_string();
                    div()
                        .id(SharedString::from(format!("cmt-award-opt-{kind}")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(pal.card_surface_rgba())
                        .border_1()
                        .border_color(pal.hairline_border_rgba())
                        .hover(move |s| s.bg(pal.row_hover_rgba()))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.give_comment_award_action(cid.clone(), ak.clone(), cx);
                            }),
                        )
                        .child(emoji)
                        .child(tr_str(lang, zh, en))
                })),
        )
        .into_any_element()
}

/// Renders the 1-click "Summon Bot" picker popover listing Auto-select + available Bots + prompt.
pub fn render_summon_bot_picker_popover(
    ws: &CommunityWorkspace,
    post_id: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let bots = collect_available_bots(ws);
    let selected_bot = ws.state.detail_summon_bot_id.clone();
    let prompt_focused = ws.state.active_input == ActiveCommunityInput::DetailSummonPrompt;
    let pid = post_id.to_string();
    let hover_bg = pal.row_hover_rgba();

    macos_popover_surface("community-summon-bot-popover", pal)
        .w(px(330.0))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(tg_icon(IconName::BotRobot, 15.0, pal.accent))
                .child(tr_str(lang, "选择要召唤的 Bot 公民", "Choose a Bot to Summon")),
        )
        .child(
            div()
                .id("summon-bot-opt-auto")
                .cursor_pointer()
                .px(px(9.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(if selected_bot.is_none() {
                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                } else {
                    pal.card_surface_rgba()
                })
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(12.0))
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.detail_summon_bot_id = None;
                        cx.notify();
                    }),
                )
                .child(tr_str(lang, "✨ 自动选择最匹配该话题的 Bot", "✨ Auto-select best Bot"))
                .when(selected_bot.is_none(), |r| r.child(tg_icon(IconName::SelectCheck, 13.0, pal.accent))),
        )
        .child(
            div()
                .id("summon-bot-list-scroll")
                .max_h(px(180.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .children(bots.into_iter().map(|bot| {
                    let is_sel = selected_bot.as_deref() == Some(bot.actor_id.as_str());
                    let bid = bot.actor_id.clone();
                    div()
                        .id(SharedString::from(format!("summon-bot-opt-{}", bot.actor_id)))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(if is_sel {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                        } else {
                            pal.card_surface_rgba()
                        })
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.detail_summon_bot_id = Some(bid.clone());
                                cx.notify();
                            }),
                        )
                        .child(render_actor_avatar(
                            &bot.actor_id,
                            &bot.display_name,
                            bot.actor_kind,
                            bot.avatar_media_id.as_deref(),
                            &bot.avatar_shape,
                            &bot.avatar_color,
                            24.0,
                            pal.menu_bg,
                            &ws.media_base_url,
                            true,
                            &pal,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(bot.display_name),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("@{} · {:?}", bot.handle, bot.bot_autonomy_mode)),
                                ),
                        )
                        .when(is_sel, |r| r.child(tg_icon(IconName::SelectCheck, 13.0, pal.accent)))
                })),
        )
        .child(
            div()
                .id("summon-bot-prompt-input")
                .cursor_text()
                .w_full()
                .px(px(8.0))
                .py(px(7.0))
                .rounded(px(8.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if prompt_focused {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.focus_input(ActiveCommunityInput::DetailSummonPrompt);
                        cx.notify();
                    }),
                )
                .child(render_input_display(
                    &ws.state.detail_summon_prompt,
                    if prompt_focused { ws.state.ime_preedit.as_deref() } else { None },
                    tr_str(lang, "可选引导提示词（例如：对比性能利弊）…", "Optional prompt (e.g. compare tradeoffs)…"),
                    prompt_focused,
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
                    div()
                        .id("summon-bot-cancel")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .bg(pal.card_surface_rgba())
                        .text_size(px(11.5))
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.active_popover = None;
                                if this.state.active_input == ActiveCommunityInput::DetailSummonPrompt {
                                    this.state.blur_input();
                                }
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "取消", "Cancel")),
                )
                .child(
                    div()
                        .id("summon-bot-confirm")
                        .cursor_pointer()
                        .px(px(13.0))
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .bg(rgb(pal.accent))
                        .hover(move |s| s.bg(rgb(pal.accent_hover)))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let bid = this.state.detail_summon_bot_id.clone();
                                let prompt = Some(this.state.detail_summon_prompt.trim().to_string())
                                    .filter(|s| !s.is_empty());
                                let parent = this.state.detail_reply_parent_id.clone();
                                this.state.detail_summon_prompt.clear();
                                this.summon_bot_with_options(pid.clone(), bid, prompt, parent, cx);
                            }),
                        )
                        .child(tr_str(lang, "立即召唤", "Summon Now")),
                ),
        )
        .into_any_element()
}
