//! Community Feed View: Sticky Tabs, ActorFilter, Reddit Sort/Time Bar, Algo Tuner Popover,
//! Live WS New-Posts Banner, IME-Wired Inline Quick-Post Box & Scrollable Post Stream.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorKind, AlgoPreferencesDto, AlgoPreset, FeedSort, FeedTab, TimeWindow,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::macos_popover_surface;

use super::post_card::render_post_card;
use super::shared_widgets::{
    render_actor_avatar, render_empty_state, render_input_display, render_loading_skeleton,
};
use super::state::{sanitize_unit_f64, ActiveCommunityInput, CommunityPopover, FeedDensity};
use super::workspace::CommunityWorkspace;

/// Renders the primary multi-tab Community Feed stream with sticky controls and inline Quick-Post.
pub fn render_feed_view(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let pending_ws = ws.state.pending_ws_new_posts;
    let visible_items: Vec<_> = ws
        .state
        .feed_items
        .iter()
        .filter(|it| !ws.state.hidden_post_ids.contains(&it.post.post_id))
        .cloned()
        .collect();
    let has_more = ws.state.next_cursor.is_some();
    let loading = ws.state.feed_loading;
    let loading_more = ws.state.feed_loading_more;
    let feed_error = ws.state.feed_error.clone();

    div()
        .id("community-feed-view")
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(render_sticky_feed_header(ws, cx))
        .child(
            div()
                .id("community-feed-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .track_scroll(&ws.feed_scroll_handle)
                .px(px(14.0))
                .py(px(12.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(740.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .when(pending_ws > 0, |col| {
                            let label = if lang.starts_with("zh") {
                                format!("有 {pending_ws} 条新动态 — 点击刷新")
                            } else {
                                format!("{pending_ws} new posts — Click to refresh")
                            };
                            col.child(
                                div()
                                    .id("community-ws-new-posts-banner")
                                    .cursor_pointer()
                                    .w_full()
                                    .py(px(8.0))
                                    .px(px(14.0))
                                    .rounded_full()
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                                    .border_1()
                                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.35))
                                    .hover(move |s| {
                                        s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.24))
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(6.0))
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.refresh_feed_from_ws_banner(cx);
                                        }),
                                    )
                                    .child(tg_icon(IconName::SparkleAlgo, 14.0, pal.accent))
                                    .child(label),
                            )
                        })
                        .child(render_inline_quick_post(ws, cx))
                        .when_some(feed_error, |col, err| {
                            col.child(
                                div()
                                    .w_full()
                                    .p(px(10.0))
                                    .rounded(px(9.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                                    .text_size(px(12.0))
                                    .text_color(rgb(pal.danger))
                                    .child(err),
                            )
                        })
                        .when(loading && visible_items.is_empty(), |col| {
                            col.child(render_loading_skeleton(4, &pal))
                        })
                        .when(!loading && visible_items.is_empty(), |col| {
                            col.child(render_empty_state(
                                IconName::CommunityGlobe,
                                tr_str(lang, "暂无符合筛选条件的帖子", "No posts match this filter yet"),
                                tr_str(
                                    lang,
                                    "试试切换其他信息流标签、放宽筛选条件，或立即发布第一条帖子！",
                                    "Try switching tabs/filters or publish the first post right above!",
                                ),
                                &pal,
                            ))
                        })
                        .children(
                            visible_items
                                .iter()
                                .enumerate()
                                .map(|(idx, item)| render_post_card(ws, item, idx, cx)),
                        )
                        .when(has_more && !visible_items.is_empty(), |col| {
                            col.child(
                                div()
                                    .id("community-feed-load-more")
                                    .cursor_pointer()
                                    .w_full()
                                    .py(px(9.0))
                                    .rounded(px(10.0))
                                    .bg(pal.card_surface_rgba())
                                    .border_1()
                                    .border_color(pal.hairline_border_rgba())
                                    .hover(move |s| s.bg(pal.row_hover_rgba()))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.load_more_feed(cx);
                                        }),
                                    )
                                    .child(if loading_more {
                                        tr_str(lang, "正在加载更多…", "Loading more…")
                                    } else {
                                        tr_str(lang, "加载更多帖子", "Load More Posts")
                                    }),
                            )
                        }),
                ),
        )
        .into_any_element()
}

