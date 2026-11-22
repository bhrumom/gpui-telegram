//! 1:1 Telegram Desktop dialog row and forum topics tree renderer.

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, GrokBotAvatarProps, GrokPersonaState,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::chat_sidebar::{BotGrokAvatarVisual, DialogDeliveryStatus, DialogItemModel};
use crate::views::sidebar::dialogs_entry::DialogsEntry;
use crate::views::sidebar::dialogs_row::DialogsRow;
use crate::views::sidebar::ui::dialogs_layout::DialogsLayoutUi;
use crate::views::sidebar::ui::dialogs_message_view::{
    DialogMessageMediaBadge, DialogsMessageViewUi,
};
use crate::views::sidebar::ui::dialogs_pill::DialogsPillUi;
use crate::views::sidebar::ui::dialogs_topics_view::DialogsTopicsViewUi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarContextAction {
    TogglePin(u64),
    ToggleMute(u64),
    ToggleArchive(u64),
    ToggleUnread(u64),
    ClearHistory(u64),
    DeleteChat(u64),
}

#[derive(Debug, Clone, Default)]
pub struct SidebarDialogMeta {
    pub is_verified: bool,
    pub is_premium: bool,
    pub is_bot: bool,
    pub fabushi_shape: Option<String>,
    pub fabushi_color: Option<u32>,
    pub grok_avatar: Option<BotGrokAvatarVisual>,
    pub is_saved_messages: bool,
    pub is_unread_marked: bool,
    pub is_archived: bool,
    pub draft_text: Option<String>,
    pub media_badge: DialogMessageMediaBadge,
    pub topics: Option<DialogsTopicsViewUi>,
    pub is_topics_expanded: bool,
    pub folder_tags: Vec<String>,
    pub miniapp_open_uri: Option<String>,
}

fn delivery_svg(
    status: DialogDeliveryStatus,
    palette: &TdesktopThemePalette,
    is_selected: bool,
) -> AnyElement {
    let icon = match status {
        DialogDeliveryStatus::Sending => IconName::ClockSending,
        DialogDeliveryStatus::Sent => IconName::CheckSingle,
        DialogDeliveryStatus::Read => IconName::CheckDouble,
    };
    let use_inverted_sel = is_selected && !palette.mode.is_fabushi_style();
    let color = if use_inverted_sel {
        palette.active_button_fg
    } else if status == DialogDeliveryStatus::Read {
        palette.check_read_fg
    } else {
        palette.check_sent_fg
    };
    tg_icon(icon, 13.0, color).into_any_element()
}

fn media_badge_svg_rgba(badge: DialogMessageMediaBadge, color: Rgba) -> Option<AnyElement> {
    let icon = match badge {
        DialogMessageMediaBadge::None => return None,
        DialogMessageMediaBadge::Photo => IconName::Camera,
        DialogMessageMediaBadge::Video => IconName::VideoRound,
        DialogMessageMediaBadge::Document => IconName::FileDoc,
        DialogMessageMediaBadge::Audio => IconName::Play,
        DialogMessageMediaBadge::Voice => IconName::Mic,
        DialogMessageMediaBadge::Location => IconName::LocationPin,
        DialogMessageMediaBadge::Poll => IconName::Poll,
        DialogMessageMediaBadge::Contact => IconName::ContactsUsers,
        DialogMessageMediaBadge::Game => IconName::BotRobot,
    };
    Some(tg_icon_rgba(icon, 12.0, color).into_any_element())
}

