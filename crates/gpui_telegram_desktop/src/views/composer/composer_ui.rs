//! Main Input Strip, Reply/Edit/Attachment Banners & Voice Record Bar (`composer_ui`).

use std::path::PathBuf;
use gpui::*;
use gpui::prelude::FluentBuilder;

use crate::i18n::{tr_key_or, tr_str};
use crate::theme::{tg_icon, tg_icon_rgba, IconName, TdesktopThemePalette};
use super::editor_state::clamp_char_boundary;
use super::voice_gesture::VoiceGestureMachine;

#[derive(Debug, Clone)]
pub enum ComposerBarAction {
    CancelContext,
    ClearAttachments,
    ToggleAttachments,
    ToggleFormatting,
    ToggleEmojiPicker,
    ToggleBotKeyboard,
    OpenBotMenu,
    FocusInput,
    SendOrSave,
    OpenSendOptions,
    StartVoice(Point<Pixels>),
    MoveVoice(Point<Pixels>),
    EndVoice,
    LockVoice,
    CancelVoice,
    CycleSendAs,
}

pub fn render_composer_context_banner<V: 'static>(
    replying_to: Option<&(u64, String, String)>,
    editing_message: Option<&(u64, String)>,
    attachments: &[PathBuf],
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerBarAction, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    let mut col = div().flex().flex_col().w_full();
    let mut has_any = false;

    if let Some((_id, sender, snippet)) = replying_to {
        has_any = true;
        let a = on_action.clone();
        col = col.child(banner_row(
            IconName::Reply,
            &format!("{} {}", tr_key_or(lang_id, "lng_reply_to", "Reply to"), sender),
            snippet,
            palette,
            cx,
            move |this, cx| a(this, ComposerBarAction::CancelContext, cx),
        ));
    } else if let Some((_id, orig)) = editing_message {
        has_any = true;
        let a = on_action.clone();
        col = col.child(banner_row(
            IconName::Edit,
            tr_key_or(lang_id, "lng_edit_message_title", "Edit Message"),
            orig,
            palette,
            cx,
            move |this, cx| a(this, ComposerBarAction::CancelContext, cx),
        ));
    }

    if !attachments.is_empty() {
        has_any = true;
        let a = on_action;
        let names = attachments
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        col = col.child(banner_row(
            IconName::Paperclip,
            &format!("{} ({})", tr_key_or(lang_id, "lng_attach_files_title", "Attachments"), attachments.len()),
            &names,
            palette,
            cx,
            move |this, cx| a(this, ComposerBarAction::ClearAttachments, cx),
        ));
    }

    if has_any { Some(col.into_any_element()) } else { None }
}

