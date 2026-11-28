//! 1:1 Telegram Desktop sidebar header, compose menu, stories strip, and archived header.

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::i18n::tr_str;
use crate::theme::{tg_icon, tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::chat_sidebar::{
    AddContactKind, DialogItemModel, SEARCH_HEADER_HEIGHT, SEARCH_HEIGHT, SEARCH_RADIUS,
};
use crate::views::sidebar::dialogs_row_view::{
    render_openbot_dialog_avatar_with_badge, resolve_dialog_felt_badge_state,
    resolve_dialog_preview_priority, SidebarDialogMeta,
};
use crate::views::sidebar::ui::dialogs_stories_list::DialogsStoriesListUi;
use crate::views::sidebar::ui::dialogs_suggestions::DialogsSuggestionsUi;

pub fn render_sidebar_header<V: 'static>(
    query: &str,
    is_search_focused: bool,
    is_plus_open: bool,
    has_modal: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_menu_or_back: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_focus_search: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_clear_search: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_toggle_plus: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let icon_color = palette.icon_fg;
    let hover_bg = rgb(palette.dialogs_hover_bg);
    let left_icon = if is_search_focused { IconName::BackArrow } else { IconName::HamburgerMenu };

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

    div()
        .h(px(SEARCH_HEADER_HEIGHT))
        .flex_shrink_0()
        .w_full()
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .id("sidebar-hamburger-btn")
                .cursor_pointer()
                .w(px(32.0))
                .h(px(32.0))
                .rounded_full()
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    on_menu_or_back(this, cx);
                }))
                .child(tg_icon(left_icon, 18.0, icon_color)),
        )
        .child(
            div()
                .id("sidebar-search-box")
                .cursor_pointer()
                .flex_1()
                .h(px(SEARCH_HEIGHT))
                .bg(rgb(palette.composer_bg))
                .border_1()
                .border_color(if is_search_focused {
                    rgb(palette.accent)
                } else {
                    rgb(palette.border_fg)
                })
                .rounded(px(SEARCH_RADIUS))
                .px(px(10.0))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    on_focus_search(this, cx);
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .overflow_hidden()
                        .child(tg_icon(
                            IconName::Search,
                            14.0,
                            if is_search_focused { palette.accent } else { palette.icon_fg },
                        ))
                        .child(
                            div()
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
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded_full()
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                on_clear_search(this, cx);
                            }))
                            .child(tg_icon(IconName::Close, 12.0, icon_color)),
                    )
                }),
        )
        .child(
            div()
                .id("sidebar-plus-action-btn")
                .cursor_pointer()
                .w(px(SEARCH_HEIGHT))
                .h(px(SEARCH_HEIGHT))
                .flex_shrink_0()
                .rounded_full()
                .bg(if is_plus_open || has_modal {
                    rgb(palette.accent)
                } else {
                    rgb(palette.composer_bg)
                })
                .hover({
                    let accent_hover = rgb(palette.accent_hover);
                    move |s| s.bg(accent_hover)
                })
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    on_toggle_plus(this, cx);
                }))
                .child(tg_icon(
                    IconName::ComposePen,
                    16.0,
                    if is_plus_open || has_modal { palette.active_button_fg } else { palette.icon_fg },
                )),
        )
        .into_any_element()
}

