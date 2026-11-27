use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::cards::{GenerativeCard, GenerativeCardLayout, GenerativeComponent};

use crate::theme::TdesktopThemePalette;
use crate::views::stage::cards_interactive::InteractiveCardAction;

fn parse_hex_color(raw: Option<&str>, fallback: u32) -> u32 {
    let Some(s) = raw else { return fallback };
    let hex = s.trim().trim_start_matches('#');
    u32::from_str_radix(hex, 16).unwrap_or(fallback)
}

pub fn render_generative_card<V: 'static>(
    msg_id: u64,
    card: &GenerativeCard,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, InteractiveCardAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let safe_title = GenerativeCard::sanitize_text_once(&card.title);
    let safe_subtitle = card.subtitle.as_deref().map(GenerativeCard::sanitize_text_once);
    let safe_status = card.status.as_deref().map(GenerativeCard::sanitize_text_once);

    let mut body = match card.layout {
        GenerativeCardLayout::VerticalStack => div().flex().flex_col().gap(px(6.0)).w_full(),
        GenerativeCardLayout::HorizontalRow => {
            div().flex().flex_row().flex_wrap().items_center().gap(px(6.0)).w_full()
        }
        GenerativeCardLayout::KeyValueGrid => {
            div().flex().flex_row().flex_wrap().gap(px(8.0)).w_full()
        }
    };

    let is_apple = palette.is_apple_style();
    let ob = palette.openbot_palette();
    let status_lc = safe_status.as_deref().unwrap_or("").to_ascii_lowercase();
    let is_answered = status_lc.contains("answered")
        || status_lc.contains("approved")
        || status_lc.contains("done")
        || status_lc.contains("已完成")
        || status_lc.contains("已批准");
    let is_danger = status_lc.contains("danger")
        || status_lc.contains("denied")
        || status_lc.contains("failed")
        || status_lc.contains("error")
        || status_lc.contains("危险")
        || status_lc.contains("拒绝");

    for (idx, comp) in card.components.iter().enumerate() {
        let el: AnyElement = match comp {
            GenerativeComponent::Text { text, bold, color } => {
                let fg = parse_hex_color(color.as_deref(), palette.text_primary);
                div()
                    .text_size(px(if is_apple { 13.5 } else { 12.5 }))
                    .text_color(rgb(fg))
                    .when(*bold, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(GenerativeCard::sanitize_text_once(text))
                    .into_any_element()
            }
            GenerativeComponent::Badge { label, color } => {
                let badge_bg = parse_hex_color(Some(color.as_str()), palette.accent);
                div()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .bg(rgb(badge_bg))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(GenerativeCard::sanitize_text_once(label))
                    .into_any_element()
            }
            GenerativeComponent::Progress { value, max, label } => {
                let ratio = if !value.is_finite() || !max.is_finite() || *max <= 0.0 {
                    0.0
                } else {
                    (*value / *max).clamp(0.0, 1.0)
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .w_full()
                    .when_some(label.as_ref(), |d, lbl| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(if is_apple { ob.text_muted() } else { rgb(palette.text_secondary).into() })
                                .child(GenerativeCard::sanitize_text_once(lbl)),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(if is_apple { ob.bg_control_active() } else { rgb(palette.border_fg).into() })
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(ratio))
                                    .rounded(px(3.0))
                                    .bg(if is_apple { ob.surface_light() } else { rgb(palette.accent).into() }),
                            ),
                    )
                    .into_any_element()
            }
            GenerativeComponent::Button {
                id,
                label,
                action_payload,
            } => {
                let btn_id = id.clone();
                let payload = action_payload.clone();
                let cb = on_action.clone();
                let is_primary = idx == 0;
                let btn_bg: Rgba = if is_apple {
                    if is_primary {
                        ob.surface_light()
                    } else {
                        ob.bg_control()
                    }
                } else {
                    palette.service_pill_rgba()
                };
                let btn_fg: Rgba = if is_apple {
                    if is_primary {
                        ob.text_on_light()
                    } else {
                        ob.text_primary()
                    }
                } else {
                    rgb(palette.service_pill_fg).into()
                };
                div()
                    .id(SharedString::from(format!("a2ui-btn-{}-{}-{}", msg_id, idx, id)))
                    .h(px(if is_apple { 30.0 } else { 26.0 }))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .bg(btn_bg)
                    .when(is_apple && !is_primary, |el| {
                        el.border_1().border_color(ob.border_hairline())
                    })
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.86))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cb(
                            this,
                            InteractiveCardAction::GenerativeButton(
                                msg_id,
                                btn_id.clone(),
                                payload.clone(),
                            ),
                            cx,
                        );
                    }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(btn_fg)
                    .child(GenerativeCard::sanitize_text_once(label))
                    .into_any_element()
            }
            GenerativeComponent::Divider => div()
                .w_full()
                .h(px(1.0))
                .bg(if is_apple { ob.border_hairline() } else { rgb(palette.border_fg).into() })
                .into_any_element(),
        };
        body = body.child(el);
    }

    let card_bg: Rgba = if is_apple { ob.bg_surface() } else { rgb(palette.card_bg).into() };
    let card_border: Rgba = if is_apple {
        if is_danger {
            ob.rgba("--openbot-danger-border").unwrap_or_else(|| ob.danger())
        } else {
            ob.border_strong_hairline()
        }
    } else {
        rgb(palette.border_fg).into()
    };

    div()
        .flex()
        .flex_col()
        .when(is_apple, |el| el.w_full().min_w(px(280.0)).max_w(px(540.0)).p(px(14.0)).rounded(px(14.0)))
        .when(!is_apple, |el| el.w(px(300.0)).p(px(10.0)).rounded(px(10.0)))
        .bg(card_bg)
        .border_1()
        .border_color(card_border)
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(if is_apple { 14.0 } else { 13.5 }))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if is_apple { ob.text_primary() } else { rgb(palette.text_primary).into() })
                        .child(safe_title),
                )
                .when_some(safe_status, |d, st| {
                    let st_bg: Rgba = if is_apple && is_answered {
                        ob.rgba("--openbot-success-soft").unwrap_or_else(|| ob.success())
                    } else if is_apple && is_danger {
                        ob.rgba("--openbot-danger-soft").unwrap_or_else(|| ob.danger())
                    } else if is_apple {
                        ob.bg_control_active()
                    } else {
                        palette.service_pill_rgba()
                    };
                    let st_fg: Rgba = if is_apple && is_answered {
                        ob.success()
                    } else if is_apple && is_danger {
                        ob.danger()
                    } else if is_apple {
                        ob.text_muted()
                    } else {
                        rgb(palette.service_pill_fg).into()
                    };
                    d.child(
                        div()
                            .flex_shrink_0()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded_full()
                            .bg(st_bg)
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(st_fg)
                            .child(st),
                    )
                }),
        )
        .when_some(safe_subtitle, |d, sub| {
            d.child(
                div()
                    .text_size(px(12.5))
                    .text_color(if is_apple { ob.text_muted() } else { rgb(palette.text_secondary).into() })
                    .child(sub),
            )
        })
        .child(body)
        .into_any_element()
}
