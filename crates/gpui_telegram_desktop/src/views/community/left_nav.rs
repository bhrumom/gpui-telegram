//! 220px Community Left Sub-Navigation Rail & Narrow-Viewport Compact Top Nav Bar.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::CommunityRuntimeMode;
use gpui_telegram_proto::community_contracts::{ActorKind, FeedTab};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

use super::shared_widgets::{format_badge_count, render_actor_avatar, render_actor_badge_row};
use super::state::{CommunityPopover, CommunityRoute};
use super::workspace::CommunityWorkspace;

fn render_nav_item(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    is_active: bool,
    badge: u32,
    pal: &TdesktopThemePalette,
    cx: &mut Context<CommunityWorkspace>,
    on_click: impl Fn(&mut CommunityWorkspace, &mut Context<CommunityWorkspace>) + 'static,
) -> Stateful<Div> {
    let hover_bg = pal.row_hover_rgba();
    let active_bg = TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14);
    let fg = if is_active { pal.accent } else { pal.text_primary };
    let ic = if is_active { pal.accent } else { pal.text_secondary };

    div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .h(px(34.0))
        .px(px(10.0))
        .rounded(px(9.0))
        .when(is_active, |el| el.bg(active_bg))
        .hover(move |s| if is_active { s } else { s.bg(hover_bg) })
        .flex()
        .items_center()
        .justify_between()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_click(this, cx);
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .min_w(px(0.0))
                .child(tg_icon(icon, 16.0, ic))
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.0))
                        .font_weight(if is_active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(rgb(fg))
                        .child(label),
                ),
        )
        .when(badge > 0, |el| {
            el.child(
                div()
                    .min_w(px(18.0))
                    .h(px(18.0))
                    .px(px(5.0))
                    .rounded_full()
                    .bg(rgb(pal.unread_badge_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(pal.unread_badge_fg))
                    .child(format_badge_count(badge)),
            )
        })
}

