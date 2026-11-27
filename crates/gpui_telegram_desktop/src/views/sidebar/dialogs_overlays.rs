//! 1:1 Telegram Desktop folder strip, forum topics tree, and context menu overlays.

use gpui::prelude::FluentBuilder;
use gpui::*;

use std::collections::HashSet;

use crate::i18n::tr_str;
use crate::theme::{tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::chat_sidebar::OPENBOT_TABS_HEIGHT;
use crate::views::sidebar::dialogs_quick_action::{DialogsQuickAction, QuickDialogAction};
use crate::views::sidebar::dialogs_row_view::SidebarContextAction;
use crate::views::sidebar::dialogs_three_state_icon::DialogsThreeStateIcon;
use crate::views::sidebar::openbot_sidebar_chrome::{openbot_vector_icon, OpenBotChromeIcon};
use crate::views::sidebar::ui::dialogs_quick_action::QuickDialogActionType;
use crate::views::sidebar::ui::dialogs_quick_action_context::DialogsQuickActionContextUi;
use crate::views::sidebar::ui::dialogs_topics_view::DialogsTopicsViewUi;

pub fn localize_folder_label(folder: &str, lang_id: &str) -> String {
    match folder {
        "All" => tr_str(lang_id, "全部", "All").to_string(),
        "Bots" => tr_str(lang_id, "机器人", "Bots").to_string(),
        "Groups" => tr_str(lang_id, "群组", "Groups").to_string(),
        "Channels" => tr_str(lang_id, "频道", "Channels").to_string(),
        "Personal" => tr_str(lang_id, "私聊", "Personal").to_string(),
        "Work" => tr_str(lang_id, "工作", "Work").to_string(),
        "Unread" => tr_str(lang_id, "未读", "Unread").to_string(),
        other => other.to_string(),
    }
}

/// Renders OpenBot's exact `32px` flat underline `SidebarTabs` (`全部` + `未读` + visible folders)
/// with the right-aligned `28x28` `SlidersHorizontal` tab-visibility trigger.
#[allow(clippy::too_many_arguments)]
pub fn render_openbot_sidebar_tabs<V: 'static>(
    folders: &[(String, u32)],
    total_unread_count: u32,
    active_folder: &str,
    hidden_tab_ids: &HashSet<String>,
    is_edit_menu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_select_folder: impl Fn(&mut V, String, &mut Context<V>) + Clone + 'static,
    on_toggle_edit_menu: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let text_pri = ob.text_primary();
    let text_sec = ob.text_secondary();
    let text_muted = ob.text_muted();
    let hover_bg = ob.hover();
    let badge_idle_bg = ob.bg_control_active();
    let badge_active_bg = ob.accent();

    // Always lead with "All", then "Unread" (visible by default unless hidden), then other visible folders.
    let mut visible_tabs: Vec<(String, u32)> = Vec::new();
    visible_tabs.push(("All".to_string(), 0));
    if !hidden_tab_ids.contains("Unread") {
        let unread_n = folders
            .iter()
            .find(|(f, _)| f == "Unread")
            .map(|(_, c)| *c)
            .unwrap_or(total_unread_count);
        visible_tabs.push(("Unread".to_string(), unread_n));
    }
    for (folder, unread) in folders {
        if folder == "All" || folder == "Unread" {
            continue;
        }
        if !hidden_tab_ids.contains(folder) {
            visible_tabs.push((folder.clone(), *unread));
        }
    }

    let tabs: Vec<AnyElement> = visible_tabs
        .into_iter()
        .enumerate()
        .map(|(idx, (folder, unread))| {
            let is_active = folder == active_folder;
            let fname = folder.clone();
            let display_label = localize_folder_label(&folder, lang_id);
            let cb = on_select_folder.clone();
            let show_badge = folder != "All" && unread > 0;
            div()
                .id(("sidebar-folder-tab", idx as u64))
                .cursor_pointer()
                .relative()
                .h(px(OPENBOT_TABS_HEIGHT))
                .flex_shrink_0()
                .px(px(6.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if is_active { text_pri } else { text_muted })
                .hover(move |s| if is_active { s } else { s.text_color(text_pri) })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cb(this, fname.clone(), cx);
                    }),
                )
                .child(display_label)
                .when(show_badge, |el| {
                    el.child(
                        div()
                            .h(px(18.0))
                            .min_w(px(18.0))
                            .px(px(4.0))
                            .rounded_full()
                            .bg(if is_active { badge_active_bg } else { badge_idle_bg })
                            .text_color(if is_active { rgb(0xFFFFFF) } else { text_sec })
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if unread > 99 {
                                "99+".to_string()
                            } else {
                                unread.to_string()
                            }),
                    )
                })
                .when(is_active, |el| {
                    el.child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(2.0))
                            .rounded_full()
                            .bg(text_pri),
                    )
                })
                .into_any_element()
        })
        .collect();

    div()
        .id("openbot-sidebar-tabs")
        .mx(px(4.0))
        .mb(px(4.0))
        .pr(px(4.0))
        .h(px(OPENBOT_TABS_HEIGHT))
        .flex_shrink_0()
        .border_b_1()
        .border_color(hover_bg)
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_x_hidden()
                .flex()
                .items_center()
                .children(tabs),
        )
        .child(
            div()
                .id("openbot-sidebar-tabs-edit-btn")
                .cursor_pointer()
                .w(px(28.0))
                .h(px(28.0))
                .flex_shrink_0()
                .rounded_full()
                .when(is_edit_menu_open, |el| el.bg(badge_idle_bg))
                .hover(move |s| s.bg(hover_bg))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        on_toggle_edit_menu(this, cx);
                    }),
                )
                .child(openbot_vector_icon(
                    OpenBotChromeIcon::SlidersHorizontal,
                    15.0,
                    if is_edit_menu_open { text_pri } else { text_muted },
                )),
        )
        .into_any_element()
}

