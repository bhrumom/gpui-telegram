//! 296px Community Right Discovery Rail (3-column mode):
//! IME Search Box, Trending Now, Recommended Communities (1-click Join),
//! Who to Follow (Humans + Bots, 1-click Follow), and Live Bot Activity Pulse.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::community_contracts::TrendingItemKind;

use crate::components::grok_bot_avatar::{render_grok_persona_avatar, GrokPersonaState};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

use super::shared_widgets::{
    format_compact_count, render_actor_avatar, render_actor_badge_row, render_input_display,
};
use super::state::{ActiveCommunityInput, CommunityRoute};
use super::workspace::CommunityWorkspace;

/// Renders the 296px right-hand discovery rail (mounted only in `ThreeColumn` layout mode).
pub fn render_right_rail(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let search_focused = ws.state.active_input == ActiveCommunityInput::RightRailSearch;
    let search_preedit = if search_focused {
        ws.state.ime_preedit.as_deref()
    } else {
        None
    };
    let rail_w = pal.scaled_px(296.0).clamp(260.0, 340.0);

    div()
        .id("community-right-rail")
        .w(px(rail_w))
        .flex_shrink_0()
        .h_full()
        .bg(rgb(pal.dialogs_bg))
        .border_l_1()
        .border_color(pal.separator_rgba())
        .overflow_y_scroll()
        .track_scroll(&ws.right_rail_scroll_handle)
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        // 1. IME-wired Search Box
        .child(
            div()
                .id("community-right-search")
                .cursor_text()
                .w_full()
                .px(px(10.0))
                .py(px(8.0))
                .rounded_full()
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if search_focused {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .flex()
                .items_center()
                .gap(px(7.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.focus_input(ActiveCommunityInput::RightRailSearch);
                        cx.notify();
                    }),
                )
                .child(tg_icon(IconName::Search, 14.0, pal.text_secondary))
                .child(div().flex_1().min_w(px(0.0)).child(render_input_display(
                    &ws.state.right_rail_search_draft,
                    search_preedit,
                    tr_str(lang, "搜索帖子、@公民、Bot、c/社区…", "Search posts, @citizens, Bots, c/…"),
                    search_focused,
                    &pal,
                ))),
        )
        // 2. Trending Now
        .child(render_trending_card(ws, cx))
        // 3. Recommended Communities
        .child(render_recommended_communities_card(ws, cx))
        // 4. Who to Follow (Humans & Bots)
        .child(render_who_to_follow_card(ws, cx))
        // 5. Live Bot Activity Pulse
        .child(render_bot_pulse_card(ws, cx))
        .into_any_element()
}

fn render_rail_card_shell(pal: &TdesktopThemePalette) -> Div {
    div()
        .w_full()
        .p(px(11.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(8.0))
}

fn render_trending_card(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let topics: Vec<_> = ws.state.trending.iter().take(6).cloned().collect();
    let hover_bg = pal.row_hover_rgba();

    render_rail_card_shell(&pal)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon(IconName::FlameTrending, 15.0, 0xF97316))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "实时热议", "Trending Now")),
                ),
        )
        .when(topics.is_empty(), |col| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(lang, "正在聚合全站热点…", "Aggregating live trends…")),
            )
        })
        .children(topics.into_iter().enumerate().map(|(i, topic)| {
            let kind = topic.kind;
            let slug_or_tag = topic.slug_or_tag.clone();
            let row_id = SharedString::from(format!("rail-trend-{i}-{}", topic.id));
            div()
                .id(row_id)
                .cursor_pointer()
                .px(px(7.0))
                .py(px(5.0))
                .rounded(px(8.0))
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if kind == TrendingItemKind::Community {
                            let clean = slug_or_tag.trim_start_matches("c/").to_string();
                            this.navigate_to(CommunityRoute::CommunityPage(clean), cx);
                        } else {
                            this.open_explore_with_query(slug_or_tag.clone(), cx);
                        }
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(10.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(topic.category.clone())
                        .child(format!("{}H · {}B", topic.human_posts_count, topic.bot_posts_count)),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(topic.label.clone()),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "{} {}",
                            format_compact_count(topic.posts_count_24h as i64),
                            tr_str(lang, "条讨论 (24h)", "posts (24h)")
                        )),
                )
        }))
}

fn render_recommended_communities_card(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let comms: Vec<_> = ws.state.communities.iter().take(5).cloned().collect();
    let hover_bg = pal.row_hover_rgba();

    render_rail_card_shell(&pal)
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
                        .child(tg_icon(IconName::CommunityGlobe, 14.5, pal.accent))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "推荐社区 (c/)", "Communities")),
                        ),
                )
                .child(
                    div()
                        .id("rail-create-community-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(2.5))
                        .rounded_full()
                        .bg(pal.service_pill_rgba())
                        .hover(move |s| s.bg(hover_bg))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_create_community_modal(cx);
                            }),
                        )
                        .child(tr_str(lang, "+ 创建", "+ Create")),
                ),
        )
        .children(comms.into_iter().map(|c| {
            let nav_slug = c.slug.clone();
            let join_slug = c.slug.clone();
            let joined = c.is_joined;
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .id(SharedString::from(format!("rail-comm-{}", c.slug)))
                        .cursor_pointer()
                        .flex_1()
                        .min_w(px(0.0))
                        .px(px(5.0))
                        .py(px(4.0))
                        .rounded(px(7.0))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.navigate_to(CommunityRoute::CommunityPage(nav_slug.clone()), cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(14.0))
                                .child(if c.icon_emoji.trim().is_empty() {
                                    "🌐".to_string()
                                } else {
                                    c.icon_emoji
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
                                        .truncate()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(format!("c/{}", c.slug)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "{} · {} Bots",
                                            format_compact_count(c.member_count as i64),
                                            c.bot_member_count
                                        )),
                                ),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("rail-comm-join-{}", c.slug)))
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if joined {
                            pal.service_pill_rgba()
                        } else {
                            rgb(pal.accent).into()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if joined {
                            pal.text_secondary
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
                            tr_str(lang, "加入", "Join")
                        }),
                )
        }))
}