/// Renders the 220px Community Sub-Nav (used in 3-column and 2-column modes).
pub fn render_left_nav(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let route = ws.state.route.clone();
    let unread = ws.state.unread_notifications;
    let joined_communities: Vec<_> = ws
        .state
        .communities
        .iter()
        .filter(|c| c.is_joined)
        .take(12)
        .cloned()
        .collect();
    let pinned_lists: Vec<_> = ws
        .state
        .lists
        .iter()
        .filter(|l| l.is_pinned)
        .take(8)
        .cloned()
        .collect();
    let active_actor = ws.state.active_actor().cloned();
    let is_online = ws.state.runtime_mode == CommunityRuntimeMode::Online;
    let base_url = ws.media_base_url.clone();
    let hover_bg = pal.row_hover_rgba();

    let nav_w = pal.scaled_px(220.0).clamp(192.0, 260.0);

    div()
        .id("community-left-nav")
        .w(px(nav_w))
        .flex_shrink_0()
        .h_full()
        .bg(rgb(pal.dialogs_bg))
        .border_r_1()
        .border_color(pal.separator_rgba())
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .p(px(10.0))
                .gap(px(4.0))
                // Brand header
                .child(
                    div()
                        .px(px(6.0))
                        .pb(px(6.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.0))
                                .child(tg_icon(IconName::CommunityGlobe, 18.0, pal.accent))
                                .child(
                                    div()
                                        .text_size(px(14.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(lang, "社区广场", "Community")),
                                ),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .text_size(px(9.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.accent))
                                .child("Reddit × X"),
                        ),
                )
                // Primary nav items
                .child(render_nav_item(
                    "community-nav-feed",
                    IconName::CommunityGlobe,
                    tr_str(lang, "信息流", "Feed"),
                    matches!(route, CommunityRoute::Feed),
                    0,
                    &pal,
                    cx,
                    |this, cx| this.replace_route(CommunityRoute::Feed, cx),
                ))
                .child(render_nav_item(
                    "community-nav-explore",
                    IconName::FlameTrending,
                    tr_str(lang, "探索与热榜", "Explore"),
                    matches!(route, CommunityRoute::Explore { .. }),
                    0,
                    &pal,
                    cx,
                    |this, cx| this.navigate_to(CommunityRoute::Explore { initial_query: None }, cx),
                ))
                .child(render_nav_item(
                    "community-nav-notifications",
                    IconName::ChannelMegaphone,
                    tr_str(lang, "互动通知", "Notifications"),
                    matches!(route, CommunityRoute::Notifications),
                    unread,
                    &pal,
                    cx,
                    |this, cx| this.navigate_to(CommunityRoute::Notifications, cx),
                ))
                .child(render_nav_item(
                    "community-nav-bookmarks",
                    IconName::Bookmark,
                    tr_str(lang, "收藏与列表", "Bookmarks & Lists"),
                    matches!(route, CommunityRoute::BookmarksLists),
                    0,
                    &pal,
                    cx,
                    |this, cx| this.navigate_to(CommunityRoute::BookmarksLists, cx),
                ))
                .child(render_nav_item(
                    "community-nav-mod",
                    IconName::ShieldMod,
                    tr_str(lang, "审核台", "Moderation"),
                    matches!(route, CommunityRoute::ModQueue(_)),
                    0,
                    &pal,
                    cx,
                    |this, cx| this.navigate_to(CommunityRoute::ModQueue(None), cx),
                ))
                .child(render_nav_item(
                    "community-nav-bot-studio",
                    IconName::BotRobot,
                    tr_str(lang, "Bot 工作室", "Bot Studio"),
                    matches!(route, CommunityRoute::BotStudio),
                    0,
                    &pal,
                    cx,
                    |this, cx| this.navigate_to(CommunityRoute::BotStudio, cx),
                ))
                // Primary "New Post" CTA
                .child(
                    div()
                        .id("community-new-post-cta")
                        .cursor_pointer()
                        .mt(px(6.0))
                        .mb(px(4.0))
                        .h(px(34.0))
                        .rounded_full()
                        .bg(rgb(pal.accent))
                        .hover(move |s| s.bg(rgb(pal.accent_hover)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                let cur_slug = match &this.state.route {
                                    CommunityRoute::CommunityPage(s) => Some(s.clone()),
                                    _ => None,
                                };
                                this.open_composer_modal(cur_slug, cx);
                            }),
                        )
                        .child(tg_icon(IconName::ComposePen, 14.0, pal.active_button_fg))
                        .child(tr_str(lang, "发布新帖", "New Post")),
                )
                // Scrollable Joined Communities + Pinned Lists
                .child(
                    div()
                        .id("community-left-nav-scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .track_scroll(&ws.left_nav_scroll_handle)
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .pt(px(4.0))
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(4.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_secondary))
                                        .child(tr_str(lang, "已加入的社区 (c/)", "MY COMMUNITIES")),
                                )
                                .child(
                                    div()
                                        .id("left-nav-create-community-btn")
                                        .cursor_pointer()
                                        .px(px(6.0))
                                        .py(px(1.5))
                                        .rounded(px(5.0))
                                        .bg(pal.service_pill_rgba())
                                        .hover(move |s| s.bg(hover_bg))
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.accent))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.open_create_community_modal(cx);
                                            }),
                                        )
                                        .child(tr_str(lang, "+ 新建", "+ New")),
                                ),
                        )
                        .when(!joined_communities.is_empty(), |col| {
                            col.children(joined_communities.into_iter().map(|c| {
                                let slug = c.slug.clone();
                                let row_id = SharedString::from(format!("left-nav-comm-{}", c.slug));
                                let active = route == CommunityRoute::CommunityPage(c.slug.clone());
                                div()
                                    .id(row_id)
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(7.0))
                                    .when(active, |el| {
                                        el.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                    })
                                    .hover(move |s| s.bg(hover_bg))
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.navigate_to(
                                                CommunityRoute::CommunityPage(slug.clone()),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .child(if c.icon_emoji.trim().is_empty() {
                                                "🌐".to_string()
                                            } else {
                                                c.icon_emoji.clone()
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(px(12.0))
                                            .text_color(rgb(if active { pal.accent } else { pal.text_primary }))
                                            .child(format!("c/{}", c.slug)),
                                    )
                            }))
                        })
                        .when(!pinned_lists.is_empty(), |col| {
                            col.child(
                                div()
                                    .px(px(8.0))
                                    .pt(px(8.0))
                                    .pb(px(4.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(lang, "置顶列表", "PINNED LISTS")),
                                    )
                                    .child(
                                        div()
                                            .id("left-nav-manage-lists-btn")
                                            .cursor_pointer()
                                            .px(px(6.0))
                                            .py(px(1.5))
                                            .rounded(px(5.0))
                                            .bg(pal.service_pill_rgba())
                                            .hover(move |s| s.bg(hover_bg))
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.navigate_to(
                                                        CommunityRoute::BookmarksLists,
                                                        cx,
                                                    );
                                                    this.open_create_list_form(cx);
                                                }),
                                            )
                                            .child(tr_str(lang, "+ 新建", "+ New")),
                                    ),
                            )
                            .children(pinned_lists.into_iter().map(|list| {
                                let lid = list.list_id.clone();
                                let row_id = SharedString::from(format!("left-nav-list-{}", list.list_id));
                                let active = route == CommunityRoute::Feed
                                    && ws.state.active_tab == FeedTab::List(list.list_id.clone());
                                div()
                                    .id(row_id)
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(7.0))
                                    .when(active, |el| {
                                        el.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                    })
                                    .hover(move |s| s.bg(hover_bg))
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.select_feed_tab(FeedTab::List(lid.clone()), cx);
                                        }),
                                    )
                                    .child(tg_icon(IconName::Bookmark, 12.0, pal.text_secondary))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(px(12.0))
                                            .text_color(rgb(if active { pal.accent } else { pal.text_primary }))
                                            .child(list.name.clone()),
                                    )
                            }))
                        }),
                ),
        )
        // Bottom sticky Active Human/Bot Identity Switcher + Online/Local status
        .child(render_left_nav_footer(active_actor, is_online, &base_url, &pal, lang, cx))
        .into_any_element()
}

