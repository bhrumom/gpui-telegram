//! Top Bar (54px), Multi-Select Bar, In-Chat Search Bar, Pinned Banner (46px) & Scroll-to-Bottom Button.

use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_telegram_proto::miniapp_contracts::{
    resolve_grok_persona_color, resolve_grok_persona_shape, FabushiBotSummaryDto,
};

use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, GrokBotAvatarProps, GrokPersonaState,
};
use crate::i18n::{tr_fmt, tr_key_or, tr_str};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use super::rich_meta::resolve_image_source;

pub const APPLE_STAGE_TOP_BAR_HEIGHT: f32 = 76.0;
pub const APPLE_STAGE_EMPTY_TOP_BAR_HEIGHT: f32 = 52.0;

#[derive(Debug, Clone)]
pub enum TopBarUiAction {
    ToggleInspector,
    ToggleSearch,
    FocusSearch,
    SearchStep(i32),
    SearchClear,
    StartCall { video: bool },
    ToggleTopMoreMenu,
    DismissTopMoreMenu,
    ToggleTtlSubmenu,
    SetMessageTtl(u32),
    ViewProfile,
    ChangeWallpaper,
    TogglePinChat,
    ToggleBlockPeer,
    DeleteOrLeaveChat,
    JumpPinned,
    UnpinCurrent,
    EnterSelectMode,
    ClearSelection,
    ForwardSelected,
    CopySelected,
    DeleteSelected,
    ScrollToBottom,
    ClearHistory,
    ExportChat,
    ToggleMuteChat,
}

