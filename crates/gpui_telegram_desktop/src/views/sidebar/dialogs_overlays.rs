//! 1:1 Telegram Desktop folder strip, forum topics tree, and context menu overlays.

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::sidebar::dialogs_quick_action::{DialogsQuickAction, QuickDialogAction};
use crate::views::sidebar::dialogs_row_view::SidebarContextAction;
use crate::views::sidebar::dialogs_three_state_icon::DialogsThreeStateIcon;
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
        let idle_bg = if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.055)
        };
        let idle_hover_bg = if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.13)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.095)
        };
        let text_hover = rgb(palette.text_primary);
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
                    .h(px(palette.scaled_px(24.0)))
                    .flex_shrink_0()
                    .px(px(palette.scaled_px(9.0)))
                    .rounded_full()
                    .bg(if is_active {
                        rgb(palette.accent)
                    } else {
                        idle_bg
                    })
                    .hover(move |s| {
                        if is_active {
                            s
                        } else {
                            s.bg(idle_hover_bg).text_color(text_hover)
                        }
                    })
                    .flex()
                    .items_center()
                    .gap(px(palette.scaled_px(4.0)))
                    .text_size(px(palette.scaled_px(11.5)))
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_active {
                        rgb(0xFFFFFF)
                    } else {
                        rgb(palette.text_secondary)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cb(this, fname.clone(), cx);
                        }),
                    )
                    .child(display_label)
                    .when(*unread > 0, |el| {
                        el.child(
                            div()
                                .h(px(palette.scaled_px(15.0)))
                                .min_w(px(palette.scaled_px(15.0)))
                                .px(px(palette.scaled_px(4.0)))
                                .rounded_full()
                                .bg(if is_active {
                                    TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.25)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, 0.18)
                                })
                                .text_color(if is_active {
                                    rgb(0xFFFFFF)
                                } else {
                                    rgb(palette.text_secondary)
                                })
                                .text_size(px(palette.scaled_px(9.5)))
                                .font_weight(FontWeight::BOLD)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(format!("{}", unread)),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        return div()
            .h(px(palette.scaled_px(34.0)))
            .flex_shrink_0()
            .w_full()
            .px(px(palette.scaled_px(12.0)))
            .pb(px(palette.scaled_px(6.0)))
            .flex()
            .items_center()
            .gap(px(palette.scaled_px(5.0)))
            .border_b_1()
            .border_color(palette.separator_rgba())
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
    let hover_bg = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.06)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.045)
        }
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    let rows: Vec<AnyElement> = topics
        .topics
        .iter()
        .filter(|t| !t.is_hidden)
        .map(|topic| {
            let tid = topic.topic_id;
            let is_sel = selected_topic == Some((forum_id, tid));
            let cb = on_select_topic.clone();
            div()
                .id(("sidebar-topic-row", tid))
                .cursor_pointer()
                .h(px(34.0))
                .w_full()
                .pl(px(58.0))
                .pr(px(10.0))
                .when(is_apple, |el| el.rounded(px(8.0)))
                .when(is_sel, |el| el.bg(rgb(palette.dialogs_active_bg)))
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
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .overflow_hidden()
                        .child(tg_icon(
                            IconName::FolderChat,
                            12.0,
                            if is_sel { 0xFFFFFF } else { palette.accent },
                        ))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(if is_sel {
                                    rgb(0xFFFFFF)
                                } else {
                                    rgb(palette.text_primary)
                                })
                                .child(topic.title.clone()),
                        )
                        .when(topic.is_closed, |el| {
                            el.child(tg_icon(
                                IconName::Lock,
                                11.0,
                                if is_sel { 0xFFFFFF } else { palette.icon_fg },
                            ))
                        }),
                )
                .when(topic.unread_count > 0, |el| {
                    el.child(
                        div()
                            .h(px(16.0))
                            .min_w(px(16.0))
                            .px(px(4.0))
                            .rounded_full()
                            .bg(if is_sel {
                                TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.25)
                            } else {
                                rgb(palette.unread_badge_bg)
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(palette.unread_badge_fg))
                            .child(format!("{}", topic.unread_count)),
                    )
                })
                .into_any_element()
        })
        .collect();
    div()
        .w_full()
        .border_b_1()
        .border_color(if is_apple {
            palette.separator_rgba()
        } else {
            rgb(palette.border_fg)
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
    let x = f32::from(anchor.x).clamp(8.0, 92.0);
    let y = f32::from(anchor.y).clamp(56.0, 420.0);
    let hover = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.05)
        }
    } else {
        rgb(palette.menu_hover_bg)
    };
    let menu_bg = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            palette.menu_bg,
            if palette.mode.is_dark() { 0.94 } else { 0.96 },
        )
    } else {
        rgb(palette.menu_bg)
    };
    let menu_border = if is_apple {
        palette.separator_rgba()
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
                    .bg(menu_border)
                    .into_any_element(),
            );
        }
        let cb = on_action.clone();
        let fg = if danger { palette.danger } else { palette.text_primary };
        let ic = if danger { palette.danger } else { palette.icon_fg };
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
                .child(tg_icon(icon, 14.0, ic))
                .child(div().text_size(px(12.5)).text_color(rgb(fg)).child(label))
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