/// Shared circular / animated bot avatar renderer used by both dialog rows and the Apple Messages
/// 3-column Pinned Chats avatar grid.
pub(crate) fn render_dialog_avatar(
    item: &DialogItemModel,
    meta: &SidebarDialogMeta,
    palette: &TdesktopThemePalette,
    avatar_size_px: f32,
    surface_bg_hex: u32,
    online_dot_border: Rgba,
) -> Div {
    let avatar_bg = if meta.is_saved_messages {
        palette.accent
    } else {
        let base = meta.fabushi_color.unwrap_or(item.avatar_color);
        if base == 0 {
            palette.peer_avatar_gradient(item.id as i64).0
        } else {
            base
        }
    };
    let avatar_size = px(avatar_size_px);
    let initials_font_px = (avatar_size_px * 0.33).clamp(13.0, 19.0);
    let saved_icon_px = (avatar_size_px * 0.48).clamp(18.0, 26.0);
    let bot_icon_px = (avatar_size_px * 0.44).clamp(18.0, 24.0);

    let render_classic_circle = || -> Div {
        let mut el = div()
            .relative()
            .w(avatar_size)
            .h(avatar_size)
            .flex_shrink_0()
            .rounded_full()
            .bg(rgb(avatar_bg))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0xFFFFFF))
            .font_weight(FontWeight::BOLD)
            .text_size(px(initials_font_px));
        el = if meta.is_saved_messages {
            el.child(tg_icon(IconName::SavedMessages, saved_icon_px, 0xFFFFFF))
        } else if meta.is_bot
            && item
                .avatar_initials
                .chars()
                .all(|c| !c.is_ascii_alphanumeric())
        {
            el.child(tg_icon(IconName::BotRobot, bot_icon_px, 0xFFFFFF))
        } else {
            el.child(item.avatar_initials.clone())
        };
        el
    };

    let mut avatar = if !meta.is_saved_messages {
        if let Some(ref ga) = meta.grok_avatar {
            if !ga.shape.eq_ignore_ascii_case("classic") {
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(&ga.agent_id, &ga.shape, &ga.color, avatar_size_px)
                        .with_state(GrokPersonaState::from_activity_str(
                            &ga.execution_state,
                            item.typing_indicator.is_some(),
                        ))
                        .with_follow_pointer(true)
                        .with_paused(false)
                        .with_surface_bg(surface_bg_hex),
                )
            } else {
                render_classic_circle()
            }
        } else if let Some(ref shape) = meta.fabushi_shape {
            if !shape.eq_ignore_ascii_case("classic") {
                crate::components::avatar::render_fabushi_shape_avatar(
                    shape,
                    avatar_bg,
                    &item.peer_name,
                    avatar_size_px,
                )
            } else {
                render_classic_circle()
            }
        } else if meta.is_bot {
            const DEFAULT_BOT_SHAPES: [&str; 6] = [
                "circle", "squircle", "hexagon", "shield", "cloud", "diamond",
            ];
            const DEFAULT_BOT_COLORS: [&str; 6] = [
                "blue", "cyan", "purple", "emerald", "amber", "rose",
            ];
            let idx = (item.id as usize) % DEFAULT_BOT_SHAPES.len();
            render_grok_bot_avatar(
                GrokBotAvatarProps::new(
                    &format!("bot-{}", item.id),
                    DEFAULT_BOT_SHAPES[idx],
                    DEFAULT_BOT_COLORS[idx],
                    avatar_size_px,
                )
                .with_state(if item.typing_indicator.is_some() {
                    GrokPersonaState::Thinking
                } else {
                    GrokPersonaState::Idle
                })
                .with_follow_pointer(true)
                .with_paused(false)
                .with_surface_bg(surface_bg_hex),
            )
        } else {
            render_classic_circle()
        }
    } else {
        render_classic_circle()
    };

    if item.is_online && !meta.is_saved_messages {
        let dot_px = (avatar_size_px * 0.24).clamp(10.0, 13.0);
        avatar = avatar.relative().child(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .w(px(dot_px))
                .h(px(dot_px))
                .rounded_full()
                .bg(rgb(palette.online_dot_fg))
                .border_2()
                .border_color(online_dot_border),
        );
    }

    avatar
}

