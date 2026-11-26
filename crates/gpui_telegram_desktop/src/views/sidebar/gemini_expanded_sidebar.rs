//! Expanded (`288px`) Gemini-inspired unified left sidebar sections.

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::chat_sidebar::{SEARCH_HEIGHT, SEARCH_RADIUS};
use crate::views::nav_rail::NavDestination;
use crate::views::sidebar::gemini_unified_sidebar::{
    format_compact_badge, render_floating_pill_tooltip_at, UnifiedNavSnapshot,
    UnifiedSidebarAction,
};

/// Renders the `52px` unified sidebar titlebar with macOS traffic-light clearance,
/// window drag area, and minimal collapse + compose icons.
pub fn render_expanded_gemini_top_bar<V: 'static>(
    _nav: &UnifiedNavSnapshot,
    is_plus_active: bool,
    hovered_item: Option<&'static str>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let hover_bg = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.06)
        }
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    let active_bg = TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.14);
    let collapse_hovered = hovered_item == Some("collapse");
    let compose_hovered = hovered_item == Some("compose");
    let on_collapse_hover = on_action.clone();
    let on_collapse_click = on_action.clone();
    let on_compose_hover = on_action.clone();
    let on_plus_click = on_action;

    let left_clearance = if cfg!(target_os = "macos") {
        76.0
    } else {
        12.0
    };

    div()
        .h(px(52.0))
        .flex_shrink_0()
        .w_full()
        .pl(px(left_clearance))
        .pr(px(12.0))
        .window_control_area(WindowControlArea::Drag)
        .flex()
        .items_center()
        .justify_end()
        .child(div().flex_1().h_full())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .id("sidebar-collapse-btn")
                        .relative()
                        .cursor_pointer()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(6.0))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_hover(cx.listener(move |this, hovered, _w, cx| {
                            let next = if *hovered { Some("collapse") } else { None };
                            on_collapse_hover(this, UnifiedSidebarAction::SetHover(next), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                on_collapse_click(this, UnifiedSidebarAction::ToggleCollapsed, cx);
                            }),
                        )
                        .child(tg_icon(
                            IconName::SidebarLeftClose,
                            16.0,
                            if collapse_hovered {
                                palette.text_primary
                            } else {
                                palette.icon_fg
                            },
                        ))
                        .when(collapse_hovered, |el| {
                            el.child(render_floating_pill_tooltip_at(
                                tr_str(lang_id, "收起边栏", "Collapse sidebar"),
                                -28.0,
                                32.0,
                                palette,
                            ))
                        }),
                )
                .child(
                    div()
                        .id("sidebar-plus-action-btn")
                        .relative()
                        .cursor_pointer()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(6.0))
                        .when(is_plus_active, |el| el.bg(active_bg))
                        .hover(move |s| if is_plus_active { s } else { s.bg(hover_bg) })
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_hover(cx.listener(move |this, hovered, _w, cx| {
                            let next = if *hovered { Some("compose") } else { None };
                            on_compose_hover(this, UnifiedSidebarAction::SetHover(next), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                on_plus_click(this, UnifiedSidebarAction::TogglePlusMenu, cx);
                            }),
                        )
                        .child(tg_icon(
                            IconName::ComposePen,
                            16.0,
                            if is_plus_active {
                                palette.accent
                            } else if compose_hovered {
                                palette.text_primary
                            } else {
                                palette.icon_fg
                            },
                        ))
                        .when(compose_hovered && !is_plus_active, |el| {
                            el.child(render_floating_pill_tooltip_at(
                                tr_str(lang_id, "发起新对话", "New Chat"),
                                -48.0,
                                32.0,
                                palette,
                            ))
                        }),
                ),
        )
        .into_any_element()
}