fn banner_row<V: 'static>(
    icon: IconName, title: &str, snippet: &str, palette: &TdesktopThemePalette,
    cx: &mut Context<V>, on_close: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let row = div()
        .h(px(if is_apple { 38.0 } else { 42.0 }))
        .w_full()
        .px(px(if is_apple { 12.0 } else { 14.0 }))
        .when(is_apple, |el| {
            el.rounded(px(12.0))
                .bg(palette.card_surface_rgba())
                .border_1()
                .border_color(palette.separator_rgba())
        })
        .when(!is_apple, |el| {
            el.bg(rgb(palette.compose_bg))
                .border_b_1()
                .border_color(rgb(palette.border_fg))
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_1()
                .overflow_hidden()
                .when(!is_apple, |el| el.child(tg_icon(icon, 16.0, palette.accent)))
                .child(div().w(px(2.5)).h(px(22.0)).rounded_full().bg(rgb(palette.accent)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(palette.accent))
                                .truncate()
                                .child(title.to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(palette.text_secondary))
                                .truncate()
                                .child(snippet.to_string()),
                        ),
                ),
        )
        .child(
            div()
                .id("composer-banner-close")
                .w(px(24.0))
                .h(px(24.0))
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(move |this, _, _, cx| on_close(this, cx)))
                .child(tg_icon(IconName::Close, 13.0, palette.icon_fg)),
        );

    if is_apple {
        div()
            .w_full()
            .px(px(20.0))
            .pt(px(4.0))
            .bg(rgb(palette.composer_bg))
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(740.0)).child(row))
            .into_any_element()
    } else {
        row.into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_text_input_strip<V: 'static>(
    text: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
    preedit: Option<&str>,
    is_focused: bool,
    is_editing: bool,
    has_attachments: bool,
    is_formatting_open: bool,
    is_emoji_open: bool,
    content_height: Pixels,
    overflow_scroll: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerBarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_text_input_strip_with_bot_menu(
        text,
        cursor,
        selection,
        preedit,
        is_focused,
        is_editing,
        has_attachments,
        is_formatting_open,
        is_emoji_open,
        content_height,
        overflow_scroll,
        None,
        palette,
        lang_id,
        cx,
        on_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_text_input_strip_with_bot_menu<V: 'static>(
    text: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
    preedit: Option<&str>,
    is_focused: bool,
    is_editing: bool,
    has_attachments: bool,
    is_formatting_open: bool,
    is_emoji_open: bool,
    content_height: Pixels,
    overflow_scroll: bool,
    bot_menu_view: Option<AnyElement>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerBarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_text_input_strip_with_options(
        text,
        cursor,
        selection,
        preedit,
        is_focused,
        is_editing,
        has_attachments,
        is_formatting_open,
        is_emoji_open,
        content_height,
        overflow_scroll,
        bot_menu_view,
        None,
        false,
        palette,
        lang_id,
        cx,
        on_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_text_input_strip_with_options<V: 'static>(
    text: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
    preedit: Option<&str>,
    is_focused: bool,
    is_editing: bool,
    has_attachments: bool,
    is_formatting_open: bool,
    is_emoji_open: bool,
    content_height: Pixels,
    overflow_scroll: bool,
    bot_menu_view: Option<AnyElement>,
    placeholder_override: Option<&str>,
    send_disabled: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerBarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_text_input_strip_with_full_options(
        text,
        cursor,
        selection,
        preedit,
        is_focused,
        is_editing,
        has_attachments,
        is_formatting_open,
        is_emoji_open,
        false,
        false,
        content_height,
        overflow_scroll,
        bot_menu_view,
        placeholder_override,
        send_disabled,
        palette,
        lang_id,
        cx,
        on_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_text_input_strip_with_full_options<V: 'static>(
    text: &str,
    cursor: usize,
    selection: Option<(usize, usize)>,
    preedit: Option<&str>,
    is_focused: bool,
    is_editing: bool,
    has_attachments: bool,
    is_formatting_open: bool,
    is_emoji_open: bool,
    has_bot_keyboard: bool,
    is_bot_keyboard_open: bool,
    content_height: Pixels,
    overflow_scroll: bool,
    bot_menu_view: Option<AnyElement>,
    placeholder_override: Option<&str>,
    send_disabled: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerBarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let is_empty = text.is_empty() && preedit.map_or(true, |p| p.is_empty());
    let show_send = (!is_empty || is_editing || has_attachments) && !send_disabled;
    let a_att = on_action.clone();
    let a_fmt = on_action.clone();
    let a_emo = on_action.clone();
    let a_kb = on_action.clone();
    let a_foc = on_action.clone();
    let a_snd = on_action.clone();
    let a_sopt = on_action.clone();
    let a_mic = on_action;

    let cur = clamp_char_boundary(text, cursor);
    let before = &text[..cur];
    let after = &text[cur..];
    let is_apple = palette.is_apple_style();
    let placeholder_text = placeholder_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if is_apple {
                tr_str(lang_id, "消息", "Message").to_string()
            } else {
                tr_key_or(lang_id, "lng_message_ph", "Write a message...").to_string()
            }
        });

    let composer_font = px(palette.scaled_px(palette.font_size_px as f32));
    let text_el = if is_empty {
        div()
            .w_full()
            .flex()
            .items_center()
            .text_size(composer_font)
            .text_color(rgb(palette.text_secondary))
            .when(is_focused, |el| {
                el.child(
                    div()
                        .w(px(1.5))
                        .h(px(palette.scaled_px(17.0)))
                        .mr(px(2.0))
                        .bg(rgb(palette.accent)),
                )
            })
            .child(placeholder_text)
            .into_any_element()
    } else if let Some((s, e)) = selection {
        let s = clamp_char_boundary(text, s);
        let e = clamp_char_boundary(text, e);
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .text_size(composer_font)
            .text_color(rgb(palette.text_primary))
            .child(text[..s].to_string())
            .child(div().bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.35)).child(text[s..e].to_string()))
            .child(text[e..].to_string())
            .into_any_element()
    } else {
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .text_size(composer_font)
            .text_color(rgb(palette.text_primary))
            .when(!before.is_empty(), |el| el.child(before.to_string()))
            .when_some(preedit.filter(|p| !p.is_empty()), |el, p| {
                el.child(div().underline().text_color(rgb(palette.accent)).child(p.to_string()))
            })
            .when(is_focused, |el| el.child(div().w(px(1.5)).h(px(palette.scaled_px(17.0))).bg(rgb(palette.accent))))
            .when(!after.is_empty(), |el| el.child(after.to_string()))
            .into_any_element()
    };

    let raw_content_h = f32::from(content_height);
    if is_apple {
        let apple_capsule_h = if raw_content_h <= 46.5 {
            46.0
        } else {
            raw_content_h.max(46.0)
        };
        let scaled_capsule_h = px(palette.scaled_px(apple_capsule_h));
        let is_dark = palette.mode.is_dark();
        let pill_bg = if is_dark {
            palette.card_bg
        } else {
            0xFFFFFF
        };
        let pill_border: Rgba = if is_focused {
            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.72)
        } else {
            palette.hairline_border_rgba()
        };
        let pill_shadow = if is_dark {
            Vec::new()
        } else {
            vec![
                BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.07),
                    offset: point(px(0.0), px(3.0)),
                    blur_radius: px(16.0),
                    spread_radius: px(0.0),
                },
                BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.05),
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(3.0),
                    spread_radius: px(0.0),
                },
            ]
        };
        let attach_hover = TdesktopThemePalette::rgba_with_alpha(
            palette.text_primary,
            if is_dark { 0.10 } else { 0.06 },
        );
        let accent = palette.accent;
        let accent_hover = palette.accent_hover;
        let mic_btn = div()
            .id("cmp-mic-btn")
            .w(px(28.0))
            .h(px(28.0))
            .rounded_full()
            .flex_shrink_0()
            .cursor_pointer()
            .hover(move |s| s.bg(attach_hover))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                a_mic(this, ComposerBarAction::StartVoice(ev.position), cx);
            }))
            .child(tg_icon(IconName::Mic, 16.0, palette.text_secondary));

        let send_btn = if show_send {
            div()
                .id("cmp-send-btn")
                .w(px(30.0))
                .h(px(30.0))
                .rounded_full()
                .flex_shrink_0()
                .bg(rgb(accent))
                .hover(move |s| s.bg(rgb(accent_hover)))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_snd(this, ComposerBarAction::SendOrSave, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_sopt(this, ComposerBarAction::OpenSendOptions, cx);
                }))
                .child(tg_icon(
                    if is_editing { IconName::SelectCheck } else { IconName::ArrowUp },
                    14.5,
                    0xFFFFFF,
                ))
        } else {
            let _ = &a_sopt;
            let disabled_bg = if send_disabled {
                TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, if is_dark { 0.14 } else { 0.10 })
            } else {
                TdesktopThemePalette::rgba_with_alpha(accent, if is_dark { 0.22 } else { 0.14 })
            };
            let disabled_fg = if send_disabled {
                TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, if is_dark { 0.42 } else { 0.40 })
            } else {
                TdesktopThemePalette::rgba_with_alpha(accent, if is_dark { 0.65 } else { 0.55 })
            };
            div()
                .id("cmp-send-btn")
                .w(px(30.0))
                .h(px(30.0))
                .rounded_full()
                .flex_shrink_0()
                .bg(disabled_bg)
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .child(tg_icon_rgba(
                    IconName::ArrowUp,
                    14.5,
                    disabled_fg,
                ))
        };

        return div()
            .w_full()
            .px(px(20.0))
            .pt(px(6.0))
            .pb(px(16.0))
            .bg(rgb(palette.composer_bg))
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(740.0))
                    .flex()
                    .items_end()
                    .gap(px(8.0))
                    .when_some(bot_menu_view, |row, btn| {
                        row.child(
                            div()
                                .h(px(palette.scaled_px(46.0)))
                                .flex()
                                .items_center()
                                .flex_shrink_0()
                                .child(btn),
                        )
                    })
                    .child(
                        div()
                            .id("cmp-input-wrap")
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(palette.scaled_px(46.0)))
                            .h(scaled_capsule_h)
                            .pl(px(8.0))
                            .pr(px(8.0))
                            .py(px(6.0))
                            .rounded(px(23.0))
                            .bg(rgb(pill_bg))
                            .border_1()
                            .border_color(pill_border)
                            .shadow(pill_shadow)
                            .cursor_text()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                a_foc(this, ComposerBarAction::FocusInput, cx);
                            }))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("cmp-attach-btn")
                                    .w(px(30.0))
                                    .h(px(30.0))
                                    .rounded_full()
                                    .flex_shrink_0()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(attach_hover))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        a_att(this, ComposerBarAction::ToggleAttachments, cx);
                                    }))
                                    .text_size(px(20.0))
                                    .line_height(px(20.0))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(rgb(palette.text_secondary))
                                    .child("+"),
                            )
                            .child(
                                div()
                                    .id("cmp-text-scroll")
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .px(px(4.0))
                                    .when(overflow_scroll, |el| el.overflow_y_scroll().max_h(px(164.0)))
                                    .child(text_el),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .flex_shrink_0()
                                    .when(has_bot_keyboard, |cluster| {
                                        let kb_active_bg = TdesktopThemePalette::rgba_with_alpha(
                                            palette.accent,
                                            if is_dark { 0.22 } else { 0.14 },
                                        );
                                        cluster.child(
                                            div()
                                                .id("cmp-bot-kb-toggle")
                                                .w(px(28.0))
                                                .h(px(28.0))
                                                .rounded_full()
                                                .cursor_pointer()
                                                .when(is_bot_keyboard_open, |d| d.bg(kb_active_bg))
                                                .hover(move |s| s.bg(attach_hover))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    a_kb(this, ComposerBarAction::ToggleBotKeyboard, cx);
                                                }))
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(if is_bot_keyboard_open {
                                                    palette.accent
                                                } else {
                                                    palette.text_secondary
                                                }))
                                                .child("⌘"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("cmp-fmt-toggle")
                                            .px(px(6.0))
                                            .py(px(3.0))
                                            .rounded_full()
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(attach_hover))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                a_fmt(this, ComposerBarAction::ToggleFormatting, cx);
                                            }))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if is_formatting_open {
                                                palette.accent
                                            } else {
                                                palette.text_secondary
                                            }))
                                            .child("Aa"),
                                    )
                                    .child(
                                        div()
                                            .id("cmp-emoji-toggle")
                                            .w(px(28.0))
                                            .h(px(28.0))
                                            .rounded_full()
                                            .cursor_pointer()
                                            .hover(move |s| s.bg(attach_hover))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                a_emo(this, ComposerBarAction::ToggleEmojiPicker, cx);
                                            }))
                                            .child(tg_icon(
                                                IconName::SmileEmoji,
                                                16.5,
                                                if is_emoji_open {
                                                    palette.accent
                                                } else {
                                                    palette.text_secondary
                                                },
                                            )),
                                    )
                                    .child(mic_btn)
                                    .child(send_btn),
                            ),
                    ),
            )
            .into_any_element();
    }

    let scaled_content_h = px(palette.scaled_px(raw_content_h));
    let input_border_hex = if is_focused {
        palette.accent
    } else {
        palette.border_fg
    };
    let (send_bg, send_hover, send_icon_fg) = (palette.accent, palette.accent_hover, 0xFFFFFF);
    div()
        .w_full()
        .min_h(px(palette.scaled_px(46.0)))
        .px(px(palette.scaled_px(10.0)))
        .py(px(palette.scaled_px(6.0)))
        .bg(rgb(palette.composer_bg))
        .border_t_1()
        .border_color(rgb(palette.border_fg))
        .flex()
        .items_end()
        .gap(px(palette.scaled_px(6.0)))
        .when_some(bot_menu_view, |el, btn| {
            el.child(div().pb(px(5.0)).flex_shrink_0().child(btn))
        })
        .child(
            div()
                .id("cmp-attach-btn")
                .w(px(palette.scaled_px(36.0)))
                .h(px(palette.scaled_px(44.0)))
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(move |this, _, _, cx| a_att(this, ComposerBarAction::ToggleAttachments, cx)))
                .child(tg_icon(IconName::Paperclip, palette.scaled_px(20.0), palette.icon_fg)),
        )
        .child(
            div()
                .id("cmp-input-wrap")
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(palette.scaled_px(44.0)))
                .h(scaled_content_h)
                .px(px(palette.scaled_px(12.0)))
                .py(px(palette.scaled_px(10.0)))
                .rounded(px(palette.scaled_px(14.0)))
                .bg(rgb(palette.input_bg))
                .border_1()
                .border_color(rgb(input_border_hex))
                .cursor_text()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    a_foc(this, ComposerBarAction::FocusInput, cx);
                }))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .id("cmp-text-scroll")
                        .flex_1()
                        .min_w(px(0.0))
                        .when(overflow_scroll, |el| el.overflow_y_scroll().max_h(px(172.0)))
                        .child(text_el),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .when(has_bot_keyboard, |cluster| {
                            cluster.child(
                                div()
                                    .id("cmp-bot-kb-toggle")
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .rounded_full()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        a_kb(this, ComposerBarAction::ToggleBotKeyboard, cx);
                                    }))
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(if is_bot_keyboard_open {
                                        palette.icon_active
                                    } else {
                                        palette.icon_fg
                                    }))
                                    .child("⌘"),
                            )
                        })
                        .child(
                            div()
                                .id("cmp-fmt-toggle")
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                                .on_click(cx.listener(move |this, _, _, cx| a_fmt(this, ComposerBarAction::ToggleFormatting, cx)))
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(if is_formatting_open { palette.icon_active } else { palette.icon_fg }))
                                .child("Aa"),
                        )
                        .child(
                            div()
                                .id("cmp-emoji-toggle")
                                .w(px(30.0))
                                .h(px(30.0))
                                .rounded_full()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_click(cx.listener(move |this, _, _, cx| a_emo(this, ComposerBarAction::ToggleEmojiPicker, cx)))
                                .child(tg_icon(IconName::SmileEmoji, 20.0, if is_emoji_open { palette.icon_active } else { palette.icon_fg })),
                        ),
                ),
        )
        .child(if show_send {
            div()
                .id("cmp-send-btn")
                .w(px(44.0))
                .h(px(46.0))
                .rounded_full()
                .bg(rgb(send_bg))
                .hover(move |s| s.bg(rgb(send_hover)))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    a_snd(this, ComposerBarAction::SendOrSave, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
                    a_sopt(this, ComposerBarAction::OpenSendOptions, cx);
                }))
                .child(tg_icon(if is_editing { IconName::SelectCheck } else { IconName::SendPlane }, 20.0, send_icon_fg))
                .into_any_element()
        } else {
            div()
                .id("cmp-mic-btn")
                .w(px(44.0))
                .h(px(46.0))
                .rounded_full()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    a_mic(this, ComposerBarAction::StartVoice(ev.position), cx);
                }))
                .child(tg_icon(IconName::Mic, 20.0, palette.icon_fg))
                .into_any_element()
        })
        .into_any_element()
}