fn format_ttl_badge(ttl_seconds: u32, lang_id: &str) -> String {
    match ttl_seconds {
        0 => tr_str(lang_id, "关闭", "Off").to_string(),
        86_400 => tr_str(lang_id, "1 天", "1 day").to_string(),
        604_800 => tr_str(lang_id, "1 周", "1 week").to_string(),
        2_592_000 | 2_678_400 => tr_str(lang_id, "1 个月", "1 month").to_string(),
        secs if secs >= 86_400 => format!("{}d", secs / 86_400),
        secs if secs >= 3600 => format!("{}h", secs / 3600),
        secs => format!("{}s", secs),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_stage_top_bar<V: 'static>(
    chat_id: Option<u64>,
    title: &str,
    subtitle: &str,
    initials: &str,
    avatar_url: Option<&str>,
    active_fabushi_bot: Option<&FabushiBotSummaryDto>,
    is_bot_chat: bool,
    bot_activity_state: &str,
    select_mode_active: bool,
    selected_count: usize,
    message_ttl_seconds: u32,
    search_open: bool,
    search_query: &str,
    search_match_cursor: usize,
    search_match_total: usize,
    is_inspector_open: bool,
    more_menu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_stage_top_bar_with_group(
        chat_id,
        title,
        subtitle,
        initials,
        avatar_url,
        active_fabushi_bot,
        is_bot_chat,
        false,
        None,
        bot_activity_state,
        select_mode_active,
        selected_count,
        message_ttl_seconds,
        search_open,
        search_query,
        search_match_cursor,
        search_match_total,
        is_inspector_open,
        more_menu_open,
        palette,
        lang_id,
        cx,
        on_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_stage_top_bar_with_group<V: 'static>(
    chat_id: Option<u64>,
    title: &str,
    subtitle: &str,
    initials: &str,
    avatar_url: Option<&str>,
    active_fabushi_bot: Option<&FabushiBotSummaryDto>,
    is_bot_chat: bool,
    is_group_chat: bool,
    group_member_tiles: Option<&[(String, u32)]>,
    bot_activity_state: &str,
    select_mode_active: bool,
    selected_count: usize,
    message_ttl_seconds: u32,
    search_open: bool,
    search_query: &str,
    search_match_cursor: usize,
    search_match_total: usize,
    is_inspector_open: bool,
    more_menu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let bar_h = px(palette.scaled_px(if is_apple {
        APPLE_STAGE_EMPTY_TOP_BAR_HEIGHT
    } else {
        54.0
    }));
    let bar_bg: Rgba = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            palette.topbar_bg,
            if palette.mode.is_dark() { 0.88 } else { 0.90 },
        )
    } else {
        rgb(palette.topbar_bg).into()
    };
    if select_mode_active || selected_count > 0 {
        let count_str = selected_count.to_string();
        let a_fwd = on_action.clone();
        let a_cpy = on_action.clone();
        let a_del = on_action.clone();
        let a_clr = on_action.clone();
        let pill_r = px(if is_apple { 999.0 } else { 6.0 });
        let left_controls = if selected_count == 0 {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(tg_icon(IconName::SelectCheck, palette.scaled_px(16.0), palette.accent))
                .child(
                    div()
                        .text_size(px(palette.scaled_px(13.0)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.text_primary))
                        .child(tr_str(lang_id, "点击消息进行多选", "Click messages to select")),
                )
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("ms-fwd-btn")
                        .px(px(14.0))
                        .py(px(6.0))
                        .rounded(pill_r)
                        .bg(rgb(palette.accent))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| a_fwd(this, TopBarUiAction::ForwardSelected, cx)))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon(IconName::Forward, 14.0, 0xFFFFFF))
                        .child(
                            div()
                                .text_size(px(palette.scaled_px(12.0)))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xFFFFFF))
                                .child(format!("{} {}", tr_key_or(lang_id, "lng_context_forward_msg", "Forward"), count_str)),
                        ),
                )
                .child(
                    div()
                        .id("ms-copy-btn")
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(pill_r)
                        .bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.14))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| a_cpy(this, TopBarUiAction::CopySelected, cx)))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(tg_icon(IconName::Copy, 14.0, palette.accent))
                        .child(
                            div()
                                .text_size(px(palette.scaled_px(12.0)))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.accent))
                                .child(tr_key_or(lang_id, "lng_copy", "Copy")),
                        ),
                )
                .child(
                    div()
                        .id("ms-del-btn")
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(pill_r)
                        .bg(TdesktopThemePalette::rgba_with_alpha(palette.danger, 0.16))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| a_del(this, TopBarUiAction::DeleteSelected, cx)))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(tg_icon(IconName::DeleteTrash, 14.0, palette.danger))
                        .child(
                            div()
                                .text_size(px(palette.scaled_px(12.0)))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(palette.danger))
                                .child(tr_key_or(lang_id, "lng_context_delete_msg", "Delete")),
                        ),
                )
        };
        return div()
            .id("stage-multiselect-bar")
            .h(bar_h)
            .w_full()
            .px(px(16.0))
            .bg(bar_bg)
            .when(!is_apple, |el| el.border_b_1().border_color(rgb(palette.border_fg)))
            .when(is_apple, |el| el.border_b_1().border_color(palette.separator_rgba()))
            .flex()
            .items_center()
            .justify_between()
            .child(left_controls)
            .child(div().flex_1().h_full().window_control_area(WindowControlArea::Drag))
            .child(
                div()
                    .id("ms-cancel-btn")
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(pill_r)
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                    .on_click(cx.listener(move |this, _, _, cx| a_clr(this, TopBarUiAction::ClearSelection, cx)))
                    .text_size(px(palette.scaled_px(12.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.accent))
                    .child(tr_key_or(lang_id, "lng_cancel", "Cancel")),
            )
            .into_any_element();
    }

    let pid = chat_id.unwrap_or(0) as i64;
    let (grad_top, grad_bot) = palette.peer_avatar_gradient(pid);
    let img_src = avatar_url.and_then(|u| resolve_image_source(u, None));
    let has_chat = chat_id.is_some();
    let is_dark = palette.mode.is_dark();
    let a_insp1 = on_action.clone();
    let a_insp2 = on_action.clone();
    let a_search = on_action.clone();
    let a_more = on_action.clone();
    let ttl_header_label = (message_ttl_seconds > 0).then(|| format_ttl_badge(message_ttl_seconds, lang_id));
    let ttl_badge_el: Option<AnyElement> = ttl_header_label.map(|ttl_lbl| {
        div()
            .id("topbar-ttl-badge")
            .flex_shrink_0()
            .px(px(palette.scaled_px(6.0)))
            .py(px(palette.scaled_px(1.5)))
            .rounded_full()
            .bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16))
            .flex()
            .items_center()
            .gap(px(3.0))
            .child(tg_icon(IconName::ClockSending, palette.scaled_px(11.0), palette.accent))
            .child(
                div()
                    .text_size(px(palette.scaled_px(10.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.accent))
                    .child(ttl_lbl),
            )
            .into_any_element()
    });
    let sub_lower = subtitle.to_ascii_lowercase();
    let is_typing_or_active_bot = sub_lower.contains("typing")
        || subtitle.contains("输入")
        || subtitle.contains("思考")
        || (!bot_activity_state.trim().is_empty()
            && !bot_activity_state.eq_ignore_ascii_case("idle"));
    let is_online = sub_lower == "online" || subtitle == "在线";
    let is_group_sub = sub_lower.contains("member")
        || sub_lower.contains("subscriber")
        || subtitle.contains("成员")
        || subtitle.contains("订阅");
    let show_apple_subtitle = !subtitle.is_empty() && (is_typing_or_active_bot || is_online || is_group_sub);

    let avatar_el: Option<AnyElement> = has_chat.then(|| {
        let avatar_size_px = palette.scaled_px(40.0);
        let is_typing = is_typing_or_active_bot;
        let fab_non_classic = active_fabushi_bot
            .filter(|f| !f.avatar_shape.eq_ignore_ascii_case("classic"));
        let show_dynamic_bot_avatar =
            fab_non_classic.is_some() || (is_bot_chat && (img_src.is_none() || is_typing));
        let show_group_composite =
            (is_group_chat || is_group_sub) && img_src.is_none() && !show_dynamic_bot_avatar;
        let avatar_ring_col = palette.hairline_border_rgba();
        let avatar_shadow = if is_dark {
            Vec::new()
        } else {
            vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.10),
                offset: point(px(0.0), px(2.0)),
                blur_radius: px(8.0),
                spread_radius: px(0.0),
            }]
        };

        if show_group_composite {
            crate::views::sidebar::dialogs_row_view::render_composite_group_avatar(
                chat_id.unwrap_or(0),
                title,
                group_member_tiles,
                avatar_size_px,
                palette.stage_bg,
                is_dark,
            )
            .into_any_element()
        } else if show_dynamic_bot_avatar {
            if let Some(fab) = fab_non_classic {
                let effective_state = if !bot_activity_state.trim().is_empty()
                    && !bot_activity_state.eq_ignore_ascii_case("idle")
                {
                    bot_activity_state
                } else {
                    fab.execution_state.as_str()
                };
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(
                        &fab.agent_id,
                        &fab.avatar_shape,
                        &fab.avatar_color,
                        avatar_size_px,
                    )
                    .with_state(GrokPersonaState::from_activity_str(
                        effective_state,
                        is_typing,
                    ))
                    .with_follow_pointer(true)
                    .with_surface_bg(palette.topbar_bg),
                )
                .into_any_element()
            } else {
                let seed = format!("bot_{}_{}", chat_id.unwrap_or(0), title);
                let shape = resolve_grok_persona_shape(&seed, None);
                let color = resolve_grok_persona_color(&seed, None);
                let effective_state = if bot_activity_state.trim().is_empty() {
                    "idle"
                } else {
                    bot_activity_state
                };
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(&seed, &shape, &color, avatar_size_px)
                        .with_state(GrokPersonaState::from_activity_str(
                            effective_state,
                            is_typing,
                        ))
                        .with_follow_pointer(true)
                        .with_surface_bg(palette.topbar_bg),
                )
                .into_any_element()
            }
        } else if let Some(src) = img_src {
            div()
                .w(px(avatar_size_px))
                .h(px(avatar_size_px))
                .rounded_full()
                .overflow_hidden()
                .bg(rgb(grad_top))
                .when(is_apple, |el| {
                    el.border_1()
                        .border_color(avatar_ring_col)
                        .shadow(avatar_shadow.clone())
                })
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(img(src).size_full().object_fit(ObjectFit::Cover))
                .into_any_element()
        } else {
            div()
                .w(px(avatar_size_px))
                .h(px(avatar_size_px))
                .rounded_full()
                .overflow_hidden()
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgb(grad_top), 0.0),
                    linear_color_stop(rgb(grad_bot), 1.0),
                ))
                .when(is_apple, |el| {
                    el.border_1()
                        .border_color(avatar_ring_col)
                        .shadow(avatar_shadow)
                })
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(palette.scaled_px(if is_apple { 13.5 } else { 14.0 })))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .when(avatar_url.map_or(true, |u| u.trim().is_empty()), |av| {
                    av.child(initials.to_string())
                })
                .into_any_element()
        }
    });

    let bar = if is_apple {
        let apple_bar_h = px(palette.scaled_px(if has_chat {
            APPLE_STAGE_TOP_BAR_HEIGHT
        } else {
            APPLE_STAGE_EMPTY_TOP_BAR_HEIGHT
        }));
        let apple_bar_bg = linear_gradient(
            180.0,
            linear_color_stop(
                TdesktopThemePalette::rgba_with_alpha(
                    palette.stage_bg,
                    if is_dark { 0.90 } else { 0.92 },
                ),
                0.0,
            ),
            linear_color_stop(
                TdesktopThemePalette::rgba_with_alpha(
                    palette.stage_bg,
                    if is_dark { 0.55 } else { 0.62 },
                ),
                1.0,
            ),
        );
        let glass_pill_bg = if is_dark {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_bg, 0.88)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.88)
        };
        let name_pill_bg = if is_dark {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_bg, 0.90)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.88)
        };
        let name_pill_hover_bg = if is_dark {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_hover_bg, 0.96)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.98)
        };
        let glass_pill_border = if is_inspector_open {
            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.55)
        } else {
            palette.hairline_border_rgba()
        };
        let right_pill_border = palette.hairline_border_rgba();
        let pill_shadow = palette.popover_shadows();

        let mut root = div()
            .relative()
            .h(apple_bar_h)
            .w_full()
            .when(has_chat, |el| el.bg(apple_bar_bg))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .window_control_area(WindowControlArea::Drag),
            );

        if has_chat {
            let search_btn = top_icon_btn("tb-search", IconName::Search, search_open, palette, cx, move |this, cx| {
                a_search(this, TopBarUiAction::ToggleSearch, cx);
            });
            let info_btn = top_icon_btn("tb-info", IconName::SidebarPanel, is_inspector_open, palette, cx, move |this, cx| {
                a_insp2(this, TopBarUiAction::ToggleInspector, cx);
            });
            let more_btn = top_icon_btn("tb-more", IconName::MoreVertical, more_menu_open, palette, cx, move |this, cx| {
                a_more(this, TopBarUiAction::ToggleTopMoreMenu, cx);
            });

            let identity_pill = div()
                .mt(px(palette.scaled_px(-4.0)))
                .px(px(palette.scaled_px(10.0)))
                .py(px(palette.scaled_px(2.5)))
                .rounded_full()
                .bg(name_pill_bg)
                .hover(move |s| s.bg(name_pill_hover_bg))
                .border_1()
                .border_color(glass_pill_border)
                .shadow(pill_shadow.clone())
                .flex()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .max_w(px(palette.scaled_px(240.0)))
                .overflow_hidden()
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_size(px(palette.scaled_px(12.5)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.text_primary))
                        .truncate()
                        .child(title.to_string()),
                )
                .child(
                    div().flex_shrink_0().flex().items_center().child(tg_icon(
                        IconName::ChevronRight,
                        palette.scaled_px(10.5),
                        palette.text_secondary,
                    )),
                )
                .when_some(ttl_badge_el, |row, badge| row.child(badge));

            let peer_header = div()
                .id("topbar-peer-header")
                .occlude()
                .cursor_pointer()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .max_w(px(palette.scaled_px(260.0)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_insp1(this, TopBarUiAction::ToggleInspector, cx);
                    }),
                )
                .when_some(avatar_el, |col, av| col.child(av))
                .child(identity_pill)
                .when(show_apple_subtitle, |col| {
                    col.child(
                        div()
                            .mt(px(palette.scaled_px(2.0)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(4.0))
                            .max_w(px(palette.scaled_px(220.0)))
                            .overflow_hidden()
                            .when(is_online && !is_typing_or_active_bot, |row| {
                                row.child(
                                    div()
                                        .w(px(5.5))
                                        .h(px(5.5))
                                        .rounded_full()
                                        .flex_shrink_0()
                                        .bg(rgb(palette.online_dot_fg)),
                                )
                            })
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .text_size(px(palette.scaled_px(10.5)))
                                    .text_color(rgb(if is_typing_or_active_bot {
                                        palette.accent
                                    } else {
                                        palette.text_secondary
                                    }))
                                    .truncate()
                                    .child(subtitle.to_string()),
                            ),
                    )
                });

            let right_actions_pill = div()
                .id("topbar-right-actions-pill")
                .absolute()
                .top(px(palette.scaled_px(12.0)))
                .right(px(14.0))
                .occlude()
                .px(px(4.0))
                .py(px(3.0))
                .rounded_full()
                .bg(glass_pill_bg)
                .border_1()
                .border_color(right_pill_border)
                .shadow(pill_shadow)
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(search_btn)
                .child(info_btn)
                .child(more_btn);

            root = root
                .child(
                    div()
                        .w_full()
                        .h_full()
                        .px(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(peer_header),
                )
                .child(right_actions_pill);
        }

        root.into_any_element()
    } else {
        div()
            .relative()
            .h(bar_h)
            .w_full()
            .px(px(14.0))
            .bg(bar_bg)
            .border_b_1()
            .border_color(rgb(palette.border_fg))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .id("topbar-peer-header")
                    .flex()
                    .items_center()
                    .gap(px(palette.scaled_px(10.0)))
                    .flex_1()
                    .overflow_hidden()
                    .when(has_chat, |el| {
                        el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                            a_insp1(this, TopBarUiAction::ToggleInspector, cx);
                        }))
                    })
                    .when_some(avatar_el, |el, av| el.child(av))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(palette.scaled_px(14.0)))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette.text_primary))
                                            .truncate()
                                            .child(title.to_string()),
                                    )
                                    .when_some(ttl_badge_el, |row, badge| row.child(badge)),
                            )
                            .when(!subtitle.is_empty(), |el| {
                                let is_live = subtitle == "online" || subtitle.contains("typing");
                                el.child(
                                    div()
                                        .text_size(px(palette.scaled_px(12.0)))
                                        .text_color(rgb(if is_live {
                                            palette.online_dot_fg
                                        } else {
                                            palette.text_secondary
                                        }))
                                        .child(subtitle.to_string()),
                                )
                            }),
                    ),
            )
            .when(has_chat, |el| {
                let search_btn = top_icon_btn("tb-search", IconName::Search, search_open, palette, cx, move |this, cx| {
                    a_search(this, TopBarUiAction::ToggleSearch, cx);
                });
                let info_btn = top_icon_btn("tb-info", IconName::SidebarPanel, is_inspector_open, palette, cx, move |this, cx| {
                    a_insp2(this, TopBarUiAction::ToggleInspector, cx);
                });
                let more_btn = top_icon_btn("tb-more", IconName::MoreVertical, more_menu_open, palette, cx, move |this, cx| {
                    a_more(this, TopBarUiAction::ToggleTopMoreMenu, cx);
                });
                let actions = div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(search_btn)
                    .child(info_btn)
                    .child(more_btn);
                el.child(actions)
            })
            .into_any_element()
    };

    let _ = (search_query, search_match_cursor, search_match_total, tr_fmt, TopBarUiAction::StartCall { video: false });
    bar
}

