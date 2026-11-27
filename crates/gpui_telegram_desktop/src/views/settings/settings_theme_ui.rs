//! Shared theme-reactive, localized, SVG-iconed GPUI widgets for all Settings views
//! aligned with OpenBot's `12px`-radius monochrome grouped cards & `64px` rows.

use crate::theme::assets::{IconName, tg_icon_rgba};
use crate::theme::palette::TdesktopThemePalette;
use gpui::prelude::FluentBuilder;
use gpui::*;

pub fn settings_container(pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .min_h_full()
        .px(px(0.0))
        .py(px(pal.scaled_px(4.0)))
        .bg(ob.bg_canvas())
        .flex()
        .flex_col()
        .gap(px(pal.scaled_px(16.0)))
}

pub fn settings_card(pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .rounded(px(pal.scaled_px(12.0)))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.frame_card_border())
        .px(px(pal.scaled_px(16.0)))
        .py(px(pal.scaled_px(10.0)))
        .flex()
        .flex_col()
        .gap(px(pal.scaled_px(6.0)))
}

pub fn settings_section_header(
    icon: IconName,
    title: &str,
    subtitle: Option<&str>,
    pal: TdesktopThemePalette,
) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .flex()
        .items_center()
        .gap(px(pal.scaled_px(12.0)))
        .pb(px(pal.scaled_px(4.0)))
        .child(
            div()
                .w(px(pal.scaled_px(34.0)))
                .h(px(pal.scaled_px(34.0)))
                .rounded_full()
                .bg(ob.bg_control_active())
                .border_1()
                .border_color(ob.border_hairline())
                .flex()
                .items_center()
                .justify_center()
                .child(tg_icon_rgba(icon, pal.scaled_px(17.0), ob.text_primary())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(pal.scaled_px(16.0)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(title.to_string()),
                )
                .when_some(subtitle, |el, sub| {
                    el.child(
                        div()
                            .text_size(px(pal.scaled_px(12.0)))
                            .text_color(ob.text_muted())
                            .child(sub.to_string()),
                    )
                }),
        )
        .into_any_element()
}

pub fn settings_subsection_title(title: &str, pal: TdesktopThemePalette) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .pt(px(2.0))
        .pb(px(2.0))
        .text_size(px(pal.scaled_px(12.0)))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ob.text_muted())
        .child(title.to_string())
        .into_any_element()
}

pub fn settings_toggle_row<V: 'static>(
    icon: IconName,
    title: &str,
    subtitle: Option<&str>,
    active: bool,
    pal: TdesktopThemePalette,
    on_toggle: impl Fn(&mut V) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let hover_bg = ob.hover();
    div()
        .cursor_pointer()
        .w_full()
        .min_h(px(pal.scaled_px(if subtitle.is_some() { 60.0 } else { 48.0 })))
        .px(px(pal.scaled_px(10.0)))
        .py(px(pal.scaled_px(8.0)))
        .rounded(px(pal.scaled_px(8.0)))
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(pal.scaled_px(12.0)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                on_toggle(this);
                cx.notify();
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(pal.scaled_px(10.0)))
                .child(tg_icon_rgba(icon, pal.scaled_px(16.0), ob.text_muted()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(pal.scaled_px(13.5)))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .child(title.to_string()),
                        )
                        .when_some(subtitle, |el, sub| {
                            el.child(
                                div()
                                    .text_size(px(pal.scaled_px(12.0)))
                                    .text_color(ob.text_muted())
                                    .child(sub.to_string()),
                            )
                        }),
                ),
        )
        .child(
            div()
                .w(px(pal.scaled_px(36.0)))
                .h(px(pal.scaled_px(20.0)))
                .flex_shrink_0()
                .rounded_full()
                .bg(if active {
                    ob.surface_light()
                } else {
                    ob.bg_control_active()
                })
                .border_1()
                .border_color(if active {
                    ob.surface_light()
                } else {
                    ob.border_strong_hairline()
                })
                .px(px(pal.scaled_px(2.0)))
                .flex()
                .items_center()
                .when(active, |el| el.justify_end())
                .when(!active, |el| el.justify_start())
                .child(
                    div()
                        .w(px(pal.scaled_px(14.0)))
                        .h(px(pal.scaled_px(14.0)))
                        .rounded_full()
                        .bg(if active {
                            ob.text_on_light()
                        } else {
                            ob.text_primary()
                        })
                        .shadow(vec![BoxShadow {
                            color: hsla(0.0, 0.0, 0.0, 0.22),
                            offset: point(px(0.0), px(1.0)),
                            blur_radius: px(2.0),
                            spread_radius: px(0.0),
                        }]),
                ),
        )
        .into_any_element()
}

pub fn settings_action_row<V: 'static>(
    icon: IconName,
    title: &str,
    value_text: &str,
    pal: TdesktopThemePalette,
    danger: bool,
    on_tap: impl Fn(&mut V) + 'static,
    cx: &mut Context<V>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let fg = if danger { ob.danger() } else { ob.text_primary() };
    let icon_fg = if danger { ob.danger() } else { ob.text_muted() };
    let val_fg = if danger { ob.danger() } else { ob.text_muted() };
    let hover_bg = if danger {
        ob.rgba("--openbot-danger-soft").unwrap_or_else(|| ob.hover())
    } else {
        ob.hover()
    };
    div()
        .cursor_pointer()
        .w_full()
        .min_h(px(pal.scaled_px(48.0)))
        .px(px(pal.scaled_px(10.0)))
        .py(px(pal.scaled_px(8.0)))
        .rounded(px(pal.scaled_px(8.0)))
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_between()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                on_tap(this);
                cx.notify();
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(pal.scaled_px(10.0)))
                .child(tg_icon_rgba(icon, pal.scaled_px(16.0), icon_fg))
                .child(
                    div()
                        .text_size(px(pal.scaled_px(13.5)))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg)
                        .child(title.to_string()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(pal.scaled_px(6.0)))
                .child(
                    div()
                        .text_size(px(pal.scaled_px(12.5)))
                        .text_color(val_fg)
                        .child(value_text.to_string()),
                )
                .child(tg_icon_rgba(
                    IconName::ChevronRight,
                    pal.scaled_px(13.0),
                    ob.text_dim(),
                )),
        )
        .into_any_element()
}

pub fn settings_info_row(label: &str, value: &str, pal: TdesktopThemePalette) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .px(px(pal.scaled_px(12.0)))
        .py(px(pal.scaled_px(9.0)))
        .rounded(px(pal.scaled_px(8.0)))
        .bg(ob.bg_surface_raised())
        .border_1()
        .border_color(ob.border_hairline())
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(pal.scaled_px(12.5)))
                .text_color(ob.text_muted())
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(pal.scaled_px(12.5)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(value.to_string()),
        )
        .into_any_element()
}

pub fn settings_empty_state(icon: IconName, text: &str, pal: TdesktopThemePalette) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .py(px(pal.scaled_px(24.0)))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(pal.scaled_px(8.0)))
        .child(tg_icon_rgba(icon, pal.scaled_px(26.0), ob.text_dim()))
        .child(
            div()
                .text_size(px(pal.scaled_px(12.5)))
                .text_color(ob.text_muted())
                .child(text.to_string()),
        )
        .into_any_element()
}