fn render_sticky_feed_header(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active_tab = ws.state.active_tab.clone();
    let actor_filter = ws.state.actor_filter;
    let sort = ws.state.sort;
    let time_window = ws.state.time_window;
    let density = ws.state.density;
    let first_comm_slug = ws
        .state
        .communities
        .iter()
        .find(|c| c.is_joined)
        .or_else(|| ws.state.communities.first())
        .map(|c| c.slug.clone())
        .unwrap_or_else(|| "general".to_string());
    let pinned_lists: Vec<_> = ws
        .state
        .lists
        .iter()
        .filter(|l| l.is_pinned)
        .take(3)
        .cloned()
        .collect();
    let hover_bg = pal.row_hover_rgba();

    let tab_btn = |id: SharedString, label: String, active: bool, target: FeedTab| {
        div()
            .id(id)
            .cursor_pointer()
            .px(px(11.0))
            .py(px(7.0))
            .border_b_2()
            .border_color(if active {
                rgb(pal.accent).into()
            } else {
                Rgba::default()
            })
            .hover(move |s| s.bg(hover_bg))
            .text_size(px(12.5))
            .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
            .text_color(rgb(if active { pal.accent } else { pal.text_secondary }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.select_feed_tab(target.clone(), cx);
                }),
            )
            .child(label)
    };

    let actor_chips: [(ActorFilter, &'static str, &'static str, &'static str); 4] = [
        (ActorFilter::AllCitizens, "all", "全部公民", "All Citizens"),
        (ActorFilter::HumansOnly, "humans", "仅人类", "Humans"),
        (ActorFilter::BotsOnly, "bots", "仅 Bot", "Bots"),
        (ActorFilter::HumanBotDebates, "debates", "人机辩论", "Debates"),
    ];

    let sort_chips: [(FeedSort, &'static str, &'static str, &'static str); 7] = [
        (FeedSort::Algo, "algo", "智能", "Algo"),
        (FeedSort::Hot, "hot", "热门", "Hot"),
        (FeedSort::Best, "best", "精选", "Best"),
        (FeedSort::Top, "top", "高赞", "Top"),
        (FeedSort::New, "new", "最新", "New"),
        (FeedSort::Controversial, "cont", "争议", "Controversial"),
        (FeedSort::Rising, "rising", "飙升", "Rising"),
    ];

    let time_chips: [(TimeWindow, &'static str, &'static str); 6] = [
        (TimeWindow::Hour, "1H", "1h"),
        (TimeWindow::Day, "24H", "24h"),
        (TimeWindow::Week, "周", "7d"),
        (TimeWindow::Month, "月", "30d"),
        (TimeWindow::Year, "年", "1y"),
        (TimeWindow::All, "全部", "All"),
    ];

    div()
        .w_full()
        .flex_shrink_0()
        .bg(rgb(pal.dialogs_bg))
        .border_b_1()
        .border_color(pal.separator_rgba())
        .px(px(14.0))
        .pt(px(6.0))
        .pb(px(8.0))
        .flex()
        .flex_col()
        .gap(px(7.0))
        // Row 1: Primary feed tabs
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(2.0))
                .border_b_1()
                .border_color(pal.hairline_border_rgba())
                .child(tab_btn(
                    "community-tab-for-you".into(),
                    tr_str(lang, "为你推荐", "For You").to_string(),
                    active_tab == FeedTab::ForYou,
                    FeedTab::ForYou,
                ))
                .child(tab_btn(
                    "community-tab-following".into(),
                    tr_str(lang, "正在关注", "Following").to_string(),
                    active_tab == FeedTab::Following,
                    FeedTab::Following,
                ))
                .child(tab_btn(
                    "community-tab-popular".into(),
                    tr_str(lang, "全站热议", "Popular").to_string(),
                    active_tab == FeedTab::Popular,
                    FeedTab::Popular,
                ))
                .child(tab_btn(
                    "community-tab-all".into(),
                    tr_str(lang, "全部广场", "All").to_string(),
                    active_tab == FeedTab::All,
                    FeedTab::All,
                ))
                .child(tab_btn(
                    "community-tab-communities".into(),
                    tr_str(lang, "社区", "Communities").to_string(),
                    matches!(active_tab, FeedTab::Community(_)),
                    FeedTab::Community(first_comm_slug),
                ))
                .children(pinned_lists.into_iter().map(|list| {
                    let is_act = active_tab == FeedTab::List(list.list_id.clone());
                    tab_btn(
                        SharedString::from(format!("community-tab-list-{}", list.list_id)),
                        list.name,
                        is_act,
                        FeedTab::List(list.list_id),
                    )
                })),
        )
        // Row 2: ActorFilter + Sort + TimeWindow + Algo Tuner + Density
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(4.0))
                        .children(actor_chips.into_iter().map(|(flt, sfx, zh, en)| {
                            let active = actor_filter == flt;
                            div()
                                .id(SharedString::from(format!("actor-filter-{sfx}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                                .text_color(rgb(if active { pal.accent } else { pal.text_secondary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_actor_filter(flt, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(4.0))
                        .child(
                            div()
                                .id("community-algo-tuner-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .hover(move |s| {
                                    s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.24))
                                })
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.state.algo_draft = this.state.algo_prefs.clone();
                                        this.toggle_popover(CommunityPopover::AlgoTuner, cx);
                                    }),
                                )
                                .child(tg_icon(IconName::SparkleAlgo, 12.0, pal.accent))
                                .child(tr_str(lang, "算法调优", "Algo Tuner")),
                        )
                        .child(
                            div()
                                .id("community-density-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .hover(move |s| s.bg(hover_bg))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text_secondary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.cycle_density(cx);
                                    }),
                                )
                                .child(match density {
                                    FeedDensity::Card => tr_str(lang, "卡片视图", "Card"),
                                    FeedDensity::Compact => tr_str(lang, "紧凑视图", "Compact"),
                                    FeedDensity::Classic => tr_str(lang, "经典视图", "Classic"),
                                }),
                        ),
                ),
        )
        // Row 3: Reddit Sort & TimeWindow bar
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(4.0))
                .children(sort_chips.into_iter().map(|(s, sfx, zh, en)| {
                    let active = sort == s;
                    div()
                        .id(SharedString::from(format!("feed-sort-{sfx}")))
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(2.5))
                        .rounded(px(6.0))
                        .bg(if active {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14)
                        } else {
                            Rgba::default()
                        })
                        .hover(move |st| st.bg(hover_bg))
                        .text_size(px(11.0))
                        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(rgb(if active { pal.accent } else { pal.text_secondary }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_feed_sort(s, cx);
                            }),
                        )
                        .child(tr_str(lang, zh, en))
                }))
                .when(matches!(sort, FeedSort::Top | FeedSort::Controversial), |row| {
                    row.child(div().w(px(1.0)).h(px(12.0)).mx(px(3.0)).bg(pal.separator_rgba()))
                        .children(time_chips.into_iter().map(|(tw, zh, en)| {
                            let active = time_window == tw;
                            div()
                                .id(SharedString::from(format!("feed-tw-{en}")))
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    pal.service_pill_rgba()
                                })
                                .text_size(px(10.5))
                                .font_weight(if active { FontWeight::BOLD } else { FontWeight::NORMAL })
                                .text_color(rgb(if active { pal.accent } else { pal.text_secondary }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.set_time_window(tw, cx);
                                    }),
                                )
                                .child(tr_str(lang, zh, en))
                        }))
                }),
        )
}

