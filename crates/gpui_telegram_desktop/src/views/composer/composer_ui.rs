//! Main Input Strip, Reply/Edit/Attachment Banners & Voice Record Bar (`composer_ui`).

use std::path::PathBuf;
use gpui::*;
use gpui::prelude::FluentBuilder;

use crate::i18n::{tr_key_or, tr_str};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::stage::openbot_stage_chrome::{
    openbot_stage_icon, render_openbot_reference_chip, OpenBotStageIcon,
};
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
    // Additive M3 OpenBot tri-state ComposerSendControl actions (UI-F12)
    StopActiveTurn,
    StartLiveVoice,
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
    let ob = palette.openbot_palette();
    let is_dark = palette.mode.is_dark();
    let tucked_bg: Rgba = if is_dark { rgb(0x121212).into() } else { rgb(0xF8F8FA).into() };
    let tucked_border: Rgba = if is_dark { rgb(0x222222).into() } else { rgb(0xE6E6EA).into() };
    let hover_bg = ob.hover();
    let row = div()
        .h(px(if is_apple { 36.0 } else { 42.0 }))
        .w_full()
        .px(px(if is_apple { 12.0 } else { 14.0 }))
        .when(is_apple, |el| {
            el.rounded(px(12.0))
                .bg(tucked_bg)
                .border_1()
                .border_color(tucked_border)
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
                .min_w(px(0.0))
                .overflow_hidden()
                .when(!is_apple, |el| el.child(tg_icon(icon, 16.0, palette.accent)))
                .child(
                    div()
                        .w(px(2.5))
                        .h(px(20.0))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(if is_apple { ob.text_dim() } else { rgb(palette.accent).into() }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if is_apple { ob.text_primary() } else { rgb(palette.accent).into() })
                                .child(title.to_string()),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.5))
                                .text_color(if is_apple { ob.text_muted() } else { rgb(palette.text_secondary).into() })
                                .child(snippet.to_string()),
                        ),
                ),
        )
        .child(
            div()
                .id("composer-banner-close")
                .w(px(24.0))
                .h(px(24.0))
                .flex_shrink_0()
                .rounded_full()
                .cursor_pointer()
                .hover(move |s| s.bg(if is_apple { hover_bg } else { rgb(palette.menu_hover_bg).into() }))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| on_close(this, cx)))
                .child(openbot_stage_icon(
                    OpenBotStageIcon::Close,
                    13.0,
                    if is_apple { ob.text_muted() } else { rgb(palette.icon_fg).into() },
                )),
        );

    if is_apple {
        div()
            .w_full()
            .px(px(16.0))
            .pt(px(4.0))
            .bg(ob.bg_canvas())
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(760.0)).child(row))
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
    render_text_input_strip_openbot(
        text,
        cursor,
        selection,
        preedit,
        is_focused,
        is_editing,
        has_attachments,
        is_formatting_open,
        is_emoji_open,
        has_bot_keyboard,
        is_bot_keyboard_open,
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

/// Splits a composer text slice into plain-text runs and completed inline OpenBot `ReferenceChip`s
/// (`@[Name](kind:id)`, `$skill:slug`, `$workflow:slug`, `$routine:slug`, `$mcp:slug`, `@everyone`).
fn render_composer_inline_slice(
    slice: &str,
    scope_prefix: &str,
    palette: &TdesktopThemePalette,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut plain = String::new();
    let mut chip_idx = 0usize;
    let bytes = slice.as_bytes();
    while pos < bytes.len() {
        let rem = &slice[pos..];
        if rem.starts_with("@[") {
            if let Some(cb) = rem[2..].find("](") {
                let label = &rem[2..2 + cb];
                let after = &rem[2 + cb + 2..];
                if let Some(cp) = after.find(')') {
                    let target = &after[..cp];
                    if !label.is_empty() && !target.is_empty() {
                        if !plain.is_empty() {
                            out.push(div().child(std::mem::take(&mut plain)).into_any_element());
                        }
                        let kind = target.split_once(':').map(|(k, _)| k).unwrap_or("agent");
                        out.push(
                            render_openbot_reference_chip(
                                SharedString::from(format!("cmp-chip-{}-{}", scope_prefix, chip_idx)),
                                kind,
                                label,
                                palette,
                            )
                            .into_any_element(),
                        );
                        chip_idx += 1;
                        pos += 2 + cb + 2 + cp + 1;
                        continue;
                    }
                }
            }
        }
        if rem.starts_with('$')
            && (pos == 0 || slice[..pos].ends_with(|c: char| c.is_whitespace()))
            && rem.len() > 2
        {
            let tok_len = 1 + rem[1..]
                .find(|c: char| !c.is_alphanumeric() && !matches!(c, '_' | '-' | ':' | '.'))
                .unwrap_or(rem.len() - 1);
            if tok_len > 2 {
                let tok = &rem[..tok_len];
                if let Some((kind_raw, slug)) = tok[1..].split_once(':') {
                    let kind_lc = kind_raw.to_ascii_lowercase();
                    if matches!(kind_lc.as_str(), "skill" | "workflow" | "routine" | "mcp")
                        && !slug.is_empty()
                    {
                        if !plain.is_empty() {
                            out.push(div().child(std::mem::take(&mut plain)).into_any_element());
                        }
                        out.push(
                            render_openbot_reference_chip(
                                SharedString::from(format!("cmp-chip-{}-{}", scope_prefix, chip_idx)),
                                &kind_lc,
                                tok,
                                palette,
                            )
                            .into_any_element(),
                        );
                        chip_idx += 1;
                        pos += tok_len;
                        continue;
                    }
                }
            }
        }
        if rem.starts_with("@everyone")
            && (rem.len() == 9
                || rem[9..].chars().next().is_some_and(|c| c.is_whitespace()))
        {
            if !plain.is_empty() {
                out.push(div().child(std::mem::take(&mut plain)).into_any_element());
            }
            out.push(
                render_openbot_reference_chip(
                    SharedString::from(format!("cmp-chip-{}-{}", scope_prefix, chip_idx)),
                    "everyone",
                    "@everyone",
                    palette,
                )
                .into_any_element(),
            );
            chip_idx += 1;
            pos += 9;
            continue;
        }
        let ch = rem.chars().next().unwrap();
        plain.push(ch);
        pos += ch.len_utf8();
    }
    if !plain.is_empty() {
        out.push(div().child(plain).into_any_element());
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub fn render_text_input_strip_openbot<V: 'static>(
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
    is_agent_busy: bool,
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
    let a_stop = on_action.clone();
    let a_live_voice = on_action.clone();
    let a_mic = on_action;

    let cur = clamp_char_boundary(text, cursor);
    let before = &text[..cur];
    let after = &text[cur..];
    let is_apple = palette.is_apple_style();
    let ob = palette.openbot_palette();
    let placeholder_text = placeholder_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if is_apple {
                tr_str(lang_id, "输入消息…", "Message…").to_string()
            } else {
                tr_key_or(lang_id, "lng_message_ph", "Write a message...").to_string()
            }
        });

    let composer_font = px(if is_apple {
        15.0
    } else {
        palette.scaled_px(palette.font_size_px as f32)
    });
    let caret_color: Rgba = if is_apple {
        ob.text_primary()
    } else {
        rgb(palette.accent).into()
    };
    let text_el = if is_empty {
        div()
            .w_full()
            .flex()
            .items_center()
            .text_size(composer_font)
            .text_color(if is_apple {
                ob.text_dim()
            } else {
                rgb(palette.text_secondary).into()
            })
            .when(is_focused, |el| {
                el.child(
                    div()
                        .w(px(1.5))
                        .h(px(18.0))
                        .mr(px(2.0))
                        .bg(caret_color),
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
            .child(
                div()
                    .bg(if is_apple {
                        ob.accent_soft()
                    } else {
                        TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.35)
                    })
                    .child(text[s..e].to_string()),
            )
            .child(text[e..].to_string())
            .into_any_element()
    } else if is_apple {
        let before_els = render_composer_inline_slice(before, "b", palette);
        let after_els = render_composer_inline_slice(after, "a", palette);
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(3.0))
            .text_size(composer_font)
            .line_height(px(22.0))
            .text_color(ob.text_primary())
            .children(before_els)
            .when_some(preedit.filter(|p| !p.is_empty()), |el, p| {
                el.child(
                    div()
                        .underline()
                        .text_color(ob.text_secondary())
                        .child(p.to_string()),
                )
            })
            .when(is_focused, |el| {
                el.child(div().w(px(1.5)).h(px(18.0)).bg(caret_color))
            })
            .children(after_els)
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
        let is_expanded = text.contains('\n') || raw_content_h > 46.5 || has_attachments;
        let pill_radius = if is_expanded { 14.0 } else { 24.0 };
        let pill_min_h = if is_expanded { 96.0 } else { 48.0 };
        let pill_border: Rgba = if is_focused {
            ob.text_dim()
        } else {
            ob.border_strong_hairline()
        };
        let hover_bg = ob.hover();
        let icon_muted = ob.text_muted();
        let cta_bg = ob.surface_light();
        let cta_fg = ob.text_on_light();

        let attach_btn = div()
            .id("cmp-attach-btn")
            .w(px(28.0))
            .h(px(28.0))
            .rounded_full()
            .flex_shrink_0()
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                a_att(this, ComposerBarAction::ToggleAttachments, cx);
            }))
            .child(openbot_stage_icon(OpenBotStageIcon::Plus, 16.0, icon_muted));

        let mic_btn = div()
            .id("cmp-mic-btn")
            .w(px(28.0))
            .h(px(28.0))
            .rounded_full()
            .flex_shrink_0()
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                a_mic(this, ComposerBarAction::StartVoice(ev.position), cx);
            }))
            .child(openbot_stage_icon(OpenBotStageIcon::Mic, 15.0, icon_muted));

        // OpenBot tri-state ComposerSendControl (UI-F12):
        // 1) Busy + empty -> StopSquare (#f0f0f0/#1c1c1e)
        // 2) Draft ready -> ArrowUp / Check (#f0f0f0/#1c1c1e)
        // 3) Idle + empty -> AudioLines live voice (#f0f0f0/#1c1c1e)
        let send_btn = if is_agent_busy && is_empty && !has_attachments {
            let _ = (&a_sopt, &a_live_voice);
            div()
                .id("cmp-send-btn")
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .flex_shrink_0()
                .bg(cta_bg)
                .hover(|s| s.opacity(0.86))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_stop(this, ComposerBarAction::StopActiveTurn, cx);
                }))
                .child(openbot_stage_icon(OpenBotStageIcon::StopSquare, 12.0, cta_fg))
        } else if show_send {
            let _ = (&a_stop, &a_live_voice);
            div()
                .id("cmp-send-btn")
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .flex_shrink_0()
                .bg(cta_bg)
                .hover(|s| s.opacity(0.86))
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
                .child(openbot_stage_icon(
                    if is_editing {
                        OpenBotStageIcon::Check
                    } else {
                        OpenBotStageIcon::ArrowUp
                    },
                    14.5,
                    cta_fg,
                ))
        } else if send_disabled {
            let _ = (&a_sopt, &a_stop, &a_live_voice);
            div()
                .id("cmp-send-btn")
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .flex_shrink_0()
                .bg(ob.bg_control_active())
                .opacity(0.45)
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(openbot_stage_icon(OpenBotStageIcon::ArrowUp, 14.0, icon_muted))
        } else {
            let _ = (&a_sopt, &a_stop);
            div()
                .id("cmp-send-btn")
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .flex_shrink_0()
                .bg(cta_bg)
                .hover(|s| s.opacity(0.86))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_live_voice(this, ComposerBarAction::StartLiveVoice, cx);
                }))
                .child(openbot_stage_icon(OpenBotStageIcon::AudioLines, 14.0, cta_fg))
        };

        let _ = (a_fmt, a_emo, is_formatting_open, is_emoji_open);
        let right_cluster = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .flex_shrink_0()
            .when(has_bot_keyboard, |cluster| {
                cluster.child(
                    div()
                        .id("cmp-bot-kb-toggle")
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded_full()
                        .cursor_pointer()
                        .when(is_bot_keyboard_open, |d| d.bg(ob.bg_control_active()))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            a_kb(this, ComposerBarAction::ToggleBotKeyboard, cx);
                        }))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_bot_keyboard_open {
                            ob.text_primary()
                        } else {
                            icon_muted
                        })
                        .child("⌘"),
                )
            })
            .child(mic_btn)
            .child(send_btn);

        return div()
            .w_full()
            .px(px(16.0))
            .pt(px(4.0))
            .pb(px(14.0))
            .bg(ob.bg_canvas())
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.0))
                    .flex()
                    .items_end()
                    .gap(px(8.0))
                    .when_some(bot_menu_view, |row, btn| {
                        row.child(
                            div()
                                .h(px(48.0))
                                .flex()
                                .items_center()
                                .flex_shrink_0()
                                .child(btn),
                        )
                    })
                    .child({
                        let pill_base = div()
                            .id("cmp-input-wrap")
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(pill_min_h))
                            .px(px(10.0))
                            .py(px(if is_expanded { 10.0 } else { 8.0 }))
                            .rounded(px(pill_radius))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(pill_border)
                            .cursor_text()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                a_foc(this, ComposerBarAction::FocusInput, cx);
                            }));
                        if !is_expanded {
                            pill_base
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(attach_btn)
                                .child(
                                    div()
                                        .id("cmp-text-scroll")
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .px(px(2.0))
                                        .child(text_el),
                                )
                                .child(right_cluster)
                        } else {
                            pill_base
                                .flex()
                                .flex_col()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("cmp-text-scroll")
                                        .w_full()
                                        .min_w(px(0.0))
                                        .px(px(4.0))
                                        .when(overflow_scroll, |el| el.overflow_y_scroll().max_h(px(200.0)))
                                        .child(text_el),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(attach_btn)
                                        .child(right_cluster),
                                )
                        }
                    }),
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
    let ob = palette.openbot_palette();
    let voice_cancel_hover: Rgba = if is_apple {
        ob.hover()
    } else {
        rgb(palette.menu_hover_bg).into()
    };
    let voice_row = div()
        .id("cmp-voice-bar")
        .w_full()
        .h(px(if is_apple { 48.0 } else { 58.0 }))
        .px(px(14.0))
        .when(is_apple, |el| {
            el.rounded(px(24.0))
                .bg(ob.bg_surface())
                .border_1()
                .border_color(ob.border_strong_hairline())
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
                .child(div().w(px(9.0)).h(px(9.0)).rounded_full().bg(ob.danger()))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if is_apple { ob.text_primary() } else { rgb(palette.text_primary).into() })
                        .child(format!("{:01}:{:02}.{}", mins, secs, tenths)),
                )
                .child(
                    div().flex().items_center().gap(px(2.5)).children((0..14).map(|i| {
                        let h = 6.0 + (((i * 7 + (dur * 10.0) as usize) % 5) as f32) * 3.0;
                        div()
                            .w(px(2.5))
                            .h(px(h))
                            .rounded_full()
                            .bg(if is_apple { ob.text_muted() } else { rgb(palette.accent).into() })
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
                        .hover(move |s| s.bg(voice_cancel_hover))
                        .on_click(cx.listener(move |this, _, _, cx| a_cancel(this, ComposerBarAction::CancelVoice, cx)))
                        .text_size(px(12.5))
                        .text_color(if is_apple { ob.danger() } else { rgb(palette.danger).into() })
                        .child(tr_key_or(lang_id, "lng_cancel", "Cancel")),
                )
                .when(!locked, |el| {
                    el.child(
                        div()
                            .id("voice-lock-btn")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_full()
                            .bg(if is_apple { ob.bg_control_active() } else { rgb(palette.card_bg).into() })
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_click(cx.listener(move |this, _, _, cx| a_lock(this, ComposerBarAction::LockVoice, cx)))
                            .child(openbot_stage_icon(
                                OpenBotStageIcon::Lock,
                                13.5,
                                if is_apple { ob.text_primary() } else { rgb(palette.accent).into() },
                            )),
                    )
                })
                .child(
                    div()
                        .id("voice-send-btn")
                        .w(px(if is_apple { 28.0 } else { 40.0 }))
                        .h(px(if is_apple { 28.0 } else { 40.0 }))
                        .rounded_full()
                        .bg(if is_apple { ob.surface_light() } else { rgb(palette.accent).into() })
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(cx.listener(move |this, _, _, cx| a_send(this, ComposerBarAction::EndVoice, cx)))
                        .child(if is_apple {
                            openbot_stage_icon(OpenBotStageIcon::ArrowUp, 14.5, ob.text_on_light())
                        } else {
                            tg_icon(IconName::SendPlane, 18.0, 0xFFFFFF).into_any_element()
                        }),
                ),
        );

    if is_apple {
        div()
            .w_full()
            .px(px(16.0))
            .pt(px(4.0))
            .pb(px(14.0))
            .bg(ob.bg_canvas())
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(760.0)).child(voice_row))
            .into_any_element()
    } else {
        voice_row.into_any_element()
    }
}