/// Renders the `SlidersHorizontal` tab-visibility checkbox popover (show/hide `未读` & folders).
#[allow(clippy::too_many_arguments)]
pub fn render_openbot_tabs_edit_popover<V: 'static>(
    available_folders: &[(String, u32)],
    hidden_tab_ids: &HashSet<String>,
    external_topbar_hosted: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_toggle_tab_visibility: impl Fn(&mut V, String, &mut Context<V>) + Clone + 'static,
    on_dismiss: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let surface = ob.bg_surface();
    let border = ob.border_strong_hairline();
    let sep = ob.border_hairline();
    let hover = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();
    let top_px = if external_topbar_hosted { 84.0 } else { 120.0 };

    let mut rows: Vec<AnyElement> = Vec::new();
    let unread_visible = !hidden_tab_ids.contains("Unread");
    let cb_unread = on_toggle_tab_visibility.clone();
    rows.push(
        div()
            .id("openbot-tab-vis-unread")
            .cursor_pointer()
            .h(px(30.0))
            .px(px(8.0))
            .rounded(px(7.0))
            .hover(move |s| s.bg(hover))
            .flex()
            .items_center()
            .gap(px(8.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb_unread(this, "Unread".to_string(), cx);
                }),
            )
            .child(div().w(px(14.0)).flex().items_center().justify_center().when(unread_visible, |el| {
                el.child(openbot_vector_icon(OpenBotChromeIcon::Check, 13.0, text_pri))
            }))
            .child(div().text_size(px(12.5)).text_color(text_pri).child(tr_str(lang_id, "未读", "Unread")))
            .into_any_element(),
    );

    let extra_folders: Vec<&(String, u32)> = available_folders
        .iter()
        .filter(|(f, _)| f != "All" && f != "Unread")
        .collect();
    if !extra_folders.is_empty() {
        rows.push(div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(sep).into_any_element());
        for (idx, (folder, count)) in extra_folders.into_iter().enumerate() {
            let visible = !hidden_tab_ids.contains(folder);
            let fname = folder.clone();
            let cb = on_toggle_tab_visibility.clone();
            let label = localize_folder_label(folder, lang_id);
            rows.push(
                div()
                    .id(("openbot-tab-vis-folder", idx as u64))
                    .cursor_pointer()
                    .h(px(30.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .hover(move |s| s.bg(hover))
                    .flex()
                    .items_center()
                    .justify_between()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            cb(this, fname.clone(), cx);
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().w(px(14.0)).flex().items_center().justify_center().when(visible, |el| {
                                el.child(openbot_vector_icon(OpenBotChromeIcon::Check, 13.0, text_pri))
                            }))
                            .child(div().text_size(px(12.5)).text_color(text_pri).child(label)),
                    )
                    .when(*count > 0, |el| {
                        el.child(div().text_size(px(11.0)).text_color(text_muted).child(count.to_string()))
                    })
                    .into_any_element(),
            );
        }
    }

    div()
        .id("openbot-tabs-edit-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                on_dismiss(this, cx);
            }),
        )
        .child(
            deferred(
                div()
                    .id("openbot-tabs-edit-popover")
                    .absolute()
                    .right(px(8.0))
                    .top(px(top_px))
                    .w(px(196.0))
                    .p(px(5.0))
                    .rounded(px(12.0))
                    .bg(surface)
                    .border_1()
                    .border_color(border)
                    .shadow(palette.popover_shadows())
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .children(rows),
            )
            .with_priority(38),
        )
        .into_any_element()
}