fn render_inline_quick_post(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active_actor = ws.state.active_actor().cloned();
    let title_focused = ws.state.active_input == ActiveCommunityInput::QuickPostTitle;
    let body_focused = ws.state.active_input == ActiveCommunityInput::QuickPostBody;
    let expanded = ws.state.quick_post_expanded
        || title_focused
        || body_focused
        || !ws.state.quick_post_body.is_empty()
        || !ws.state.quick_post_title.is_empty();
    let submitting = ws.state.quick_post_submitting;
    let val_err = ws.state.quick_post_validation_error.clone();
    let selected_comm = ws.state.quick_post_community.clone();
    let base_url = ws.media_base_url.clone();

    let (aid, dname, handle, kind, avatar_id, shape, color) = match active_actor {
        Some(a) => (
            a.actor_id,
            a.display_name,
            a.handle,
            a.actor_kind,
            a.avatar_media_id,
            a.avatar_shape,
            a.avatar_color,
        ),
        None => (
            "u:local".to_string(),
            "Citizen".to_string(),
            "citizen".to_string(),
            ActorKind::Human,
            None,
            "classic".to_string(),
            String::new(),
        ),
    };

    let act_as_label = if lang.starts_with("zh") {
        format!("以 @{handle} 发言")
    } else {
        format!("Posting as @{handle}")
    };

    div()
        .id("community-quick-post")
        .w_full()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(if body_focused || title_focused {
            rgb(pal.accent).into()
        } else {
            pal.hairline_border_rgba()
        })
        .flex()
        .flex_col()
        .gap(px(8.0))
        // Top strip: avatar + Act-As identity pill + c/ community picker pill
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
                        .gap(px(8.0))
                        .child(render_actor_avatar(
                            &aid,
                            &dname,
                            kind,
                            avatar_id.as_deref(),
                            &shape,
                            &color,
                            28.0,
                            pal.card_bg,
                            &base_url,
                            true,
                            &pal,
                        ))
                        .child(
                            div()
                                .id("quick-post-act-as-pill")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .hover(move |s| s.bg(pal.row_hover_rgba()))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
                                    }),
                                )
                                .child(act_as_label)
                                .child(tg_icon(IconName::ChevronDown, 10.0, pal.text_secondary)),
                        ),
                )
                .child(
                    div()
                        .id("quick-post-community-pill")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded_full()
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .hover(move |s| s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.20)))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_popover(CommunityPopover::QuickPostCommunityPicker, cx);
                            }),
                        )
                        .child(
                            selected_comm
                                .as_deref()
                                .map(|s| format!("c/{s}"))
                                .unwrap_or_else(|| tr_str(lang, "选择社区 (可选)", "Pick c/community").to_string()),
                        )
                        .child(tg_icon(IconName::ChevronDown, 10.0, pal.accent)),
                ),
        )
        // Optional title field when expanded
        .when(expanded, |col| {
            col.child(
                div()
                    .id("quick-post-title-input")
                    .cursor_text()
                    .w_full()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(if title_focused {
                        rgb(pal.accent).into()
                    } else {
                        pal.hairline_border_rgba()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.state.quick_post_expanded = true;
                            this.state.quick_post_validation_error = None;
                            this.state.focus_input(ActiveCommunityInput::QuickPostTitle);
                            cx.notify();
                        }),
                    )
                    .child(render_input_display(
                        &ws.state.quick_post_title,
                        if title_focused {
                            ws.state.ime_preedit.as_deref()
                        } else {
                            None
                        },
                        tr_str(lang, "标题 (可选，支持 Reddit 讨论帖样式)…", "Title (optional Reddit-style headline)…"),
                        title_focused,
                        &pal,
                    )),
            )
        })
        // Main IME-wired body input
        .child(
            div()
                .id("quick-post-body-input")
                .cursor_text()
                .w_full()
                .min_h(px(if expanded { 62.0 } else { 38.0 }))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(9.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if body_focused {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.quick_post_expanded = true;
                        this.state.quick_post_validation_error = None;
                        this.state.focus_input(ActiveCommunityInput::QuickPostBody);
                        cx.notify();
                    }),
                )
                .child(render_input_display(
                    &ws.state.quick_post_body,
                    if body_focused {
                        ws.state.ime_preedit.as_deref()
                    } else {
                        None
                    },
                    tr_str(
                        lang,
                        "分享想法、Markdown 代码、@提及人类或 Bot、#话题、c/社区… (Shift+Enter 换行，Enter 发布)",
                        "Share thoughts, Markdown, @mention humans/bots, #tags, c/community… (Shift+Enter newline, Enter post)",
                    ),
                    body_focused,
                    &pal,
                )),
        )
        // Validation error + submit bar
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(if val_err.is_some() {
                            pal.danger
                        } else {
                            pal.text_secondary
                        }))
                        .child(val_err.unwrap_or_else(|| {
                            tr_str(lang, "支持 Markdown / @Bot / #话题 / c/社区", "Markdown · @Bot · #tag · c/community")
                                .to_string()
                        })),
                )
                .child(
                    div()
                        .id("quick-post-submit-btn")
                        .cursor_pointer()
                        .px(px(14.0))
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
                                this.submit_quick_post(cx);
                            }),
                        )
                        .child(if submitting {
                            tr_str(lang, "发布中…", "Posting…")
                        } else {
                            tr_str(lang, "发布", "Post")
                        }),
                ),
        )
}