pub fn render_stage_search_bar<V: 'static>(
    search_query: &str,
    search_match_cursor: usize,
    search_match_total: usize,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let a_focus = on_action.clone();
    let a_prev = on_action.clone();
    let a_next = on_action.clone();
    let a_close = on_action;
    let match_lbl = if search_query.is_empty() {
        tr_str(lang_id, "输入关键词搜索", "Type to search").to_string()
    } else if search_match_total == 0 {
        tr_str(lang_id, "无匹配结果", "0 / 0").to_string()
    } else {
        format!("{} / {}", search_match_cursor + 1, search_match_total)
    };
    let display_text = if search_query.is_empty() {
        tr_str(lang_id, "| 搜索当前对话消息…", "| Search messages in this chat…").to_string()
    } else {
        format!("{}|", search_query)
    };

    let is_apple = palette.is_apple_style();
    let bar_bg: Rgba = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            palette.topbar_bg,
            if palette.mode.is_dark() { 0.90 } else { 0.92 },
        )
    } else {
        rgb(palette.topbar_bg).into()
    };
    let div_col: Rgba = if is_apple {
        palette.separator_rgba()
    } else {
        rgb(palette.border_fg).into()
    };

    div()
        .id("stage-in-chat-search-bar")
        .h(px(44.0))
        .w_full()
        .flex_shrink_0()
        .px(px(16.0))
        .bg(bar_bg)
        .border_b_1()
        .border_color(div_col)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                a_focus(this, TopBarUiAction::FocusSearch, cx);
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_1()
                .child(tg_icon(IconName::Search, 15.0, palette.accent))
                .child(
                    div()
                        .flex_1()
                        .px(px(12.0))
                        .py(px(5.0))
                        .rounded(px(if is_apple { 14.0 } else { 8.0 }))
                        .bg(rgb(palette.input_bg))
                        .border_1()
                        .border_color(if is_apple {
                            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.72)
                        } else {
                            rgb(palette.accent).into()
                        })
                        .text_size(px(13.0))
                        .text_color(if search_query.is_empty() {
                            rgb(palette.text_secondary)
                        } else {
                            rgb(palette.text_primary)
                        })
                        .child(display_text),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(palette.text_secondary))
                        .child(match_lbl),
                )
                .child(
                    div()
                        .id("search-prev-btn")
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_prev(this, TopBarUiAction::SearchStep(-1), cx);
                            }),
                        )
                        .text_size(px(12.0))
                        .text_color(rgb(palette.icon_fg))
                        .child("▲"),
                )
                .child(
                    div()
                        .id("search-next-btn")
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_next(this, TopBarUiAction::SearchStep(1), cx);
                            }),
                        )
                        .text_size(px(12.0))
                        .text_color(rgb(palette.icon_fg))
                        .child("▼"),
                )
                .child(
                    div()
                        .id("search-close-btn")
                        .p(px(5.0))
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_close(this, TopBarUiAction::SearchClear, cx);
                            }),
                        )
                        .child(tg_icon(IconName::Close, 14.0, palette.icon_fg)),
                ),
        )
        .into_any_element()
}