/// Renders the inline search bar shown inside the expanded sidebar.
pub fn render_expanded_gemini_search_bar<V: 'static>(
    query: &str,
    is_search_focused: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let hover_bg = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.06)
        }
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    let search_placeholder = if is_search_focused {
        tr_str(lang_id, "| 搜索", "| Search").to_string()
    } else {
        tr_str(lang_id, "搜索", "Search").to_string()
    };
    let search_display = if query.is_empty() {
        search_placeholder
    } else if is_search_focused {
        format!("{}|", query)
    } else {
        query.to_string()
    };

    let on_back = on_action.clone();
    let on_focus = on_action.clone();
    let on_clear = on_action;

    let box_height = if is_apple { 29.0 } else { SEARCH_HEIGHT };
    let box_radius = if is_apple { 8.0 } else { SEARCH_RADIUS };
    let box_bg = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.09)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x808080, 0.14)
        }
    } else {
        rgb(palette.input_bg)
    };
    let box_border = if is_search_focused {
        rgb(palette.accent)
    } else if is_apple {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }
    } else {
        rgb(palette.border_fg)
    };

    div()
        .w_full()
        .flex_shrink_0()
        .px(px(12.0))
        .pb(px(if is_apple { 10.0 } else { 8.0 }))
        .flex()
        .items_center()
        .gap(px(6.0))
        .when(is_search_focused || !query.is_empty(), |el| {
            el.child(
                div()
                    .id("sidebar-search-back-btn")
                    .cursor_pointer()
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded_full()
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            on_back(this, UnifiedSidebarAction::ExitSearch, cx);
                        }),
                    )
                    .child(tg_icon(IconName::BackArrow, 15.0, palette.icon_fg)),
            )
        })
        .child(
            div()
                .id("sidebar-search-box")
                .cursor_pointer()
                .flex_1()
                .min_w(px(0.0))
                .h(px(box_height))
                .bg(box_bg)
                .border_1()
                .border_color(box_border)
                .rounded(px(box_radius))
                .px(px(9.0))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        on_focus(this, UnifiedSidebarAction::FocusSearch, cx);
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .overflow_hidden()
                        .child(tg_icon(
                            IconName::Search,
                            13.5,
                            if is_search_focused {
                                palette.accent
                            } else {
                                palette.text_secondary
                            },
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(13.0))
                                .text_color(if query.is_empty() {
                                    rgb(palette.text_secondary)
                                } else {
                                    rgb(palette.text_primary)
                                })
                                .child(search_display),
                        ),
                )
                .when(is_search_focused || !query.is_empty(), |el| {
                    el.child(
                        div()
                            .id("sidebar-search-clear-btn")
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex_shrink_0()
                            .rounded_full()
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev, _w, cx| {
                                    cx.stop_propagation();
                                    on_clear(this, UnifiedSidebarAction::ClearSearch, cx);
                                }),
                            )
                            .child(tg_icon(IconName::Close, 11.0, palette.text_secondary)),
                    )
                }),
        )
        .into_any_element()
}

/// Renders the prominent first-class left-sidebar entry pill for Code Scene ("代码场景")
/// directly below the search bar without polluting `ChatSidebar::dialogs`.
pub fn render_expanded_code_scene_entry<V: 'static>(
    nav: &UnifiedNavSnapshot,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + 'static,
) -> AnyElement {
    let is_active = nav.active_destination == NavDestination::CodeScene;
    let is_apple = palette.is_apple_style();
    let is_dark = palette.mode.is_dark();
    let bg = if is_active {
        TdesktopThemePalette::rgba_with_alpha(0xFE2C55, if is_dark { 0.24 } else { 0.14 })
    } else if is_apple {
        if is_dark {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.06)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.045)
        }
    } else {
        rgb(palette.input_bg).into()
    };
    let hover_bg = if is_active {
        TdesktopThemePalette::rgba_with_alpha(0xFE2C55, if is_dark { 0.30 } else { 0.20 })
    } else if is_apple {
        if is_dark {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.11)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.08)
        }
    } else {
        rgb(palette.dialogs_hover_bg).into()
    };
    let border_col = if is_active {
        TdesktopThemePalette::rgba_with_alpha(0xFE2C55, 0.65)
    } else if is_apple {
        palette.separator_rgba()
    } else {
        rgb(palette.border_fg).into()
    };
    let icon_fg = if is_active { 0xFE2C55 } else { palette.accent };

    div()
        .w_full()
        .px(px(12.0))
        .pb(px(8.0))
        .flex_shrink_0()
        .child(
            div()
                .id("sidebar-code-scene-entry")
                .w_full()
                .h(px(38.0))
                .px(px(11.0))
                .rounded(px(10.0))
                .bg(bg)
                .border_1()
                .border_color(border_col)
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        on_action(
                            this,
                            UnifiedSidebarAction::SelectDestination(NavDestination::CodeScene),
                            cx,
                        );
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .min_w(px(0.0))
                        .child(
                            div()
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(7.0))
                                .bg(if is_active {
                                    TdesktopThemePalette::rgba_with_alpha(0xFE2C55, 0.22)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16)
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .child(tg_icon(IconName::CodeScene, 15.0, icon_fg)),
                        )
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(if is_active {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::SEMIBOLD
                                })
                                .text_color(if is_active {
                                    rgb(0xFE2C55)
                                } else {
                                    rgb(palette.text_primary)
                                })
                                .child(tr_str(lang_id, "代码场景", "Code Scene")),
                        ),
                )
                .child(
                    div()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(999.0))
                        .bg(if is_active {
                            rgba(0xFE2C55DD)
                        } else {
                            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.14)
                        })
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if is_active {
                            rgba(0xFFFFFFFF)
                        } else {
                            rgb(palette.text_secondary)
                        })
                        .child(tr_str(lang_id, "视频·动画·游戏", "AI Feed")),
                ),
        )
        .into_any_element()
}