pub fn render_voice_recording_strip<V: 'static>(
    vg: &VoiceGestureMachine,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerBarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let dur = vg.duration_secs();
    let mins = (dur as u32) / 60;
    let secs = (dur as u32) % 60;
    let tenths = ((dur * 10.0) as u32) % 10;
    let locked = vg.is_locked();
    let a_cancel = on_action.clone();
    let a_lock = on_action.clone();
    let a_send = on_action;

    let is_apple = palette.is_apple_style();
    let voice_row = div()
        .id("cmp-voice-bar")
        .w_full()
        .h(px(if is_apple { 40.0 } else { 58.0 }))
        .px(px(14.0))
        .when(is_apple, |el| {
            el.rounded(px(20.0))
                .bg(rgb(palette.input_bg))
                .border_1()
                .border_color(palette.separator_rgba())
        })
        .when(!is_apple, |el| {
            el.bg(rgb(palette.composer_bg))
                .border_t_1()
                .border_color(rgb(palette.border_fg))
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(div().w(px(10.0)).h(px(10.0)).rounded_full().bg(rgb(palette.danger)))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(palette.text_primary))
                        .child(format!("{:01}:{:02}.{}", mins, secs, tenths)),
                )
                .child(
                    div().flex().items_center().gap(px(2.0)).children((0..14).map(|i| {
                        let h = 6.0 + (((i * 7 + (dur * 10.0) as usize) % 5) as f32) * 3.0;
                        div().w(px(2.5)).h(px(h)).rounded_full().bg(rgb(palette.accent))
                    })),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("voice-cancel-btn")
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                        .on_click(cx.listener(move |this, _, _, cx| a_cancel(this, ComposerBarAction::CancelVoice, cx)))
                        .text_size(px(12.5))
                        .text_color(rgb(palette.danger))
                        .child(tr_key_or(lang_id, "lng_cancel", "Cancel")),
                )
                .when(!locked, |el| {
                    el.child(
                        div()
                            .id("voice-lock-btn")
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded_full()
                            .bg(rgb(palette.card_bg))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_click(cx.listener(move |this, _, _, cx| a_lock(this, ComposerBarAction::LockVoice, cx)))
                            .child(tg_icon(IconName::Lock, 14.0, palette.accent)),
                    )
                })
                .child(
                    div()
                        .id("voice-send-btn")
                        .w(px(if is_apple { 30.0 } else { 40.0 }))
                        .h(px(if is_apple { 30.0 } else { 40.0 }))
                        .rounded_full()
                        .bg(rgb(palette.accent))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(cx.listener(move |this, _, _, cx| a_send(this, ComposerBarAction::EndVoice, cx)))
                        .child(tg_icon(
                            if is_apple { IconName::ArrowUp } else { IconName::SendPlane },
                            if is_apple { 15.0 } else { 18.0 },
                            0xFFFFFF,
                        )),
                ),
        );

    if is_apple {
        div()
            .w_full()
            .px(px(20.0))
            .pt(px(6.0))
            .pb(px(16.0))
            .bg(rgb(palette.composer_bg))
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(740.0)).child(voice_row))
            .into_any_element()
    } else {
        voice_row.into_any_element()
    }
}
