//! Feature F20-Notifications — Categorized Community Notifications (All 13 Kinds),
//! Per-Category Unread Badges, Single & Bulk Mark-Read, and Click-to-Jump Routing.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_community::extract_entities;
use gpui_telegram_proto::community_contracts::{
    CommunityNotificationDto, CommunityNotificationKind, MarkNotificationsReadRequest,
    VerifiedTier,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

use super::shared_widgets::{
    format_badge_count, format_relative_time, render_actor_avatar, render_actor_badge_row,
    render_empty_state, render_loading_skeleton,
};
use super::state::{CommunityRoute, CommunityWorkspaceAction, NotificationCategoryTab};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

fn category_label(tab: NotificationCategoryTab, lang: &str) -> &'static str {
    match tab {
        NotificationCategoryTab::All => tr_str(lang, "全部", "All"),
        NotificationCategoryTab::Mentions => tr_str(lang, "@提及", "Mentions"),
        NotificationCategoryTab::Replies => tr_str(lang, "回复", "Replies"),
        NotificationCategoryTab::VotesLikes => tr_str(lang, "赞同与喜欢", "Votes & Likes"),
        NotificationCategoryTab::RepostsQuotes => tr_str(lang, "转发与引用", "Reposts & Quotes"),
        NotificationCategoryTab::Follows => tr_str(lang, "关注", "Follows"),
        NotificationCategoryTab::BotActivity => tr_str(lang, "Bot 动态", "Bot Activity"),
        NotificationCategoryTab::Moderation => tr_str(lang, "审核与系统", "Moderation"),
    }
}

fn notif_kind_meta(kind: CommunityNotificationKind, pal: &TdesktopThemePalette, lang: &str) -> (IconName, u32, &'static str) {
    match kind {
        CommunityNotificationKind::Mention => {
            (IconName::BotRobot, pal.accent, tr_str(lang, "提及了你", "mentioned you"))
        }
        CommunityNotificationKind::Reply => {
            (IconName::Reply, pal.accent, tr_str(lang, "回复了你", "replied to you"))
        }
        CommunityNotificationKind::Like => {
            (IconName::HeartReaction, 0xEC4899, tr_str(lang, "喜欢了你的内容", "liked your post"))
        }
        CommunityNotificationKind::UpvoteMilestone => (
            IconName::UpvoteArrow,
            0xFF4500,
            tr_str(lang, "达到赞同里程碑", "hit an upvote milestone"),
        ),
        CommunityNotificationKind::Quote => {
            (IconName::Repost, 0x10B981, tr_str(lang, "引用了你的帖子", "quoted your post"))
        }
        CommunityNotificationKind::Repost => {
            (IconName::Repost, 0x10B981, tr_str(lang, "转发了你的帖子", "reposted your post"))
        }
        CommunityNotificationKind::Follow => {
            (IconName::ContactsUsers, pal.accent, tr_str(lang, "关注了你", "followed you"))
        }
        CommunityNotificationKind::Award => (
            IconName::CommunityAward,
            0xF59E0B,
            tr_str(lang, "打赏了你的内容", "awarded your content"),
        ),
        CommunityNotificationKind::ModAction => (
            IconName::ShieldMod,
            0x10B981,
            tr_str(lang, "执行了社区审核操作", "took a moderation action"),
        ),
        CommunityNotificationKind::BotSummon => (
            IconName::BotRobot,
            0x8B5CF6,
            tr_str(lang, "召唤了 Bot 参与讨论", "summoned a Bot"),
        ),
        CommunityNotificationKind::BotReply => (
            IconName::BotRobot,
            0x8B5CF6,
            tr_str(lang, "Bot 回复了你的讨论", "replied as a Bot"),
        ),
        CommunityNotificationKind::PollEnded => (
            IconName::Poll,
            0x3B82F6,
            tr_str(lang, "投票已结束", "poll has ended"),
        ),
        CommunityNotificationKind::System => (
            IconName::ChannelMegaphone,
            pal.text_secondary,
            tr_str(lang, "系统通知", "system update"),
        ),
    }
}

impl CommunityWorkspace {
    pub fn set_notification_category(
        &mut self,
        category: NotificationCategoryTab,
        cx: &mut Context<Self>,
    ) {
        if self.state.notif_category != category {
            self.state.notif_category = category;
            cx.notify();
        }
    }

    pub fn toggle_notifications_unread_only(&mut self, cx: &mut Context<Self>) {
        self.state.notif_unread_only = !self.state.notif_unread_only;
        cx.notify();
    }