pub fn render_plus_dropdown<V: 'static>(
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_dismiss: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_add_contact: impl Fn(&mut V, AddContactKind, &mut Context<V>) + Clone + 'static,
    on_create_group: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_create_channel: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_open_contacts: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let pal = *palette;
    let is_apple = pal.is_apple_style();
    let hover_bg = if is_apple {
        if pal.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.05)
        }
    } else {
        rgb(pal.menu_hover_bg)
    };
    let menu_bg = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            pal.menu_bg,
            if pal.mode.is_dark() { 0.94 } else { 0.96 },
        )
    } else {
        rgb(pal.menu_bg)
    };
    let menu_border = if is_apple {
        pal.separator_rgba()
    } else {
        rgb(pal.border_fg)
    };

    let menu_row = |id: &'static str, icon: IconName, title: String, subtitle: String| {
        div()
            .id(id)
            .cursor_pointer()
            .w_full()
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(7.0))
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(tg_icon(icon, 16.0, pal.accent))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(subtitle),
                            ),
                    ),
            )
            .child(tg_icon(IconName::ChevronRight, 11.5, pal.icon_fg))
    };

    let add_cb = on_add_contact.clone();
    let friend_cb = on_add_contact;
    let popup = div()
        .id("sidebar-plus-floating-menu")
        .absolute()
        .top(px(50.0))
        .left(px(10.0))
        .right(px(10.0))
        .p(px(6.0))
        .rounded(px(12.0))
        .bg(menu_bg)
        .border_1()
        .border_color(menu_border)
        .shadow(pal.popover_shadows())
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
        .child(
            menu_row(
                "plus-menu-create-group",
                IconName::GroupUsers,
                tr_str(lang_id, "新建群组", "New Group").to_string(),
                tr_str(lang_id, "创建多人或协同群聊", "Create a group chat").to_string(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, _w, cx| {
                    cx.stop_propagation();
                    on_create_group(this, cx);
                }),
            ),
        )
        .child(
            menu_row(
                "plus-menu-create-channel",
                IconName::ChannelMegaphone,
                tr_str(lang_id, "新建频道", "New Channel").to_string(),
                tr_str(lang_id, "面向订阅者广播消息", "Broadcast to unlimited subscribers").to_string(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, _w, cx| {
                    cx.stop_propagation();
                    on_create_channel(this, cx);
                }),
            ),
        )
        .child(
            menu_row(
                "plus-menu-add-friend",
                IconName::ContactsUsers,
                tr_str(lang_id, "添加好友", "Add Contact").to_string(),
                tr_str(lang_id, "通过手机号或 @用户名添加好友", "Add friend by phone or @username").to_string(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, _w, cx| {
                    cx.stop_propagation();
                    friend_cb(this, AddContactKind::Friend, cx);
                }),
            ),
        )
        .child(
            menu_row(
                "plus-menu-add-contact",
                IconName::BotRobot,
                tr_str(lang_id, "创建 AI Bot", "Create AI Bot").to_string(),
                tr_str(lang_id, "自定义智能体提示词与用户名", "Create AI assistant bot").to_string(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, _w, cx| {
                    cx.stop_propagation();
                    add_cb(this, AddContactKind::AiBot, cx);
                }),
            ),
        )
        .child(div().h(px(1.0)).mx(px(6.0)).my(px(3.0)).bg(menu_border))
        .child(
            menu_row(
                "plus-menu-contacts-modal",
                IconName::ContactsUsers,
                tr_str(lang_id, "联系人列表", "Contacts").to_string(),
                tr_str(lang_id, "查看全部联系人并发起对话", "Browse saved contacts").to_string(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, _w, cx| {
                    cx.stop_propagation();
                    on_open_contacts(this, cx);
                }),
            ),
        );

    div()
        .id("sidebar-plus-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev, _w, cx| {
                cx.stop_propagation();
                on_dismiss(this, cx);
            }),
        )
        .child(deferred(popup).with_priority(35))
        .into_any_element()
}

/// Renders OpenBot's compact centered 3-column `SidebarPinnedGroup` (`max-width: 236px`,
/// `72x102px` tiles, `56x56px` 3D plush/cluster avatars + felt status badges, `8px` gap).
/// Returns `None` when `pinned_dialogs` is empty (zero empty chrome or divider).
pub fn render_pinned_chats_grid<V: 'static>(
    pinned_dialogs: &[(DialogItemModel, SidebarDialogMeta)],
    active_id: Option<u64>,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_select: impl Fn(&mut V, u64, &mut Context<V>) + Clone + 'static,
    on_context: impl Fn(&mut V, u64, Point<Pixels>, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    render_pinned_chats_grid_localized(
        pinned_dialogs,
        active_id,
        palette,
        "zh-CN",
        cx,
        on_select,
        on_context,
    )
}