pub fn render_stage_more_menu_overlay<V: 'static>(
    is_group_chat: bool,
    can_block_peer: bool,
    is_muted_chat: bool,
    is_pinned_chat: bool,
    is_peer_blocked: bool,
    message_ttl_seconds: u32,
    ttl_submenu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let a_dismiss = on_action.clone();
    let a_mute = on_action.clone();
    let a_prof = on_action.clone();
    let a_srch = on_action.clone();
    let a_sel = on_action.clone();
    let a_ttl_toggle = on_action.clone();
    let a_wall = on_action.clone();
    let a_pin = on_action.clone();
    let a_exp = on_action.clone();
    let a_clr = on_action.clone();
    let a_blk = on_action.clone();
    let a_del = on_action.clone();

    let mute_label = if is_muted_chat {
        tr_str(lang_id, "开启通知", "Unmute notifications")
    } else {
        tr_str(lang_id, "静音通知", "Mute notifications")
    };
    let profile_label = if is_group_chat {
        tr_str(lang_id, "查看群组信息", "View group info")
    } else {
        tr_str(lang_id, "查看个人资料", "View profile")
    };
    let pin_label = if is_pinned_chat {
        tr_str(lang_id, "取消置顶对话", "Unpin chat")
    } else {
        tr_str(lang_id, "置顶对话", "Pin chat")
    };
    let block_label = if is_peer_blocked {
        tr_str(lang_id, "取消屏蔽用户", "Unblock user")
    } else {
        tr_str(lang_id, "屏蔽用户", "Block user")
    };
    let delete_label = if is_group_chat {
        tr_str(lang_id, "退出并删除群组", "Leave group")
    } else {
        tr_str(lang_id, "删除对话", "Delete chat")
    };
    let ttl_badge = format_ttl_badge(message_ttl_seconds, lang_id);
    let hover_bg = palette.menu_hover_bg;
    let pal = *palette;

    let ttl_options: [(u32, &'static str, &'static str); 4] = [
        (0, "关闭", "Off"),
        (86_400, "1 天", "1d"),
        (604_800, "1 周", "1w"),
        (2_592_000, "1 月", "1m"),
    ];

    let is_apple = pal.is_apple_style();
    let menu_bg: Rgba = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(pal.menu_bg, 0.96)
    } else {
        rgb(pal.menu_bg).into()
    };
    let menu_border: Rgba = if is_apple {
        pal.separator_rgba()
    } else {
        rgb(pal.border_fg).into()
    };

    let mut dropdown = div()
        .id("topbar-more-dropdown")
        .absolute()
        .top(px(if is_apple { 52.0 } else { 50.0 }))
        .right(px(12.0))
        .w(px(238.0))
        .p(px(6.0))
        .rounded(px(if is_apple { 14.0 } else { 12.0 }))
        .bg(menu_bg)
        .border_1()
        .border_color(menu_border)
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }),
        )
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(more_menu_item(
            "tb-menu-mute",
            if is_muted_chat { IconName::Speaker } else { IconName::Mute },
            mute_label,
            false,
            palette,
            cx,
            move |this, cx| a_mute(this, TopBarUiAction::ToggleMuteChat, cx),
        ))
        .child(more_menu_item(
            "tb-menu-profile",
            IconName::ContactsUsers,
            profile_label,
            false,
            palette,
            cx,
            move |this, cx| a_prof(this, TopBarUiAction::ViewProfile, cx),
        ))
        .child(more_menu_item(
            "tb-menu-search",
            IconName::Search,
            tr_str(lang_id, "搜索聊天记录", "Search in chat"),
            false,
            palette,
            cx,
            move |this, cx| a_srch(this, TopBarUiAction::ToggleSearch, cx),
        ))
        .child(more_menu_item(
            "tb-menu-select",
            IconName::SelectCheck,
            tr_str(lang_id, "选择消息", "Select messages"),
            false,
            palette,
            cx,
            move |this, cx| a_sel(this, TopBarUiAction::EnterSelectMode, cx),
        ))
        .child(
            div()
                .id("tb-menu-ttl")
                .w_full()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(if is_apple { 8.0 } else { 6.0 }))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hover_bg)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_ttl_toggle(this, TopBarUiAction::ToggleTtlSubmenu, cx);
                    }),
                )
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(tg_icon(IconName::ClockSending, 15.0, pal.icon_fg))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang_id, "自动删除消息", "Auto-delete").to_string()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .child(ttl_badge),
                ),
        )
        .when(ttl_submenu_open, |el| {
            el.child(
                div()
                    .w_full()
                    .px(px(6.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .children(ttl_options.into_iter().map(|(secs, zh_lbl, en_lbl)| {
                        let is_sel = message_ttl_seconds == secs;
                        let a_set = on_action.clone();
                        let lbl = tr_str(lang_id, zh_lbl, en_lbl).to_string();
                        div()
                            .cursor_pointer()
                            .flex_1()
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(if is_sel { rgb(pal.accent) } else { rgb(pal.card_bg) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if is_sel {
                                rgb(pal.active_button_fg)
                            } else {
                                rgb(pal.text_primary)
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_set(this, TopBarUiAction::SetMessageTtl(secs), cx);
                                }),
                            )
                            .child(lbl)
                    })),
            )
        })
        .child(more_menu_item(
            "tb-menu-wallpaper",
            IconName::SunDay,
            tr_str(lang_id, "更换聊天背景", "Change wallpaper"),
            false,
            palette,
            cx,
            move |this, cx| a_wall(this, TopBarUiAction::ChangeWallpaper, cx),
        ))
        .child(more_menu_item(
            "tb-menu-pin-chat",
            IconName::Pin,
            pin_label,
            false,
            palette,
            cx,
            move |this, cx| a_pin(this, TopBarUiAction::TogglePinChat, cx),
        ))
        .child(more_menu_item(
            "tb-menu-export",
            IconName::FileDoc,
            tr_str(lang_id, "导出聊天记录", "Export chat history"),
            false,
            palette,
            cx,
            move |this, cx| a_exp(this, TopBarUiAction::ExportChat, cx),
        ))
        .child(div().h(px(1.0)).my(px(3.0)).bg(menu_border))
        .child(more_menu_item(
            "tb-menu-clear",
            IconName::Edit,
            tr_str(lang_id, "清空聊天记录", "Clear history"),
            false,
            palette,
            cx,
            move |this, cx| a_clr(this, TopBarUiAction::ClearHistory, cx),
        ))
        .when(can_block_peer, |el| {
            el.child(more_menu_item(
                "tb-menu-block",
                IconName::Lock,
                block_label,
                !is_peer_blocked,
                palette,
                cx,
                move |this, cx| a_blk(this, TopBarUiAction::ToggleBlockPeer, cx),
            ))
        })
        .child(more_menu_item(
            "tb-menu-delete-chat",
            IconName::DeleteTrash,
            delete_label,
            true,
            palette,
            cx,
            move |this, cx| a_del(this, TopBarUiAction::DeleteOrLeaveChat, cx),
        ));
    dropdown = if is_apple {
        dropdown.shadow(pal.popover_shadows())
    } else {
        dropdown.shadow_xl()
    };

    div()
        .id("topbar-more-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                a_dismiss(this, TopBarUiAction::DismissTopMoreMenu, cx);
            }),
        )
        .child(deferred(dropdown).with_priority(40))
        .into_any_element()
}