pub fn render_folder_tabs_strip<V: 'static>(
    folders: &[(String, u32)],
    active_folder: &str,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_select_folder: impl Fn(&mut V, String, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let icon_states = DialogsThreeStateIcon::new("folder", "folder_over", "folder_active");
    let is_apple = palette.is_apple_style();

    if is_apple {
        let ob = palette.openbot_palette();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();
        let hover_bg = ob.hover();
        let badge_idle_bg = ob.bg_control_active();
        let badge_active_bg = ob.accent();
        let tabs: Vec<AnyElement> = folders
            .iter()
            .enumerate()
            .map(|(idx, (folder, unread))| {
                let is_active = folder == active_folder;
                let _state_key = icon_states.select(is_active, false);
                let fname = folder.clone();
                let display_label = localize_folder_label(folder, lang_id);
                let cb = on_select_folder.clone();
                div()
                    .id(("sidebar-folder-tab", idx as u64))
                    .cursor_pointer()
                    .relative()
                    .h(px(OPENBOT_TABS_HEIGHT))
                    .flex_shrink_0()
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if is_active { text_pri } else { text_muted })
                    .hover(move |s| if is_active { s } else { s.text_color(text_pri) })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cb(this, fname.clone(), cx);
                        }),
                    )
                    .child(display_label)
                    .when(*unread > 0 && folder != "All", |el| {
                        el.child(
                            div()
                                .h(px(18.0))
                                .min_w(px(18.0))
                                .px(px(4.0))
                                .rounded_full()
                                .bg(if is_active { badge_active_bg } else { badge_idle_bg })
                                .text_color(if is_active { rgb(0xFFFFFF) } else { text_sec })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(format!("{}", unread)),
                        )
                    })
                    .when(is_active, |el| {
                        el.child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .h(px(2.0))
                                .rounded_full()
                                .bg(text_pri),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        return div()
            .mx(px(4.0))
            .mb(px(4.0))
            .h(px(OPENBOT_TABS_HEIGHT))
            .flex_shrink_0()
            .border_b_1()
            .border_color(hover_bg)
            .flex()
            .items_center()
            .overflow_x_hidden()
            .children(tabs)
            .into_any_element();
    }

    let tabs: Vec<AnyElement> = folders
        .iter()
        .enumerate()
        .map(|(idx, (folder, unread))| {
            let is_active = folder == active_folder;
            let _state_key = icon_states.select(is_active, false);
            let fname = folder.clone();
            let display_label = localize_folder_label(folder, lang_id);
            let cb = on_select_folder.clone();
            let text_hover = rgb(palette.text_primary);
            div()
                .id(("sidebar-folder-tab", idx as u64))
                .cursor_pointer()
                .relative()
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(palette.scaled_px(4.0)))
                .px(px(palette.scaled_px(6.0)))
                .text_size(px(palette.scaled_px(13.0)))
                .font_weight(if is_active { FontWeight::BOLD } else { FontWeight::NORMAL })
                .text_color(if is_active { rgb(palette.accent) } else { rgb(palette.text_secondary) })
                .hover(move |s| if is_active { s } else { s.text_color(text_hover) })
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    cb(this, fname.clone(), cx);
                }))
                .child(display_label)
                .when(*unread > 0, |el| {
                    el.child(
                        div()
                            .h(px(palette.scaled_px(16.0)))
                            .min_w(px(palette.scaled_px(16.0)))
                            .px(px(palette.scaled_px(4.0)))
                            .rounded_full()
                            .bg(if is_active { rgb(palette.accent) } else { rgb(palette.muted_badge_bg) })
                            .text_color(rgb(palette.unread_badge_fg))
                            .text_size(px(palette.scaled_px(10.0)))
                            .font_weight(FontWeight::BOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(format!("{}", unread)),
                    )
                })
                .when(is_active, |el| {
                    el.child(div().absolute().bottom_0().left_0().right_0().h(px(2.0)).bg(rgb(palette.accent)))
                })
                .into_any_element()
        })
        .collect();

    div()
        .h(px(palette.scaled_px(36.0)))
        .flex_shrink_0()
        .w_full()
        .px(px(palette.scaled_px(8.0)))
        .flex()
        .items_center()
        .gap(px(palette.scaled_px(10.0)))
        .border_b_1()
        .border_color(rgb(palette.border_fg))
        .overflow_x_hidden()
        .children(tabs)
        .into_any_element()
}