pub fn render_dialog_row_item<V: 'static>(
    item: &DialogItemModel,
    index: usize,
    is_selected: bool,
    is_narrow: bool,
    meta: &SidebarDialogMeta,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_select: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_context: impl Fn(&mut V, Point<Pixels>, &mut Context<V>) + 'static,
    on_toggle_topics: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_open_miniapp: impl Fn(&mut V, String, &mut Context<V>) + 'static,
) -> AnyElement {
    let mut entry = DialogsEntry::new(item.id as i64, item.peer_name.clone());
    entry.data.is_forum = meta.topics.as_ref().is_some_and(|t| !t.topics.is_empty());
    entry.set_active(is_selected);
    let mut row_model = DialogsRow::new(entry, index, 0);
    row_model.set_selected(is_selected);
    row_model.recount_height(if is_narrow { 1.0 } else { 0.0 });
    let _ = (DialogsLayoutUi::AVATAR_SIZE, DialogsPillUi::HEIGHT, DialogsPillUi::PADDING);

    let mut preview_ui = if let Some(ref draft) = meta.draft_text {
        DialogsMessageViewUi::as_draft(draft.clone())
    } else {
        DialogsMessageViewUi::new(item.last_message.clone())
    };
    preview_ui.media_badge = meta.media_badge;
    let mut pill = DialogsPillUi::new(String::new(), item.unread_count);
    pill.is_active = is_selected;
    pill.is_muted = item.is_muted;

    if palette.is_apple_style() {
        let row_height = palette.scaled_px(68.0);
        let row_bg_hex = if is_selected {
            palette.dialogs_active_bg
        } else {
            palette.dialogs_bg
        };
        let apple_hover_bg = if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.06)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.045)
        };
        let selected_sub_rgba = TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.85);
        let name_fg: Rgba = if is_selected {
            rgb(0xFFFFFF)
        } else {
            rgb(palette.text_primary)
        };
        let sub_fg: Rgba = if is_selected {
            selected_sub_rgba
        } else {
            rgb(palette.text_secondary)
        };
        let time_fg: Rgba = if is_selected {
            selected_sub_rgba
        } else {
            rgb(palette.msg_in_date_fg)
        };
        let chevron_rgba: Rgba = if is_selected {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.80)
        } else {
            TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, 0.55)
        };
        let avatar_size_px = palette.scaled_px(44.0);
        let online_border = if is_selected {
            rgb(palette.dialogs_active_bg)
        } else {
            palette.sidebar_bg_rgba()
        };
        let avatar = render_dialog_avatar(
            item,
            meta,
            palette,
            avatar_size_px,
            row_bg_hex,
            online_border,
        );

        let has_unread = item.unread_count > 0 || meta.is_unread_marked;
        let unread_dot_bg = if is_selected {
            rgb(0xFFFFFF)
        } else if item.is_muted {
            rgb(palette.muted_badge_bg)
        } else {
            rgb(palette.accent)
        };
        let unread_gutter = div()
            .w(px(palette.scaled_px(14.0)))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when(has_unread, |el| {
                el.child(
                    div()
                        .w(px(palette.scaled_px(8.0)))
                        .h(px(palette.scaled_px(8.0)))
                        .rounded_full()
                        .bg(unread_dot_bg),
                )
            });

        let mut title_row = div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(palette.scaled_px(4.0)))
            .overflow_hidden()
            .child(
                div()
                    .truncate()
                    .text_size(px(palette.scaled_px(13.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(name_fg)
                    .child(item.peer_name.clone()),
            );
        if meta.is_verified {
            title_row = title_row.child(tg_icon(
                IconName::Verified,
                palette.scaled_px(13.0),
                if is_selected { 0xFFFFFF } else { palette.accent },
            ));
        }
        if meta.is_premium {
            title_row = title_row.child(tg_icon(
                IconName::PremiumStar,
                palette.scaled_px(12.0),
                if is_selected { 0xFFFFFF } else { palette.accent },
            ));
        }
        if item.is_muted {
            title_row = title_row.child(tg_icon_rgba(
                IconName::Mute,
                palette.scaled_px(11.5),
                if is_selected {
                    TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.78)
                } else {
                    TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, 0.72)
                },
            ));
        }

        let top_row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(title_row)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .flex_shrink_0()
                    .child(
                        div()
                            .text_size(px(palette.scaled_px(11.5)))
                            .text_color(time_fg)
                            .child(item.timestamp.clone()),
                    )
                    .child(tg_icon_rgba(
                        IconName::ChevronRight,
                        palette.scaled_px(11.0),
                        chevron_rgba,
                    )),
            );

        let preview_font = px(palette.scaled_px(12.5));
        let preview_max_h = px(palette.scaled_px(34.0));
        let preview_line: AnyElement = if preview_ui.is_draft {
            let draft_prefix_color = if is_selected {
                rgb(0xFFFFFF)
            } else {
                rgb(palette.danger)
            };
            div()
                .w_full()
                .max_h(preview_max_h)
                .overflow_hidden()
                .flex()
                .items_start()
                .gap(px(3.0))
                .text_size(preview_font)
                .line_height(relative(1.32))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(draft_prefix_color)
                        .child(tr_str(lang_id, "草稿:", "Draft:")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_color(sub_fg)
                        .child(preview_ui.text),
                )
                .into_any_element()
        } else if let Some(ref typing) = item.typing_indicator {
            div()
                .w_full()
                .max_h(preview_max_h)
                .overflow_hidden()
                .text_size(preview_font)
                .line_height(relative(1.32))
                .text_color(if is_selected {
                    TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.92)
                } else {
                    rgb(palette.accent)
                })
                .child(typing.clone())
                .into_any_element()
        } else if let Some(icon) = media_badge_svg_rgba(preview_ui.media_badge, sub_fg) {
            div()
                .w_full()
                .max_h(preview_max_h)
                .overflow_hidden()
                .flex()
                .items_start()
                .gap(px(4.0))
                .child(div().pt(px(2.0)).flex_shrink_0().child(icon))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(preview_font)
                        .line_height(relative(1.32))
                        .text_color(sub_fg)
                        .child(preview_ui.text),
                )
                .into_any_element()
        } else {
            div()
                .w_full()
                .max_h(preview_max_h)
                .overflow_hidden()
                .text_size(preview_font)
                .line_height(relative(1.32))
                .text_color(sub_fg)
                .child(preview_ui.text)
                .into_any_element()
        };

        let mut right_aux = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .flex_shrink_0();
        let mut has_aux = false;

        if let Some(ref open_uri) = meta.miniapp_open_uri {
            has_aux = true;
            let uri_owned = open_uri.clone();
            let pill_bg = if is_selected {
                rgb(0xFFFFFF)
            } else {
                rgb(palette.accent)
            };
            let pill_fg = if is_selected {
                rgb(palette.accent)
            } else {
                rgb(0xFFFFFF)
            };
            right_aux = right_aux.child(
                div()
                    .id(("dialog-row-open-miniapp", item.id))
                    .cursor_pointer()
                    .px(px(palette.scaled_px(8.0)))
                    .py(px(palette.scaled_px(2.0)))
                    .rounded_full()
                    .bg(pill_bg)
                    .text_size(px(palette.scaled_px(11.0)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pill_fg)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            on_open_miniapp(this, uri_owned.clone(), cx);
                        }),
                    )
                    .child(tr_str(lang_id, "打开", "Open")),
            );
        }

        // Keep unread state crisp: left blue dot is primary; for multi-message unread counts,
        // show a subtle, lightweight count pill without heavy badge clutter.
        if item.unread_count > 1 {
            has_aux = true;
            let (count_bg, count_fg) = if is_selected {
                (
                    TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.24),
                    rgb(0xFFFFFF),
                )
            } else if item.is_muted {
                (
                    TdesktopThemePalette::rgba_with_alpha(palette.muted_badge_bg, 0.22),
                    rgb(palette.text_secondary),
                )
            } else {
                (
                    TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.14),
                    rgb(palette.accent),
                )
            };
            right_aux = right_aux.child(
                div()
                    .h(px(palette.scaled_px(16.0)))
                    .min_w(px(palette.scaled_px(16.0)))
                    .px(px(palette.scaled_px(5.0)))
                    .rounded_full()
                    .bg(count_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(palette.scaled_px(10.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(count_fg)
                    .child(format!("{}", pill.count)),
            );
        }

        if meta.topics.as_ref().is_some_and(|t| !t.topics.is_empty()) {
            has_aux = true;
            let chev = if meta.is_topics_expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            right_aux = right_aux.child(
                div()
                    .id(("forum-topics-toggle", item.id))
                    .cursor_pointer()
                    .px(px(4.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .bg(if is_selected {
                        TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.20)
                    } else {
                        TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.12)
                    })
                    .flex()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            on_toggle_topics(this, cx);
                        }),
                    )
                    .child(tg_icon(
                        chev,
                        10.0,
                        if is_selected { 0xFFFFFF } else { palette.accent },
                    )),
            );
        }

        let bottom_row = div()
            .w_full()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(6.0))
            .child(div().flex_1().min_w(px(0.0)).child(preview_line))
            .when(has_aux, |el| el.child(div().pt(px(1.0)).child(right_aux)));

        return div()
            .id(("sidebar-dialog-row", item.id))
            .cursor_pointer()
            .h(px(row_height))
            .flex_shrink_0()
            .w_full()
            .pl(px(palette.scaled_px(4.0)))
            .pr(px(palette.scaled_px(10.0)))
            .py(px(palette.scaled_px(8.0)))
            .rounded(px(palette.scaled_px(10.0)))
            .when(is_selected, |el| el.bg(rgb(palette.dialogs_active_bg)))
            .hover(move |s| if is_selected { s } else { s.bg(apple_hover_bg) })
            .flex()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, _w, cx| {
                    on_select(this, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                    on_context(this, ev.position, cx);
                }),
            )
            .child(unread_gutter)
            .child(div().ml(px(palette.scaled_px(2.0))).flex_shrink_0().child(avatar))
            .child(
                div()
                    .ml(px(palette.scaled_px(8.0)))
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(palette.scaled_px(2.0)))
                    .child(top_row)
                    .child(bottom_row),
            )
            .into_any_element();
    }

    // Legacy Telegram Desktop Classic style path
    let row_height = palette.scaled_px(row_model.height.max(62) as f32);
    let use_inverted_sel = is_selected;
    let row_bg_hex = if is_selected {
        palette.dialogs_active_bg
    } else {
        palette.dialogs_bg
    };
    let row_bg = rgb(row_bg_hex);
    let hover_bg = rgb(palette.dialogs_hover_bg);
    let name_fg = if use_inverted_sel {
        rgb(palette.active_button_fg)
    } else {
        rgb(palette.text_primary)
    };
    let sub_fg_hex = if use_inverted_sel {
        palette.active_button_fg
    } else {
        palette.text_secondary
    };
    let time_fg = if use_inverted_sel {
        rgb(palette.active_button_fg)
    } else {
        rgb(palette.msg_in_date_fg)
    };
    let icon_hex = if use_inverted_sel {
        palette.active_button_fg
    } else {
        palette.icon_fg
    };
    let badge_bg = if use_inverted_sel {
        rgb(palette.active_button_fg)
    } else if item.is_muted {
        rgb(palette.muted_badge_bg)
    } else {
        rgb(palette.unread_badge_bg)
    };
    let badge_fg = if use_inverted_sel {
        rgb(palette.accent)
    } else {
        rgb(palette.unread_badge_fg)
    };
    let avatar_size_px = palette.scaled_px(46.0);
    let avatar = render_dialog_avatar(item, meta, palette, avatar_size_px, row_bg_hex, row_bg);

    let root = div()
        .id(("sidebar-dialog-row", item.id))
        .cursor_pointer()
        .h(px(row_height))
        .flex_shrink_0()
        .w_full()
        .px(px(palette.scaled_px(10.0)))
        .py(px(palette.scaled_px(8.0)))
        .bg(row_bg)
        .hover(move |s| if is_selected { s } else { s.bg(hover_bg) })
        .flex()
        .items_center()
        .gap(px(palette.scaled_px(12.0)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev, _w, cx| {
                on_select(this, cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                on_context(this, ev.position, cx);
            }),
        );

    let mut title_row = div()
        .flex()
        .items_center()
        .gap(px(palette.scaled_px(4.0)))
        .overflow_hidden();
    title_row = title_row.child(
        div()
            .text_size(px(palette.scaled_px(14.0)))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(name_fg)
            .child(item.peer_name.clone()),
    );
    if meta.is_verified {
        title_row = title_row.child(tg_icon(
            IconName::Verified,
            palette.scaled_px(14.0),
            if use_inverted_sel {
                palette.active_button_fg
            } else {
                palette.accent
            },
        ));
    }
    if meta.is_premium {
        title_row = title_row.child(tg_icon(
            IconName::PremiumStar,
            palette.scaled_px(13.0),
            if use_inverted_sel {
                palette.active_button_fg
            } else {
                palette.accent
            },
        ));
    }
    if item.is_muted {
        title_row = title_row.child(tg_icon(IconName::Mute, palette.scaled_px(12.0), icon_hex));
    }

    let preview_font = px(palette.scaled_px(13.0));
    let preview_line: AnyElement = if preview_ui.is_draft {
        div()
            .flex()
            .items_center()
            .gap(px(3.0))
            .overflow_hidden()
            .child(
                div()
                    .text_size(preview_font)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.danger))
                    .child(tr_str(lang_id, "草稿:", "Draft:")),
            )
            .child(
                div()
                    .text_size(preview_font)
                    .text_color(rgb(sub_fg_hex))
                    .child(preview_ui.text),
            )
            .into_any_element()
    } else if let Some(ref typing) = item.typing_indicator {
        div()
            .text_size(preview_font)
            .text_color(if use_inverted_sel {
                rgb(palette.active_button_fg)
            } else {
                rgb(palette.accent)
            })
            .child(typing.clone())
            .into_any_element()
    } else {
        let mut line = div().flex().items_center().gap(px(4.0)).overflow_hidden();
        if let Some(icon) = media_badge_svg_rgba(preview_ui.media_badge, rgb(sub_fg_hex)) {
            line = line.child(icon);
        }
        line.child(
            div()
                .text_size(preview_font)
                .text_color(rgb(sub_fg_hex))
                .child(preview_ui.text),
        )
        .into_any_element()
    };

    let mut right_badges = div()
        .flex()
        .items_center()
        .gap(px(palette.scaled_px(5.0)))
        .flex_shrink_0();
    if let Some(ref open_uri) = meta.miniapp_open_uri {
        let uri_owned = open_uri.clone();
        let pill_bg = if use_inverted_sel {
            rgb(palette.active_button_fg)
        } else {
            rgb(palette.accent)
        };
        let pill_fg = if use_inverted_sel {
            rgb(palette.accent)
        } else {
            rgb(0xFFFFFF)
        };
        right_badges = right_badges.child(
            div()
                .id(("dialog-row-open-miniapp", item.id))
                .cursor_pointer()
                .px(px(palette.scaled_px(8.0)))
                .py(px(palette.scaled_px(2.0)))
                .rounded_full()
                .bg(pill_bg)
                .text_size(px(palette.scaled_px(11.0)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pill_fg)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        on_open_miniapp(this, uri_owned.clone(), cx);
                    }),
                )
                .child(tr_str(lang_id, "打开", "Open")),
        );
    }
    for (tag_idx, tag_label) in meta.folder_tags.iter().enumerate() {
        let display_tag =
            crate::views::sidebar::dialogs_overlays::localize_folder_label(tag_label, lang_id);
        let tag_bg = if use_inverted_sel {
            TdesktopThemePalette::rgba_with_alpha(palette.active_button_fg, 0.22)
        } else {
            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16)
        };
        let tag_fg = if use_inverted_sel {
            rgb(palette.active_button_fg)
        } else {
            rgb(palette.accent)
        };
        right_badges = right_badges.child(
            div()
                .id((
                    "dialog-folder-tag",
                    item.id.saturating_mul(10) + tag_idx as u64,
                ))
                .px(px(palette.scaled_px(5.0)))
                .py(px(palette.scaled_px(1.5)))
                .rounded(px(palette.scaled_px(4.0)))
                .bg(tag_bg)
                .text_size(px(palette.scaled_px(10.0)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tag_fg)
                .child(display_tag),
        );
    }
    if item.is_pinned && item.unread_count == 0 && !meta.is_unread_marked {
        right_badges =
            right_badges.child(tg_icon(IconName::Pin, palette.scaled_px(14.0), icon_hex));
    }
    if item.unread_count > 0 {
        let pill_h = px(palette.scaled_px(19.0));
        let pill_pad = px(palette.scaled_px(5.0));
        right_badges = right_badges.child(
            div()
                .h(pill_h)
                .min_w(pill_h)
                .px(pill_pad)
                .rounded_full()
                .bg(badge_bg)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(palette.scaled_px(11.5)))
                .font_weight(FontWeight::BOLD)
                .text_color(badge_fg)
                .child(format!("{}", pill.count)),
        );
    } else if meta.is_unread_marked {
        right_badges = right_badges.child(
            div()
                .w(px(palette.scaled_px(10.0)))
                .h(px(palette.scaled_px(10.0)))
                .rounded_full()
                .bg(badge_bg),
        );
    }

    let top_row = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(title_row)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(3.0))
                .flex_shrink_0()
                .when(item.unread_count == 0 && meta.draft_text.is_none(), |el| {
                    el.child(delivery_svg(item.delivery_status, palette, is_selected))
                })
                .child(
                    div()
                        .text_size(px(palette.scaled_px(12.0)))
                        .text_color(time_fg)
                        .child(item.timestamp.clone()),
                ),
        );

    let mut bottom_row = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(div().flex_1().overflow_hidden().child(preview_line))
        .child(right_badges);
    if meta.topics.as_ref().is_some_and(|t| !t.topics.is_empty()) {
        let chev = if meta.is_topics_expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        bottom_row = bottom_row.child(
            div()
                .id(("forum-topics-toggle", item.id))
                .cursor_pointer()
                .px(px(4.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .bg(rgb(palette.composer_bg))
                .flex()
                .items_center()
                .gap(px(2.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        on_toggle_topics(this, cx);
                    }),
                )
                .child(tg_icon(chev, 10.0, palette.accent)),
        );
    }

    root.child(avatar)
        .child(
            div()
                .flex_1()
                .h_full()
                .overflow_hidden()
                .flex()
                .flex_col()
                .justify_between()
                .child(top_row)
                .child(bottom_row),
        )
        .into_any_element()
}