/// Renders the `c/<slug>` community picker popover for the inline Quick-Post box.
pub fn render_quick_post_community_picker_popover(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let current = ws.state.quick_post_community.clone();
    let comms = ws.state.communities.clone();
    let hover_bg = pal.row_hover_rgba();

    macos_popover_surface("quick-post-comm-popover", pal)
        .w(px(260.0))
        .max_h(px(320.0))
        .p(px(8.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id("qp-comm-none")
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .when(current.is_none(), |el| {
                    el.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                })
                .hover(move |s| s.bg(hover_bg))
                .text_size(px(12.0))
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.quick_post_community = None;
                        this.state.active_popover = None;
                        cx.notify();
                    }),
                )
                .child(tr_str(lang, "个人主页 / 广场 (不限定社区)", "Profile / Global Stream (No c/)")),
        )
        .children(comms.into_iter().take(12).map(|c| {
            let slug = c.slug.clone();
            let active = current.as_deref() == Some(c.slug.as_str());
            div()
                .id(SharedString::from(format!("qp-comm-{}", c.slug)))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .when(active, |el| {
                    el.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                })
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .gap(px(7.0))
                .text_size(px(12.0))
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.quick_post_community = Some(slug.clone());
                        this.state.active_popover = None;
                        cx.notify();
                    }),
                )
                .child(if c.icon_emoji.trim().is_empty() {
                    "🌐".to_string()
                } else {
                    c.icon_emoji
                })
                .child(format!("c/{}", c.slug))
        }))
        .into_any_element()
}