fn render_left_nav_footer(
    active_actor: Option<gpui_telegram_proto::community_contracts::CommunityActorDto>,
    is_online: bool,
    base_url: &str,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let hover_bg = pal.row_hover_rgba();
    let status_dot_color = if is_online { pal.success } else { 0xF59E0B };
    let status_label = if is_online {
        tr_str(lang, "在线同步", "Online")
    } else {
        tr_str(lang, "本地离线", "Local Mode")
    };

    let (aid, dname, handle, kind, avatar_id, shape, color, vtier, bbadge) = match active_actor {
        Some(a) => (
            a.actor_id,
            a.display_name,
            a.handle,
            a.actor_kind,
            a.avatar_media_id,
            a.avatar_shape,
            a.avatar_color,
            a.verified_tier,
            a.bot_badge,
        ),
        None => (
            "u:local".to_string(),
            tr_str(lang, "当前身份", "Active Identity").to_string(),
            "citizen".to_string(),
            ActorKind::Human,
            None,
            "classic".to_string(),
            String::new(),
            gpui_telegram_proto::community_contracts::VerifiedTier::None,
            None,
        ),
    };

    div()
        .w_full()
        .flex_shrink_0()
        .p(px(10.0))
        .border_t_1()
        .border_color(pal.separator_rgba())
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .id("community-act-as-btn")
                .cursor_pointer()
                .w_full()
                .px(px(8.0))
                .py(px(7.0))
                .rounded(px(10.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
                    }),
                )
                .child(render_actor_avatar(
                    &aid,
                    &dname,
                    kind,
                    avatar_id.as_deref(),
                    &shape,
                    &color,
                    28.0,
                    pal.dialogs_bg,
                    base_url,
                    true,
                    pal,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(dname),
                                )
                                .child(render_actor_badge_row(
                                    kind,
                                    vtier,
                                    bbadge.as_deref(),
                                    false,
                                    false,
                                    &color,
                                    pal,
                                    lang,
                                )),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("@{handle}")),
                        ),
                )
                .child(tg_icon(IconName::ChevronDown, 12.0, pal.text_secondary)),
        )
        .child(
            div()
                .px(px(4.0))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgb(status_dot_color)))
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(status_label),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(pal.text_secondary))
                        .child("⌘⇧C"),
                ),
        )
}