fn render_who_to_follow_card(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let active_id = ws.state.active_actor_id.as_str();
    let base_url = ws.media_base_url.clone();
    let actors: Vec<_> = ws
        .state
        .recommended_actors
        .iter()
        .filter(|a| a.actor_id != active_id)
        .take(5)
        .cloned()
        .collect();
    let hover_bg = pal.row_hover_rgba();

    render_rail_card_shell(&pal)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon(IconName::ContactsUsers, 14.5, pal.accent))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "推荐关注 (人类 & Bot)", "Who to Follow")),
                ),
        )
        .when(actors.is_empty(), |col| {
            col.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(lang, "正在发现活跃人类与 Bot 创作者…", "Discovering active humans & bots…")),
            )
        })
        .children(actors.into_iter().map(|a| {
            let handle = a.handle.clone();
            let aid = a.actor_id.clone();
            let following = a.is_following;
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .id(SharedString::from(format!("rail-actor-{}", a.actor_id)))
                        .cursor_pointer()
                        .flex_1()
                        .min_w(px(0.0))
                        .px(px(4.0))
                        .py(px(3.0))
                        .rounded(px(7.0))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_actor_profile(handle.clone(), cx);
                            }),
                        )
                        .child(render_actor_avatar(
                            &a.actor_id,
                            &a.display_name,
                            a.actor_kind,
                            a.avatar_media_id.as_deref(),
                            &a.avatar_shape,
                            &a.avatar_color,
                            28.0,
                            pal.card_bg,
                            &base_url,
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
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(a.display_name.clone()),
                                        )
                                        .child(render_actor_badge_row(
                                            a.actor_kind,
                                            a.verified_tier,
                                            a.bot_badge.as_deref(),
                                            false,
                                            false,
                                            &a.avatar_color,
                                            &pal,
                                            lang,
                                        )),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("@{}", a.handle)),
                                ),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("rail-follow-{}", a.actor_id)))
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px(px(9.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(if following {
                            pal.service_pill_rgba()
                        } else {
                            rgb(pal.accent).into()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if following {
                            pal.text_secondary
                        } else {
                            pal.active_button_fg
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_follow_actor(aid.clone(), !following, cx);
                            }),
                        )
                        .child(if following {
                            tr_str(lang, "已关注", "Following")
                        } else {
                            tr_str(lang, "关注", "Follow")
                        }),
                )
        }))
}

fn render_bot_pulse_card(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let bots: Vec<_> = ws
        .state
        .recommended_actors
        .iter()
        .filter(|a| a.actor_kind.is_bot_like())
        .take(3)
        .cloned()
        .collect();
    let bot_posts_count = ws
        .state
        .feed_items
        .iter()
        .filter(|it| it.post.author_kind.is_bot_like())
        .count();

    render_rail_card_shell(&pal)
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
                        .child(tg_icon(IconName::BotRobot, 15.0, 0x8B5CF6))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "Bot 公民实时脉搏", "Live Bot Pulse")),
                        ),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(1.5))
                        .rounded_full()
                        .bg(TdesktopThemePalette::rgba_with_alpha(0x10B981, 0.16))
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0x10B981))
                        .child(format!("{bot_posts_count} live")),
                ),
        )
        .children(bots.into_iter().map(|b| {
            let handle = b.handle.clone();
            let shape = if b.avatar_shape.is_empty() {
                "blob"
            } else {
                b.avatar_shape.as_str()
            };
            let color = if b.avatar_color.is_empty() {
                "violet"
            } else {
                b.avatar_color.as_str()
            };
            div()
                .id(SharedString::from(format!("rail-bot-pulse-{}", b.actor_id)))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(5.0))
                .rounded(px(8.0))
                .hover(move |s| s.bg(pal.row_hover_rgba()))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_actor_profile(handle.clone(), cx);
                    }),
                )
                .child(render_grok_persona_avatar(
                    &b.actor_id,
                    shape,
                    color,
                    &b.display_name,
                    26.0,
                    GrokPersonaState::Idle,
                    pal.card_bg,
                    true,
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
                                .child(b.display_name),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "{} · {} karma",
                                    b.bot_model.unwrap_or_else(|| "Agent".to_string()),
                                    format_compact_count(b.post_karma + b.comment_karma)
                                )),
                        ),
                )
        }))
        .child(
            div()
                .id("rail-open-bot-studio")
                .cursor_pointer()
                .w_full()
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.14))
                .hover(move |s| s.bg(TdesktopThemePalette::rgba_with_alpha(0x8B5CF6, 0.24)))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(5.0))
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(0x8B5CF6))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.navigate_to(CommunityRoute::BotStudio, cx);
                    }),
                )
                .child(tg_icon(IconName::BotRobot, 13.0, 0x8B5CF6))
                .child(tr_str(lang, "在 Bot 工作室创建/管理 Bot", "Open Bot Studio")),
        )
}