/// Renders the interactive Algorithm Tuner popover with all 7 `AlgoPreset`s, weight steppers,
/// bot content ratio control, NSFW/explainability toggles, and live Apply (`update_algo_prefs`).
pub fn render_algo_tuner_popover(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let draft = ws.state.algo_draft.clone();

    let presets: [(AlgoPreset, &'static str, &'static str, &'static str); 7] = [
        (AlgoPreset::Balanced, "balanced", "平衡融合", "Balanced"),
        (AlgoPreset::RedditDeepDive, "reddit", "Reddit 深度", "Reddit Deep"),
        (AlgoPreset::XRealTime, "xrt", "X 实时脉搏", "X Real-Time"),
        (AlgoPreset::Discovery, "disc", "高探索", "Discovery"),
        (AlgoPreset::HumanFocused, "human", "偏人类", "Human Focus"),
        (AlgoPreset::BotHeavy, "bot", "偏 Bot", "Bot Heavy"),
        (AlgoPreset::Chronological, "chrono", "纯时间序", "Chronological"),
    ];

    let defs = AlgoPreferencesDto::default();

    let weight_row = |id_pfx: &'static str,
                      label: &'static str,
                      val: f64,
                      default_val: f64,
                      on_delta: fn(&mut CommunityWorkspace, f64)| {
        let safe_val = sanitize_unit_f64(val, default_val);
        let pct = (safe_val * 100.0).round() as u32;
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.5))
                    .child(div().text_color(rgb(pal.text_primary)).child(label))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("algo-dec-{id_pfx}")))
                                    .cursor_pointer()
                                    .w(px(20.0))
                                    .h(px(20.0))
                                    .rounded(px(5.0))
                                    .bg(pal.service_pill_rgba())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            on_delta(this, -0.10);
                                            cx.notify();
                                        }),
                                    )
                                    .child("-"),
                            )
                            .child(
                                div()
                                    .w(px(34.0))
                                    .text_center()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .child(format!("{pct}%")),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("algo-inc-{id_pfx}")))
                                    .cursor_pointer()
                                    .w(px(20.0))
                                    .h(px(20.0))
                                    .rounded(px(5.0))
                                    .bg(pal.service_pill_rgba())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            on_delta(this, 0.10);
                                            cx.notify();
                                        }),
                                    )
                                    .child("+"),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(5.0))
                    .rounded_full()
                    .bg(pal.service_pill_rgba())
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative((safe_val as f32).clamp(0.02, 1.0)))
                            .rounded_full()
                            .bg(rgb(pal.accent)),
                    ),
            )
    };

    macos_popover_surface("community-algo-tuner-popover", pal)
        .w(px(pal.scaled_px(350.0)))
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon(IconName::SparkleAlgo, 15.0, pal.accent))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "推荐算法调优 (5-Stage Ranker)", "Algorithm Tuner")),
                        ),
                )
                .child(
                    div()
                        .id("algo-tuner-close-btn")
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.active_popover = None;
                                cx.notify();
                            }),
                        )
                        .child(tg_icon(IconName::Close, 13.0, pal.text_secondary)),
                ),
        )
        // 7 AlgoPreset pills
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(presets.into_iter().map(|(preset, sfx, zh, en)| {
                    let active = draft.preset == preset;
                    div()
                        .id(SharedString::from(format!("algo-preset-{sfx}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if active {
                            rgb(pal.accent).into()
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                        .text_color(rgb(if active {
                            pal.active_button_fg
                        } else {
                            pal.text_primary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let allow_nsfw = this.state.algo_draft.allow_nsfw;
                                let show_exp = this.state.algo_draft.show_explanations;
                                let mut next = gpui_telegram_community::default_prefs_for_preset(preset);
                                next.allow_nsfw = allow_nsfw;
                                next.show_explanations = show_exp;
                                this.state.algo_draft = next;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, zh, en))
                })),
        )
        // Interactive weight steppers (NaN/Inf-safe)
        .child(weight_row(
            "recency",
            tr_str(lang, "时效性权重", "Recency Weight"),
            draft.recency_weight,
            defs.recency_weight,
            |this, d| {
                let def = AlgoPreferencesDto::default().recency_weight;
                let cur = sanitize_unit_f64(this.state.algo_draft.recency_weight, def);
                this.state.algo_draft.recency_weight = sanitize_unit_f64(cur + d, def);
            },
        ))
        .child(weight_row(
            "affinity",
            tr_str(lang, "社交亲和权重", "Affinity Weight"),
            draft.affinity_weight,
            defs.affinity_weight,
            |this, d| {
                let def = AlgoPreferencesDto::default().affinity_weight;
                let cur = sanitize_unit_f64(this.state.algo_draft.affinity_weight, def);
                this.state.algo_draft.affinity_weight = sanitize_unit_f64(cur + d, def);
            },
        ))
        .child(weight_row(
            "quality",
            tr_str(lang, "互动质量权重", "Quality Weight"),
            draft.quality_weight,
            defs.quality_weight,
            |this, d| {
                let def = AlgoPreferencesDto::default().quality_weight;
                let cur = sanitize_unit_f64(this.state.algo_draft.quality_weight, def);
                this.state.algo_draft.quality_weight = sanitize_unit_f64(cur + d, def);
            },
        ))
        .child(weight_row(
            "discovery",
            tr_str(lang, "跨圈探索比例", "Discovery Ratio"),
            draft.discovery_ratio,
            defs.discovery_ratio,
            |this, d| {
                let def = AlgoPreferencesDto::default().discovery_ratio;
                let cur = sanitize_unit_f64(this.state.algo_draft.discovery_ratio, def);
                this.state.algo_draft.discovery_ratio = sanitize_unit_f64(cur + d, def);
            },
        ))
        .child(weight_row(
            "botratio",
            tr_str(lang, "Bot 内容占比", "Bot Content Ratio"),
            draft.bot_content_ratio,
            defs.bot_content_ratio,
            |this, d| {
                let def = AlgoPreferencesDto::default().bot_content_ratio;
                let cur = sanitize_unit_f64(this.state.algo_draft.bot_content_ratio, def);
                this.state.algo_draft.bot_content_ratio = sanitize_unit_f64(cur + d, def);
            },
        ))
        // Toggles: show_explanations & allow_nsfw
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .id("algo-toggle-explain")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .bg(if draft.show_explanations {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .text_color(rgb(if draft.show_explanations {
                            pal.accent
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.algo_draft.show_explanations =
                                    !this.state.algo_draft.show_explanations;
                                cx.notify();
                            }),
                        )
                        .child(if draft.show_explanations {
                            tr_str(lang, "✓ 显示推荐理由", "✓ Explanations On")
                        } else {
                            tr_str(lang, "显示推荐理由", "Explanations Off")
                        }),
                )
                .child(
                    div()
                        .id("algo-toggle-nsfw")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .bg(if draft.allow_nsfw {
                            TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14)
                        } else {
                            pal.service_pill_rgba()
                        })
                        .text_size(px(11.0))
                        .text_color(rgb(if draft.allow_nsfw {
                            pal.danger
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.algo_draft.allow_nsfw = !this.state.algo_draft.allow_nsfw;
                                cx.notify();
                            }),
                        )
                        .child(if draft.allow_nsfw {
                            tr_str(lang, "✓ 允许 NSFW", "✓ Allow NSFW")
                        } else {
                            tr_str(lang, "过滤 NSFW", "Filter NSFW")
                        }),
                ),
        )
        // Apply button
        .child(
            div()
                .id("algo-tuner-apply-btn")
                .cursor_pointer()
                .w_full()
                .py(px(7.0))
                .rounded(px(9.0))
                .bg(rgb(pal.accent))
                .hover(move |s| s.bg(rgb(pal.accent_hover)))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.save_algo_prefs(cx);
                    }),
                )
                .child(tr_str(lang, "应用并刷新信息流", "Apply & Refresh Feed")),
        )
        .into_any_element()
}