/// Single-source-of-truth layout snapshot for OpenBot's compact `3-col 72x102px` pinned grid (D4 fix).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenBotPinnedGridLayout {
    pub visible: bool,
    pub legacy_oversized_circles: bool,
    pub is_compact_grid: bool,
    pub columns: usize,
    pub max_width_px: f32,
    pub tile_w_px: f32,
    pub tile_h_px: f32,
    pub avatar_px: f32,
    pub gap_px: f32,
}

pub fn compute_openbot_pinned_grid_layout(
    pinned_count: usize,
    is_compact: bool,
    palette: &TdesktopThemePalette,
) -> OpenBotPinnedGridLayout {
    let visible = pinned_count > 0;
    let is_apple = palette.is_apple_style();
    let (columns, max_width_px, tile_w_px, tile_h_px, avatar_px, gap_px) = if is_compact {
        (1, 64.0, 60.0, 60.0, palette.scaled_px(48.0), 4.0)
    } else {
        (3, 236.0, 72.0, 102.0, palette.scaled_px(56.0), 8.0)
    };
    OpenBotPinnedGridLayout {
        visible,
        legacy_oversized_circles: !is_apple || avatar_px > 56.0,
        is_compact_grid: is_apple && tile_w_px <= 72.0 && tile_h_px <= 102.0,
        columns,
        max_width_px,
        tile_w_px,
        tile_h_px,
        avatar_px,
        gap_px,
    }
}

/// Single-source-of-truth layout snapshot for the empty sidebar workspace CTA (`UI-F06-04`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarEmptyStateLayout {
    pub visible: bool,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub cta_label: &'static str,
}

pub fn compute_sidebar_empty_state_layout(
    row_count: usize,
    palette: &TdesktopThemePalette,
    lang_id: &str,
) -> SidebarEmptyStateLayout {
    let visible = row_count == 0;
    if palette.is_apple_style() {
        SidebarEmptyStateLayout {
            visible,
            title: tr_str(lang_id, "暂无助手或对话", "No agents or chats yet"),
            subtitle: tr_str(
                lang_id,
                "创建你的第一个 AI 助手开始工作区对话",
                "Create your first AI agent to start chatting",
            ),
            cta_label: tr_str(lang_id, "创建第一个助手", "Create First Agent"),
        }
    } else {
        SidebarEmptyStateLayout {
            visible,
            title: tr_str(lang_id, "暂无对话", "No chats yet"),
            subtitle: tr_str(
                lang_id,
                "点击右上角图标添加联系人或创建群组",
                "Start a chat or connect your workspace",
            ),
            cta_label: tr_str(lang_id, "新建对话", "New Chat"),
        }
    }
}