fn more_menu_item<V: 'static>(
    id: &'static str,
    icon: IconName,
    label: &str,
    danger: bool,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let fg = if danger { palette.danger } else { palette.text_primary };
    let icon_fg = if danger { palette.danger } else { palette.icon_fg };
    let hover_bg = palette.menu_hover_bg;
    div()
        .id(id)
        .w_full()
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(hover_bg)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_click(this, cx);
            }),
        )
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(tg_icon(icon, 15.0, icon_fg))
        .child(div().text_size(px(13.0)).text_color(rgb(fg)).child(label.to_string()))
        .into_any_element()
}

fn top_icon_btn<V: 'static>(
    id: &'static str,
    icon: IconName,
    active: bool,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let color = if active { palette.accent } else { palette.icon_fg };
    let hover_bg: Rgba = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            palette.text_primary,
            if palette.mode.is_dark() { 0.10 } else { 0.06 },
        )
    } else {
        rgb(palette.menu_hover_bg).into()
    };
    let btn_sz = if is_apple { 30.0 } else { 34.0 };
    let icon_sz = if is_apple { 16.5 } else { 18.0 };
    div()
        .id(id)
        .w(px(btn_sz))
        .h(px(btn_sz))
        .rounded_full()
        .when(is_apple && active, |el| {
            el.bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.12))
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_click(this, cx);
            }),
        )
        .child(tg_icon(icon, icon_sz, color))
        .into_any_element()
}