fn render_expanded_nav_row<V: 'static>(
    id: &'static str,
    icon: IconName,
    label: &str,
    is_active: bool,
    unread_badge: u32,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + 'static,
    action: UnifiedSidebarAction,
) -> AnyElement {
    let active_bg = TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.18);
    let hover_bg = rgb(palette.dialogs_hover_bg);
    let icon_color = if is_active {
        palette.accent
    } else {
        palette.icon_fg
    };
    let text_color = if is_active {
        rgb(palette.accent)
    } else {
        rgb(palette.text_primary)
    };

    div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .h(px(31.0))
        .px(px(12.0))
        .rounded_full()
        .when(is_active, |el| el.bg(active_bg))
        .hover(move |s| if is_active { s } else { s.bg(hover_bg) })
        .flex()
        .items_center()
        .justify_between()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev, _w, cx| {
                on_action(this, action, cx);
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .overflow_hidden()
                .child(tg_icon(icon, 16.0, icon_color))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(if is_active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(text_color)
                        .child(label.to_string()),
                ),
        )
        .when(unread_badge > 0, |el| {
            el.child(
                div()
                    .h(px(17.0))
                    .min_w(px(18.0))
                    .px(px(5.0))
                    .rounded_full()
                    .bg(rgb(palette.unread_badge_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(palette.unread_badge_fg))
                    .child(format_compact_badge(unread_badge)),
            )
        })
        .into_any_element()
}