/// Localized variant of [`render_pinned_chats_grid`] for OpenBot `SidebarPinnedGroup`.
#[allow(clippy::too_many_arguments)]
pub fn render_pinned_chats_grid_localized<V: 'static>(
    pinned_dialogs: &[(DialogItemModel, SidebarDialogMeta)],
    active_id: Option<u64>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_select: impl Fn(&mut V, u64, &mut Context<V>) + Clone + 'static,
    on_context: impl Fn(&mut V, u64, Point<Pixels>, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    let grid = compute_openbot_pinned_grid_layout(pinned_dialogs.len(), false, palette);
    if !grid.visible {
        return None;
    }
    let ob = palette.openbot_palette();
    let is_dark = palette.mode.is_dark();
    let shown = &pinned_dialogs[..pinned_dialogs.len().min(9)];
    let avatar_size_px = grid.avatar_px;
    let hover_bg = ob.hover();
    let active_bg = if is_dark { ob.bg_surface() } else { rgb(0xE8E8EB) };
    let text_pri = ob.text_primary();

    let tiles: Vec<AnyElement> = shown
        .iter()
        .map(|(item, meta)| {
            let did = item.id;
            let is_selected = active_id == Some(did);
            let sel_cb = on_select.clone();
            let ctx_cb = on_context.clone();
            let tile_surface_hex = if is_selected {
                if is_dark { 0x212121 } else { 0xE8E8EB }
            } else if is_dark {
                0x1A1A1A
            } else {
                0xF5F5F6
            };
            let badge_state = resolve_dialog_felt_badge_state(
                item,
                meta,
                &resolve_dialog_preview_priority(item, meta, lang_id),
            );
            let avatar = render_openbot_dialog_avatar_with_badge(
                item,
                meta,
                palette,
                avatar_size_px,
                tile_surface_hex,
                is_selected,
                badge_state,
            );

            div()
                .id(("sidebar-pinned-tile", did))
                .cursor_pointer()
                .w(px(grid.tile_w_px))
                .h(px(grid.tile_h_px))
                .flex_shrink_0()
                .px(px(2.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .when(is_selected, |el| el.bg(active_bg))
                .hover(move |s| if is_selected { s } else { s.bg(hover_bg) })
                .flex()
                .flex_col()
                .items_center()
                .justify_start()
                .gap(px(5.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        sel_cb(this, did, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        ctx_cb(this, did, ev.position, cx);
                    }),
                )
                .child(avatar)
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_center()
                                .text_size(px(13.0))
                                .line_height(px(16.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_pri)
                                .child(item.peer_name.clone()),
                        ),
                )
                .into_any_element()
        })
        .collect();

    Some(
        div()
            .id("sidebar-pinned-chats-grid")
            .w_full()
            .flex_shrink_0()
            .pt(px(4.0))
            .pb(px(4.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(grid.max_width_px))
                    .mx_auto()
                    .p(px(2.0))
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .content_start()
                    .gap(px(grid.gap_px))
                    .children(tiles),
            )
            .into_any_element(),
    )
}

pub fn render_suggestion_bar<V: 'static>(
    suggestions: &DialogsSuggestionsUi,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_dismiss: impl Fn(&mut V, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    if !suggestions.is_visible {
        return None;
    }
    let item = suggestions.current_item.as_ref()?;
    let dismiss_cta = on_dismiss.clone();
    let is_apple = palette.is_apple_style();
    Some(
        div()
            .h(px(37.0))
            .flex_shrink_0()
            .w_full()
            .px(px(12.0))
            .when(!is_apple, |el| el.bg(rgb(palette.composer_bg)))
            .border_b_1()
            .border_color(if is_apple {
                palette.separator_rgba()
            } else {
                rgb(palette.border_fg)
            })
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .child(tg_icon(IconName::PremiumStar, 13.0, palette.accent))
                    .child(div().text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(rgb(palette.text_primary)).child(item.title.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .when_some(item.button_text.clone(), |el, btn| {
                        el.child(
                            div()
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded_full()
                                .bg(rgb(palette.accent))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.active_button_fg))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                                    dismiss_cta(this, cx);
                                }))
                                .child(btn),
                        )
                    })
                    .child(
                        div()
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                                on_dismiss(this, cx);
                            }))
                            .child(tg_icon(IconName::Close, 12.0, palette.icon_fg)),
                    ),
            )
            .into_any_element(),
    )
}