pub fn render_pinned_banner_46px<V: 'static>(
    pinned_snippet: &str,
    pinned_sender: &str,
    pinned_thumb_url: Option<&str>,
    pin_count: usize,
    active_pin_idx: usize,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let a_jump = on_action.clone();
    let a_unpin = on_action.clone();
    let dashes = pin_count.clamp(1, 4);
    let active_dash = active_pin_idx % dashes;
    let dash_h = if dashes <= 1 { 26.0 } else if dashes == 2 { 12.0 } else if dashes == 3 { 8.0 } else { 5.5 };
    let thumb_src = pinned_thumb_url.and_then(|u| resolve_image_source(u, None));
    let header_title = if pin_count > 1 {
        format!(
            "{} #{}/{}",
            tr_key_or(lang_id, "lng_pinned_message", "Pinned Message"),
            active_pin_idx + 1,
            pin_count
        )
    } else if !pinned_sender.is_empty() {
        format!("{}: {}", tr_key_or(lang_id, "lng_pinned_message", "Pinned Message"), pinned_sender)
    } else {
        tr_key_or(lang_id, "lng_pinned_message", "Pinned Message").to_string()
    };

    let is_apple = palette.is_apple_style();
    let bar_bg: Rgba = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            palette.topbar_bg,
            if palette.mode.is_dark() { 0.90 } else { 0.92 },
        )
    } else {
        rgb(palette.topbar_bg).into()
    };
    let div_col: Rgba = if is_apple {
        palette.separator_rgba()
    } else {
        rgb(palette.border_fg).into()
    };

    div()
        .h(px(46.0))
        .w_full()
        .px(px(if is_apple { 16.0 } else { 14.0 }))
        .bg(bar_bg)
        .border_b_1()
        .border_color(div_col)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .id("pinned-banner-jump")
                .flex()
                .items_center()
                .gap(px(10.0))
                .flex_1()
                .overflow_hidden()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    a_jump(this, TopBarUiAction::JumpPinned, cx);
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(2.0))
                        .children((0..dashes).map(move |i| {
                            let is_active = i == active_dash;
                            div()
                                .w(px(2.5))
                                .h(px(dash_h))
                                .rounded_full()
                                .bg(if is_active {
                                    rgba((palette.accent << 8) | 0xFF)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.32)
                                })
                        })),
                )
                .when_some(thumb_src, |el, src| {
                    el.child(
                        img(src)
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded(px(4.0))
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(palette.accent))
                                .child(header_title),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(palette.text_secondary))
                                .child(pinned_snippet.to_string()),
                        ),
                ),
        )
        .child(
            div()
                .id("pinned-banner-close")
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    a_unpin(this, TopBarUiAction::UnpinCurrent, cx);
                }))
                .child(tg_icon(IconName::Close, 14.0, palette.icon_fg)),
        )
        .into_any_element()
}

pub fn render_scroll_to_bottom_fab<V: 'static>(
    unread_count: u32,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let mut fab = div()
        .id("scroll-to-bottom-fab")
        .absolute()
        .bottom(px(16.0))
        .right(px(18.0))
        .w(px(if is_apple { 36.0 } else { 42.0 }))
        .h(px(if is_apple { 36.0 } else { 42.0 }))
        .rounded_full()
        .bg(if is_apple {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_bg, 0.94)
        } else {
            rgb(palette.card_bg).into()
        })
        .border_1()
        .border_color(if is_apple {
            palette.separator_rgba()
        } else {
            rgb(palette.border_fg).into()
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(tg_icon(IconName::ChevronDown, if is_apple { 17.0 } else { 20.0 }, palette.icon_fg))
        .when(unread_count > 0, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(-8.0))
                    .right(px(-4.0))
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded_full()
                    .bg(rgb(palette.unread_badge_bg))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(palette.unread_badge_fg))
                    .child(unread_count.to_string()),
            )
        });
    fab = if is_apple {
        fab.shadow(palette.popover_shadows())
    } else {
        fab.shadow_md()
    };
    fab.into_any_element()
}