/// Renders the Gemini-style primary action button, primary navigation pills, and expandable More rows.
pub fn render_expanded_gemini_nav_section<V: 'static>(
    nav: &UnifiedNavSnapshot,
    is_plus_open: bool,
    has_modal: bool,
    is_nav_more_expanded: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let hover_bg = rgb(palette.dialogs_hover_bg);
    let accent_hover = rgb(palette.accent_hover);
    let plus_active = is_plus_open || has_modal;
    let effective_more_open = is_nav_more_expanded
        || matches!(
            nav.active_destination,
            NavDestination::TalkMode
                | NavDestination::RemoteComputer
                | NavDestination::SavedMessages
        );

    let on_plus = on_action.clone();
    let on_more = on_action.clone();

    let primary_items: [(&'static str, NavDestination, IconName, &'static str, &'static str, u32); 3] = [
        (
            "sidebar-nav-chats",
            NavDestination::Chats,
            IconName::FolderAll,
            "全部会话",
            "All Chats",
            nav.chats_unread,
        ),
        (
            "sidebar-nav-tasks",
            NavDestination::TaskCenter,
            IconName::Poll,
            "任务中心",
            "Task Center",
            nav.tasks_unread,
        ),
        (
            "sidebar-nav-mcp",
            NavDestination::McpTools,
            IconName::TerminalMcp,
            "MCP 工具",
            "MCP Tools",
            nav.mcp_unread,
        ),
    ];

    div()
        .w_full()
        .flex_shrink_0()
        .px(px(10.0))
        .pb(px(6.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .id("sidebar-plus-action-btn")
                .cursor_pointer()
                .w_full()
                .h(px(36.0))
                .mb(px(4.0))
                .px(px(12.0))
                .rounded_full()
                .bg(rgb(if plus_active {
                    palette.accent
                } else {
                    palette.input_bg
                }))
                .hover(move |s| {
                    s.bg(if plus_active {
                        accent_hover
                    } else {
                        hover_bg
                    })
                })
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        on_plus(this, UnifiedSidebarAction::TogglePlusMenu, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(tg_icon(
                            IconName::ComposePen,
                            16.0,
                            if plus_active {
                                palette.active_button_fg
                            } else {
                                palette.text_primary
                            },
                        ))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if plus_active {
                                    palette.active_button_fg
                                } else {
                                    palette.text_primary
                                }))
                                .child(tr_str(lang_id, "发起新对话", "New Chat")),
                        ),
                )
                .child(tg_icon(
                    IconName::ChevronDown,
                    12.0,
                    if plus_active {
                        palette.active_button_fg
                    } else {
                        palette.icon_fg
                    },
                )),
        )
        .children(
            primary_items
                .into_iter()
                .map(|(id, dest, icon, zh, en, badge)| {
                    let is_active = if dest == NavDestination::Chats {
                        matches!(
                            nav.active_destination,
                            NavDestination::Chats | NavDestination::Folders
                        )
                    } else {
                        nav.active_destination == dest
                    };
                    render_expanded_nav_row(
                        id,
                        icon,
                        tr_str(lang_id, zh, en),
                        is_active,
                        badge,
                        palette,
                        cx,
                        on_action.clone(),
                        UnifiedSidebarAction::SelectDestination(dest),
                    )
                }),
        )
        .child(
            div()
                .id("sidebar-nav-more-btn")
                .cursor_pointer()
                .w_full()
                .h(px(29.0))
                .px(px(12.0))
                .rounded_full()
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        on_more(this, UnifiedSidebarAction::ToggleNavMore, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div()
                                .w(px(16.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(palette.icon_fg))
                                .child(if effective_more_open { "⌃" } else { "⋯" }),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(palette.text_secondary))
                                .child(if effective_more_open {
                                    tr_str(lang_id, "收起", "Less")
                                } else {
                                    tr_str(lang_id, "更多", "More")
                                }),
                        ),
                ),
        )
        .when(effective_more_open, |el| {
            el.child(render_expanded_nav_row(
                "sidebar-nav-talk",
                IconName::Mic,
                tr_str(lang_id, "语音对话", "Talk Mode"),
                nav.active_destination == NavDestination::TalkMode,
                0,
                palette,
                cx,
                on_action.clone(),
                UnifiedSidebarAction::SelectDestination(NavDestination::TalkMode),
            ))
            .child(render_expanded_nav_row(
                "sidebar-nav-remote",
                IconName::ComputerDesktop,
                tr_str(lang_id, "远程电脑", "Remote Computer"),
                nav.active_destination == NavDestination::RemoteComputer,
                0,
                palette,
                cx,
                on_action.clone(),
                UnifiedSidebarAction::SelectDestination(NavDestination::RemoteComputer),
            ))
            .child(render_expanded_nav_row(
                "sidebar-nav-saved",
                IconName::SavedMessages,
                tr_str(lang_id, "收藏夹", "Saved Messages"),
                nav.active_destination == NavDestination::SavedMessages,
                nav.saved_unread,
                palette,
                cx,
                on_action.clone(),
                UnifiedSidebarAction::SelectDestination(NavDestination::SavedMessages),
            ))
            .child(render_expanded_nav_row(
                "sidebar-nav-contacts",
                IconName::ContactsUsers,
                tr_str(lang_id, "联系人", "Contacts"),
                false,
                0,
                palette,
                cx,
                on_action.clone(),
                UnifiedSidebarAction::OpenContacts,
            ))
        })
        .into_any_element()
}