pub fn render_forum_topics_subtree<V: 'static>(
    topics: &DialogsTopicsViewUi,
    selected_topic: Option<(u64, u64)>,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_select_topic: impl Fn(&mut V, u64, u64, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let forum_id = topics.forum_id;
    let is_apple = palette.is_apple_style();
    let ob = palette.openbot_palette();
    let hover_bg = if is_apple {
        ob.hover()
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    let active_bg = if is_apple {
        if palette.mode.is_dark() {
            ob.bg_surface()
        } else {
            rgb(0xE8E8EB)
        }
    } else {
        rgb(palette.dialogs_active_bg)
    };
    let rows: Vec<AnyElement> = topics
        .topics
        .iter()
        .filter(|t| !t.is_hidden)
        .map(|topic| {
            let tid = topic.topic_id;
            let is_sel = selected_topic == Some((forum_id, tid));
            let cb = on_select_topic.clone();
            let title_fg = if is_apple {
                ob.text_primary()
            } else if is_sel {
                rgb(0xFFFFFF)
            } else {
                rgb(palette.text_primary)
            };
            let icon_col = if is_apple {
                if is_sel { ob.text_primary() } else { ob.text_muted() }
            } else if is_sel {
                rgb(0xFFFFFF)
            } else {
                rgb(palette.accent)
            };
            div()
                .id(("sidebar-topic-row", tid))
                .cursor_pointer()
                .h(px(34.0))
                .w_full()
                .pl(px(58.0))
                .pr(px(10.0))
                .when(is_apple, |el| el.rounded(px(8.0)))
                .when(is_sel, |el| el.bg(active_bg))
                .when(!is_sel && !is_apple, |el| el.bg(rgb(palette.dialogs_bg)))
                .hover(move |s| if is_sel { s } else { s.bg(hover_bg) })
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    cb(this, forum_id, tid, cx);
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .overflow_hidden()
                        .child(tg_icon_rgba(IconName::FolderChat, 12.0, icon_col))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.5))
                                .text_color(title_fg)
                                .child(topic.title.clone()),
                        )
                        .when(topic.is_closed, |el| {
                            el.child(tg_icon_rgba(IconName::Lock, 11.0, icon_col))
                        }),
                )
                .when(topic.unread_count > 0, |el| {
                    el.child(
                        div()
                            .h(px(16.0))
                            .min_w(px(16.0))
                            .px(px(4.0))
                            .rounded_full()
                            .bg(if is_apple {
                                ob.bg_control_active()
                            } else if is_sel {
                                TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.25)
                            } else {
                                rgb(palette.unread_badge_bg)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if is_apple {
                                ob.text_secondary()
                            } else {
                                rgb(palette.unread_badge_fg)
                            })
                            .child(format!("{}", topic.unread_count)),
                    )
                })
                .into_any_element()
        })
        .collect();
    div()
        .w_full()
        .when(!is_apple, |el| {
            el.border_b_1().border_color(rgb(palette.border_fg))
        })
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