pub fn render_stories_strip<V: 'static>(
    stories: &DialogsStoriesListUi,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_toggle_expand: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_click_story: impl Fn(&mut V, u64, String, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    if stories.elements.is_empty() {
        return None;
    }
    let is_apple = palette.is_apple_style();
    let limit = if stories.is_expanded { stories.elements.len() } else { 3 };
    let shown: Vec<_> = stories.elements.iter().take(limit).cloned().collect();
    let story_buttons: Vec<AnyElement> = shown
        .into_iter()
        .map(|s| {
            let sid = s.id;
            let peer = s.peer_name.clone();
            let cb = on_click_story.clone();
            div()
                .id(("sidebar-story-pill", sid))
                .cursor_pointer()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.0))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    cb(this, sid, peer.clone(), cx);
                }))
                .child(
                    div()
                        .w(px(30.0))
                        .h(px(30.0))
                        .rounded_full()
                        .bg(rgb(s.avatar_color))
                        .border_2()
                        .border_color(if s.unread_count > 0 { rgb(palette.accent) } else { rgb(palette.border_fg) })
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(s.avatar_initials),
                )
                .into_any_element()
        })
        .collect();

    Some(
        div()
            .h(px(44.0))
            .flex_shrink_0()
            .w_full()
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(if is_apple {
                palette.separator_rgba()
            } else {
                rgb(palette.border_fg)
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().text_size(px(11.0)).font_weight(FontWeight::BOLD).text_color(rgb(palette.text_secondary)).child(tr_str(lang_id, "动态", "STORIES")))
                    .children(story_buttons),
            )
            .child(
                div()
                    .id("sidebar-stories-expand-btn")
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                        on_toggle_expand(this, cx);
                    }))
                    .child(tg_icon(
                        if stories.is_expanded { IconName::ChevronDown } else { IconName::ChevronRight },
                        12.0,
                        palette.icon_fg,
                    )),
            )
            .into_any_element(),
    )
}

pub fn render_archived_header_row<V: 'static>(
    archived_dialogs: &[DialogItemModel],
    is_expanded: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_toggle_archived: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Option<AnyElement> {
    if archived_dialogs.is_empty() {
        return None;
    }
    let is_apple = palette.is_apple_style();
    let ob = palette.openbot_palette();
    let preview: String = archived_dialogs
        .iter()
        .take(3)
        .map(|d| d.peer_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let total_unread: u32 = archived_dialogs.iter().map(|d| d.unread_count).sum();
    let chev = if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight };
    let hover = if is_apple {
        ob.hover()
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    Some(
        div()
            .id("sidebar-archived-chats-row")
            .cursor_pointer()
            .h(px(56.0))
            .flex_shrink_0()
            .w_full()
            .px(px(8.0))
            .py(px(6.0))
            .when(is_apple, |el| el.rounded(px(8.0)))
            .when(!is_apple, |el| {
                el.bg(rgb(palette.dialogs_bg))
                    .border_b_1()
                    .border_color(rgb(palette.border_fg))
            })
            .hover(move |s| s.bg(hover))
            .flex()
            .items_center()
            .gap(px(10.0))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                on_toggle_archived(this, cx);
            }))
            .child(
                div()
                    .w(px(40.0))
                    .h(px(40.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(if is_apple { ob.bg_control_active() } else { rgb(palette.muted_badge_bg) })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon_rgba(
                        IconName::ArchivedChats,
                        18.0,
                        if is_apple { ob.text_secondary() } else { rgb(0xFFFFFF) },
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if is_apple { ob.text_primary() } else { rgb(palette.text_primary) })
                                    .child(tr_str(lang_id, "已归档对话", "Archived Chats")),
                            )
                            .child(tg_icon_rgba(
                                chev,
                                12.0,
                                if is_apple { ob.text_muted() } else { rgb(palette.icon_fg) },
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.5))
                                    .text_color(if is_apple { ob.text_muted() } else { rgb(palette.text_secondary) })
                                    .child(preview),
                            )
                            .when(total_unread > 0, |el| {
                                el.child(
                                    div()
                                        .h(px(18.0))
                                        .min_w(px(18.0))
                                        .px(px(5.0))
                                        .rounded_full()
                                        .bg(if is_apple { ob.bg_control_active() } else { rgb(palette.muted_badge_bg) })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if is_apple { ob.text_secondary() } else { rgb(palette.unread_badge_fg) })
                                        .child(format!("{}", total_unread)),
                                )
                            }),
                    ),
            )
            .into_any_element(),
    )
}