/// Renders the streamlined `44px` expanded sidebar bottom utility dock with account trigger on the
/// left and minimal workspace action icons (Tasks, Remote, Contacts, Calls, Settings) on the right.
pub fn render_expanded_gemini_footer<V: 'static>(
    nav: &UnifiedNavSnapshot,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let hover_bg = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.06)
        }
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    let active_bg = TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.14);
    let avatar_grad = palette.peer_avatar_gradient(1);
    let display_name = if nav.account_name.trim().is_empty() {
        tr_str(lang_id, "我的账号", "My Account").to_string()
    } else {
        nav.account_name.clone()
    };

    let render_dock_icon_btn =
        |id: &'static str,
         icon: IconName,
         is_active: bool,
         badge: u32,
         action: UnifiedSidebarAction| {
            let on_btn = on_action.clone();
            let fg = if is_active {
                palette.accent
            } else {
                palette.icon_fg
            };
            div()
                .id(id)
                .relative()
                .cursor_pointer()
                .w(px(26.0))
                .h(px(26.0))
                .rounded(px(6.0))
                .when(is_active, |el| el.bg(active_bg))
                .hover(move |s| if is_active { s } else { s.bg(hover_bg) })
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        on_btn(this, action, cx);
                    }),
                )
                .child(tg_icon(icon, 14.5, fg))
                .when(badge > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(2.0))
                            .right(px(2.0))
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgb(palette.unread_badge_bg)),
                    )
                })
        };

    let tasks_target = if nav.active_destination == NavDestination::TaskCenter {
        NavDestination::Chats
    } else {
        NavDestination::TaskCenter
    };
    let remote_target = if nav.active_destination == NavDestination::RemoteComputer {
        NavDestination::Chats
    } else {
        NavDestination::RemoteComputer
    };
    let settings_target = if nav.active_destination == NavDestination::Settings {
        NavDestination::Chats
    } else {
        NavDestination::Settings
    };

    let on_account = on_action.clone();

    div()
        .h(px(44.0))
        .flex_shrink_0()
        .w_full()
        .px(px(10.0))
        .border_t_1()
        .border_color(if is_apple {
            palette.separator_rgba()
        } else {
            rgb(palette.border_fg)
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(4.0))
        .child(
            div()
                .id("sidebar-account-footer-btn")
                .cursor_pointer()
                .flex_1()
                .min_w(px(0.0))
                .px(px(5.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .gap(px(7.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        on_account(this, UnifiedSidebarAction::OpenMainMenu, cx);
                    }),
                )
                .child(
                    div()
                        .relative()
                        .w(px(24.0))
                        .h(px(24.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(rgb(avatar_grad.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(nav.account_initials.clone())
                        .child(
                            div()
                                .absolute()
                                .bottom(px(-1.0))
                                .right(px(-1.0))
                                .w(px(7.5))
                                .h(px(7.5))
                                .rounded_full()
                                .bg(if nav.is_connected {
                                    rgb(palette.online_dot_fg)
                                } else {
                                    rgb(palette.muted_badge_bg)
                                })
                                .border_1()
                                .border_color(rgb(palette.dialogs_bg)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(palette.text_primary))
                        .child(display_name),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .flex_shrink_0()
                .gap(px(2.0))
                .child(render_dock_icon_btn(
                    "top-action-tasks",
                    IconName::Poll,
                    nav.active_destination == NavDestination::TaskCenter,
                    nav.tasks_unread,
                    UnifiedSidebarAction::SelectDestination(tasks_target),
                ))
                .child(render_dock_icon_btn(
                    "top-action-remote",
                    IconName::ComputerDesktop,
                    nav.active_destination == NavDestination::RemoteComputer,
                    0,
                    UnifiedSidebarAction::SelectDestination(remote_target),
                ))
                .child(render_dock_icon_btn(
                    "top-action-contacts",
                    IconName::ContactsUsers,
                    false,
                    0,
                    UnifiedSidebarAction::OpenContacts,
                ))
                .child(render_dock_icon_btn(
                    "top-action-settings",
                    IconName::SettingsGear,
                    nav.active_destination == NavDestination::Settings,
                    0,
                    UnifiedSidebarAction::SelectDestination(settings_target),
                )),
        )
        .into_any_element()
}