pub fn render_dialog_context_menu<V: 'static>(
    dialog_id: u64,
    anchor: Point<Pixels>,
    is_pinned: bool,
    is_muted: bool,
    is_archived: bool,
    has_unread: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, SidebarContextAction, &mut Context<V>) + Clone + 'static,
    on_dismiss: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let mut qa_pin = DialogsQuickAction::new(dialog_id as i64, QuickDialogAction::Pin);
    qa_pin.update_state(false, is_muted, is_pinned, has_unread, is_archived, true, false);
    let _ctx_pin = DialogsQuickActionContextUi::new(dialog_id, QuickDialogActionType::Pin);

    let is_apple = palette.is_apple_style();
    let ob = palette.openbot_palette();
    let x = f32::from(anchor.x).clamp(8.0, 92.0);
    let y = f32::from(anchor.y).clamp(56.0, 420.0);
    let hover = if is_apple {
        ob.bg_control_hover()
    } else {
        rgb(palette.menu_hover_bg)
    };
    let menu_bg = if is_apple {
        ob.bg_surface()
    } else {
        rgb(palette.menu_bg)
    };
    let menu_border = if is_apple {
        ob.border_strong_hairline()
    } else {
        rgb(palette.border_fg)
    };

    let items: [(IconName, String, SidebarContextAction, bool); 6] = [
        (IconName::Pin, if is_pinned { tr_str(lang_id, "取消置顶", "Unpin").into() } else { tr_str(lang_id, "置顶对话", "Pin").into() }, SidebarContextAction::TogglePin(dialog_id), false),
        (IconName::Mute, if is_muted { tr_str(lang_id, "取消静音", "Unmute").into() } else { tr_str(lang_id, "静音通知", "Mute").into() }, SidebarContextAction::ToggleMute(dialog_id), false),
        (IconName::CheckDouble, if has_unread { tr_str(lang_id, "标记为已读", "Mark as Read").into() } else { tr_str(lang_id, "标记为未读", "Mark as Unread").into() }, SidebarContextAction::ToggleUnread(dialog_id), false),
        (IconName::ArchivedChats, if is_archived { tr_str(lang_id, "取消归档", "Unarchive").into() } else { tr_str(lang_id, "归档对话", "Archive").into() }, SidebarContextAction::ToggleArchive(dialog_id), false),
        (IconName::Close, tr_str(lang_id, "清空历史记录", "Clear History").into(), SidebarContextAction::ClearHistory(dialog_id), false),
        (IconName::DeleteTrash, tr_str(lang_id, "删除对话", "Delete Chat").into(), SidebarContextAction::DeleteChat(dialog_id), true),
    ];

    let mut rows: Vec<AnyElement> = Vec::with_capacity(items.len() + 1);
    for (idx, (icon, label, act, danger)) in items.into_iter().enumerate() {
        if idx == 4 {
            rows.push(
                div()
                    .h(px(1.0))
                    .mx(px(6.0))
                    .my(px(3.0))
                    .bg(if is_apple { ob.border_hairline() } else { menu_border })
                    .into_any_element(),
            );
        }
        let cb = on_action.clone();
        let fg = if danger {
            if is_apple { ob.danger() } else { rgb(palette.danger) }
        } else if is_apple {
            ob.text_primary()
        } else {
            rgb(palette.text_primary)
        };
        let ic = if danger {
            if is_apple { ob.danger() } else { rgb(palette.danger) }
        } else if is_apple {
            ob.text_muted()
        } else {
            rgb(palette.icon_fg)
        };
        rows.push(
            div()
                .id(("sidebar-ctx-item", idx as u64))
                .cursor_pointer()
                .h(px(30.0))
                .px(px(10.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(hover))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
                    cx.stop_propagation();
                    cb(this, act, cx);
                }))
                .child(tg_icon_rgba(icon, 14.0, ic))
                .child(div().text_size(px(12.5)).text_color(fg).child(label))
                .into_any_element(),
        );
    }

    div()
        .id("sidebar-context-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _w, cx| {
            on_dismiss(this, cx);
        }))
        .child(
            deferred(
                div()
                    .id("sidebar-context-menu")
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(184.0))
                    .p(px(5.0))
                    .rounded(px(12.0))
                    .bg(menu_bg)
                    .border_1()
                    .border_color(menu_border)
                    .shadow(palette.popover_shadows())
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .children(rows),
            )
            .with_priority(40),
        )
        .into_any_element()
}