    /// Marks a single notification read via `facade.mark_notifications_read`, updating local state
    /// immediately and keeping sidebar/rail unread badges synchronized.
    pub fn mark_single_notification_read(&mut self, notif_id: String, cx: &mut Context<Self>) {
        let mut was_unread = false;
        for n in &mut self.state.notifications {
            if n.notif_id == notif_id && !n.is_read {
                n.is_read = true;
                was_unread = true;
            }
        }
        if was_unread {
            self.state.unread_notifications = self.state.unread_notifications.saturating_sub(1);
            let next_unread = self.state.unread_notifications;
            self.emit_action(CommunityWorkspaceAction::UnreadCountChanged(next_unread), cx);
        }
        self.state.notifications_gen = self.state.notifications_gen.wrapping_add(1);
        let my_gen = self.state.notifications_gen;
        self.state.notifications_loading = false;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                facade
                    .mark_notifications_read(&MarkNotificationsReadRequest {
                        notif_ids: Some(vec![notif_id]),
                    })
                    .await
            })
            .await;
            if let Ok(Ok(resp)) = res {
                let _ = weak.update(app, |this, cx| {
                    if this.state.notifications_gen != my_gen {
                        return;
                    }
                    this.state.notifications_loading = false;
                    let unread = resp.unread_count.min(u32::MAX as u64) as u32;
                    this.state.unread_notifications = unread;
                    this.emit_action(CommunityWorkspaceAction::UnreadCountChanged(unread), cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Marks an unread notification read and jumps directly to its target post/comment,
    /// community `c/<slug>`, or `@handle` actor profile.
    pub fn open_notification_target(
        &mut self,
        notif: CommunityNotificationDto,
        cx: &mut Context<Self>,
    ) {
        if !notif.is_read {
            self.mark_single_notification_read(notif.notif_id.clone(), cx);
        }
        if let Some(pid) = notif
            .target_post_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.state.detail_focused_comment_id = notif
                .target_comment_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string);
            self.open_post_detail(pid.to_string(), cx);
            return;
        }
        let entities = extract_entities(&notif.preview_text);
        if let Some(slug) = entities.communities.first() {
            self.navigate_to(CommunityRoute::CommunityPage(slug.clone()), cx);
            return;
        }
        let handle = notif.actor_handle.trim();
        if !handle.is_empty() {
            self.open_actor_profile(handle.to_string(), cx);
            return;
        }
        let actor_id = notif.actor_id.trim();
        if !actor_id.is_empty() {
            self.open_actor_profile(actor_id.to_string(), cx);
        }
    }
}

/// Renders the full F20 Categorized Community Notifications view.
pub fn render_notifications_view(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active_cat = ws.state.notif_category;
    let unread_only = ws.state.notif_unread_only;
    let total_unread = ws.state.unread_notifications;
    let all_notifs = &ws.state.notifications;
    let filtered: Vec<CommunityNotificationDto> = all_notifs
        .iter()
        .filter(|n| active_cat.matches(n.kind) && (!unread_only || !n.is_read))
        .cloned()
        .collect();
    let subtitle = if lang.starts_with("zh") {
        format!("{total_unread} 条未读通知")
    } else {
        format!("{total_unread} unread")
    };

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(
            tr_str(lang, "社区通知", "Notifications").to_string(),
            Some(subtitle),
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
                        .max_w(px(760.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        // Top bar: Unread badge + Unread-only toggle + Refresh + Mark All Read
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .px(px(9.0))
                                                .py(px(3.5))
                                                .rounded_full()
                                                .bg(if total_unread > 0 {
                                                    rgb(pal.unread_badge_bg).into()
                                                } else {
                                                    pal.service_pill_rgba()
                                                })
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(if total_unread > 0 {
                                                    pal.unread_badge_fg
                                                } else {
                                                    pal.text_secondary
                                                }))
                                                .child(format!(
                                                    "{} {}",
                                                    format_badge_count(total_unread),
                                                    tr_str(lang, "未读", "unread")
                                                )),
                                        )
                                        .child(
                                            div()
                                                .id("notifs-unread-only-toggle")
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(4.0))
                                                .rounded_full()
                                                .bg(if unread_only {
                                                    TdesktopThemePalette::rgba_with_alpha(
                                                        pal.accent, 0.16,
                                                    )
                                                } else {
                                                    pal.service_pill_rgba()
                                                })
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(if unread_only {
                                                    pal.accent
                                                } else {
                                                    pal.text_secondary
                                                }))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.toggle_notifications_unread_only(cx);
                                                    }),
                                                )
                                                .child(tr_str(lang, "仅看未读", "Unread only")),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .id("notifs-refresh-btn")
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(5.0))
                                                .rounded_full()
                                                .bg(pal.service_pill_rgba())
                                                .text_size(px(11.5))
                                                .text_color(rgb(pal.text_secondary))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.load_notifications(cx);
                                                    }),
                                                )
                                                .child(tr_str(lang, "刷新", "Refresh")),
                                        )
                                        .child(
                                            div()
                                                .id("notifs-mark-all-read-btn")
                                                .cursor_pointer()
                                                .px(px(12.0))
                                                .py(px(5.0))
                                                .rounded_full()
                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                    pal.accent, 0.14,
                                                ))
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
                                                .child(tr_str(lang, "全部标为已读", "Mark All Read")),
                                        ),
                                ),
                        )
                        // Category Filter Tabs with live per-category unread counts
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(5.0))
                                .children(NotificationCategoryTab::ALL_TABS.into_iter().map(
                                    |cat| {
                                        let active = active_cat == cat;
                                        let unread_in_cat = all_notifs
                                            .iter()
                                            .filter(|n| cat.matches(n.kind) && !n.is_read)
                                            .count()
                                            as u32;
                                        div()
                                            .id(SharedString::from(format!("notif-cat-{cat:?}")))
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(4.5))
                                            .rounded_full()
                                            .bg(if active {
                                                rgb(pal.accent).into()
                                            } else {
                                                pal.service_pill_rgba()
                                            })
                                            .flex()
                                            .items_center()
                                            .gap(px(5.0))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if active {
                                                pal.active_button_fg
                                            } else {
                                                pal.text_secondary
                                            }))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.set_notification_category(cat, cx);
                                                }),
                                            )
                                            .child(category_label(cat, lang))
                                            .when(unread_in_cat > 0, |pill| {
                                                pill.child(
                                                    div()
                                                        .px(px(5.0))
                                                        .py(px(1.0))
                                                        .rounded_full()
                                                        .bg(if active {
                                                            TdesktopThemePalette::rgba_with_alpha(
                                                                0xFFFFFF, 0.24,
                                                            )
                                                        } else {
                                                            rgb(pal.unread_badge_bg).into()
                                                        })
                                                        .text_size(px(9.5))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(if active {
                                                            pal.active_button_fg
                                                        } else {
                                                            pal.unread_badge_fg
                                                        }))
                                                        .child(format_badge_count(unread_in_cat)),
                                                )
                                            })
                                    },
                                )),
                        )
                        // Notifications Feed
                        .when(ws.state.notifications_loading && filtered.is_empty(), |col| {
                            col.child(render_loading_skeleton(4, &pal))
                        })
                        .when(!ws.state.notifications_loading && filtered.is_empty(), |col| {
                            col.child(render_empty_state(
                                IconName::ChannelMegaphone,
                                tr_str(lang, "该分类暂无通知", "No notifications here"),
                                tr_str(
                                    lang,
                                    "当有人类或 Bot 回复、@提及、投票、打赏或执行审核时，通知会实时显示在这里。",
                                    "Mentions, replies, votes, reposts, follows, bot replies, and moderation updates will appear here.",
                                ),
                                &pal,
                            ))
                        })
                        .children(filtered.into_iter().map(|n| {
                            render_notification_row(n, &ws.media_base_url, &pal, lang, cx)
                        })),
                ),
        )
        .into_any_element()
}