/// Compact horizontal navigation strip rendered at the top in 1-column narrow viewports (`<780px`).
pub fn render_compact_top_nav(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let route = ws.state.route.clone();
    let unread = ws.state.unread_notifications;
    let active_actor = ws.state.active_actor().cloned();
    let base_url = ws.media_base_url.clone();
    let hover_bg = pal.row_hover_rgba();

    let chip = |id: &'static str,
                icon: IconName,
                label: &'static str,
                active: bool,
                badge: u32,
                on_click: Box<dyn Fn(&mut CommunityWorkspace, &mut Context<CommunityWorkspace>)>| {
        div()
            .id(id)
            .cursor_pointer()
            .px(px(9.0))
            .py(px(5.0))
            .rounded_full()
            .when(active, |el| {
                el.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
            })
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .gap(px(5.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    on_click(this, cx);
                }),
            )
            .child(tg_icon(
                icon,
                14.0,
                if active { pal.accent } else { pal.text_secondary },
            ))
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                    .text_color(rgb(if active { pal.accent } else { pal.text_primary }))
                    .child(label),
            )
            .when(badge > 0, |el| {
                el.child(
                    div()
                        .px(px(4.0))
                        .rounded_full()
                        .bg(rgb(pal.unread_badge_bg))
                        .text_size(px(9.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.unread_badge_fg))
                        .child(format_badge_count(badge)),
                )
            })
    };

    div()
        .id("community-compact-top-nav")
        .w_full()
        .flex_shrink_0()
        .px(px(10.0))
        .py(px(7.0))
        .bg(rgb(pal.dialogs_bg))
        .border_b_1()
        .border_color(pal.separator_rgba())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(4.0))
                .child(chip(
                    "compact-nav-feed",
                    IconName::CommunityGlobe,
                    tr_str(lang, "信息流", "Feed"),
                    matches!(route, CommunityRoute::Feed),
                    0,
                    Box::new(|this, cx| this.replace_route(CommunityRoute::Feed, cx)),
                ))
                .child(chip(
                    "compact-nav-explore",
                    IconName::FlameTrending,
                    tr_str(lang, "探索", "Explore"),
                    matches!(route, CommunityRoute::Explore { .. }),
                    0,
                    Box::new(|this, cx| {
                        this.navigate_to(CommunityRoute::Explore { initial_query: None }, cx)
                    }),
                ))
                .child(chip(
                    "compact-nav-notifs",
                    IconName::ChannelMegaphone,
                    tr_str(lang, "通知", "Notifs"),
                    matches!(route, CommunityRoute::Notifications),
                    unread,
                    Box::new(|this, cx| this.navigate_to(CommunityRoute::Notifications, cx)),
                ))
                .child(chip(
                    "compact-nav-bookmarks",
                    IconName::Bookmark,
                    tr_str(lang, "收藏", "Saved"),
                    matches!(route, CommunityRoute::BookmarksLists),
                    0,
                    Box::new(|this, cx| this.navigate_to(CommunityRoute::BookmarksLists, cx)),
                ))
                .child(chip(
                    "compact-nav-mod",
                    IconName::ShieldMod,
                    tr_str(lang, "版务", "Mod"),
                    matches!(route, CommunityRoute::ModQueue(_)),
                    0,
                    Box::new(|this, cx| this.navigate_to(CommunityRoute::ModQueue(None), cx)),
                ))
                .child(chip(
                    "compact-nav-bot-studio",
                    IconName::BotRobot,
                    tr_str(lang, "Bot", "Bots"),
                    matches!(route, CommunityRoute::BotStudio),
                    0,
                    Box::new(|this, cx| this.navigate_to(CommunityRoute::BotStudio, cx)),
                ))
                .child(
                    div()
                        .id("compact-new-post-btn")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(5.0))
                        .rounded_full()
                        .bg(rgb(pal.accent))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                let cur_slug = match &this.state.route {
                                    CommunityRoute::CommunityPage(s) => Some(s.clone()),
                                    _ => None,
                                };
                                this.open_composer_modal(cur_slug, cx);
                            }),
                        )
                        .child(tg_icon(IconName::ComposePen, 12.0, pal.active_button_fg))
                        .child(tr_str(lang, "发帖", "Post")),
                ),
        )
        .child(
            div()
                .id("compact-act-as-btn")
                .cursor_pointer()
                .px(px(6.0))
                .py(px(3.0))
                .rounded_full()
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
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
                .child(match active_actor {
                    Some(a) => render_actor_avatar(
                        &a.actor_id,
                        &a.display_name,
                        a.actor_kind,
                        a.avatar_media_id.as_deref(),
                        &a.avatar_shape,
                        &a.avatar_color,
                        22.0,
                        pal.dialogs_bg,
                        &base_url,
                        true,
                        &pal,
                    ),
                    None => render_actor_avatar(
                        "u:local",
                        "U",
                        ActorKind::Human,
                        None,
                        "classic",
                        "",
                        22.0,
                        pal.dialogs_bg,
                        &base_url,
                        true,
                        &pal,
                    ),
                })
                .child(tg_icon(IconName::ChevronDown, 10.0, pal.text_secondary)),
        )
        .into_any_element()
}