fn render_notification_row(
    notif: CommunityNotificationDto,
    base_url: &str,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let (icon, icon_color, verb) = notif_kind_meta(notif.kind, pal, lang);
    let jump_notif = notif.clone();
    let mark_id = notif.notif_id.clone();
    let is_read = notif.is_read;
    let time_str = format_relative_time(notif.created_at_ms, lang);
    let avatar_shape = if notif.actor_kind.is_bot_like() {
        "blob"
    } else {
        "classic"
    };
    let avatar_color = if notif.actor_kind.is_bot_like() {
        "violet"
    } else {
        "blue"
    };

    div()
        .id(SharedString::from(format!("notif-row-{}", notif.notif_id)))
        .cursor_pointer()
        .p(px(11.0))
        .rounded(px(11.0))
        .bg(if is_read {
            pal.card_surface_rgba()
        } else {
            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.10)
        })
        .border_1()
        .border_color(if is_read {
            pal.hairline_border_rgba()
        } else {
            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.30)
        })
        .hover({
            let hbg = pal.row_hover_rgba();
            move |s| s.bg(hbg)
        })
        .flex()
        .items_start()
        .gap(px(10.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.open_notification_target(jump_notif.clone(), cx);
            }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(render_actor_avatar(
                    &notif.actor_id,
                    &notif.actor_display_name,
                    notif.actor_kind,
                    None,
                    avatar_shape,
                    avatar_color,
                    34.0,
                    pal.card_bg,
                    base_url,
                    true,
                    pal,
                ))
                .child(tg_icon(icon, 12.0, icon_color)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .when(!is_read, |row| {
                            row.child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgb(pal.accent)))
                        })
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(notif.actor_display_name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("@{}", notif.actor_handle)),
                        )
                        .child(render_actor_badge_row(
                            notif.actor_kind,
                            VerifiedTier::None,
                            None,
                            false,
                            false,
                            avatar_color,
                            pal,
                            lang,
                        ))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(verb),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("· {time_str}")),
                        ),
                )
                .when(!notif.preview_text.trim().is_empty(), |col| {
                    col.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_primary))
                            .child(notif.preview_text.clone()),
                    )
                }),
        )
        .when(!is_read, |row| {
            row.child(
                div()
                    .id(SharedString::from(format!(
                        "notif-mark-read-{}",
                        notif.notif_id
                    )))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(pal.service_pill_rgba())
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.mark_single_notification_read(mark_id.clone(), cx);
                        }),
                    )
                    .child(tr_str(lang, "标为已读", "Mark Read")),
            )
        })
}
