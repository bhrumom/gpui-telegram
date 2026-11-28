//! Shared theme-reactive, localized, SVG-iconed GPUI widgets for all Settings views
//! aligned with OpenBot's `12px`-radius monochrome grouped cards & `64px` rows.

use crate::theme::assets::{IconName, tg_icon_rgba};
use crate::theme::palette::TdesktopThemePalette;
use gpui::prelude::FluentBuilder;
use gpui::*;

pub const SETTINGS_LEFT_NAV_WIDTH_PX: f32 = 220.0;
pub const SETTINGS_TOPBAR_HEIGHT_PX: f32 = 48.0;
pub const SETTINGS_CONTENT_MAX_WIDTH_PX: f32 = 960.0;
pub const SETTINGS_MODAL_MAX_WIDTH_PX: f32 = 1180.0;
pub const SETTINGS_MODAL_MAX_HEIGHT_PX: f32 = 860.0;
pub const SETTINGS_CARD_RADIUS_PX: f32 = 12.0;
pub const SETTINGS_ROW_MIN_HEIGHT_PX: f32 = 64.0;
pub const SETTINGS_CARD_BG_DARK: u32 = 0x1E1E1E;
pub const SETTINGS_CARD_BG_LIGHT: u32 = 0xFFFFFF;

/// State controller for OpenBot's sticky bottom `SaveBarDock` inside `SettingsPage` (`UI-F19`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveBarDockState {
    pub dirty: bool,
    pub saving: bool,
    pub error: Option<String>,
    pub save_count: usize,
    pub discard_count: usize,
}

impl SaveBarDockState {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.error = None;
    }

    pub fn discard(&mut self) {
        self.dirty = false;
        self.saving = false;
        self.error = None;
        self.discard_count = self.discard_count.saturating_add(1);
    }

    pub fn commit_save(
        &mut self,
        toaster: &mut crate::views::app_shell::modal_container::OpenBotToasterState,
    ) {
        self.dirty = false;
        self.saving = false;
        self.error = None;
        self.save_count = self.save_count.saturating_add(1);
        toaster.push_toast(
            crate::views::app_shell::modal_container::OpenBotToastTone::Success,
            "Saved",
            None,
        );
    }

    pub fn fail_save(
        &mut self,
        err: impl Into<String>,
        toaster: &mut crate::views::app_shell::modal_container::OpenBotToasterState,
    ) {
        let msg = err.into();
        self.saving = false;
        self.error = Some(msg.clone());
        toaster.push_toast(
            crate::views::app_shell::modal_container::OpenBotToastTone::Error,
            msg,
            None,
        );
    }

    pub const fn is_visible(&self) -> bool {
        self.dirty || self.saving || self.error.is_some()
    }
}

/// Single-source-of-truth OpenBot 2-column `SettingsPage` layout snapshot (`UI-F19`).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsPageLayoutSnapshot {
    pub open: bool,
    pub left_nav_px: f32,
    pub topbar_height_px: f32,
    pub content_max_px: f32,
    pub modal_max_width_px: f32,
    pub modal_max_height_px: f32,
    pub card_bg_hex: String,
    pub card_radius_px: f32,
    pub row_min_height_px: f32,
    pub save_bar_dock_visible: bool,
    pub search_results_count: usize,
}

/// Computes live `SettingsPage` geometry & card surface tokens directly from `SettingsRouter`.
pub fn compute_settings_page_layout(
    router: &super::SettingsRouter,
) -> SettingsPageLayoutSnapshot {
    let ob = router.palette.openbot_palette();
    let card_rgb = if ob.scheme.is_dark() {
        SETTINGS_CARD_BG_DARK
    } else {
        SETTINGS_CARD_BG_LIGHT
    };
    SettingsPageLayoutSnapshot {
        open: router.is_open,
        left_nav_px: SETTINGS_LEFT_NAV_WIDTH_PX,
        topbar_height_px: SETTINGS_TOPBAR_HEIGHT_PX,
        content_max_px: SETTINGS_CONTENT_MAX_WIDTH_PX,
        modal_max_width_px: SETTINGS_MODAL_MAX_WIDTH_PX,
        modal_max_height_px: SETTINGS_MODAL_MAX_HEIGHT_PX,
        card_bg_hex: crate::views::content_hub::apple_primitives::rgb24_to_hex(card_rgb),
        card_radius_px: SETTINGS_CARD_RADIUS_PX,
        row_min_height_px: SETTINGS_ROW_MIN_HEIGHT_PX,
        save_bar_dock_visible: router.save_bar.is_visible(),
        search_results_count: router.ranked_nav_search_results().len(),
    }
}

pub fn settings_card_bg(pal: TdesktopThemePalette) -> Rgba {
    let ob = pal.openbot_palette();
    rgb(if ob.scheme.is_dark() {
        SETTINGS_CARD_BG_DARK
    } else {
        SETTINGS_CARD_BG_LIGHT
    })
}

pub fn settings_container(pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .min_h_full()
        .px(px(0.0))
        .py(px(pal.scaled_px(2.0)))
        .bg(ob.bg_canvas())
        .flex()
        .flex_col()
        .gap(px(pal.scaled_px(20.0)))
}

/// OpenBot `.settings-page-heading` (`24px` semibold title + concise muted description).
pub fn settings_page_heading(
    title: &str,
    subtitle: Option<&str>,
    pal: TdesktopThemePalette,
) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(pal.scaled_px(4.0)))
        .pb(px(pal.scaled_px(2.0)))
        .child(
            div()
                .text_size(px(pal.scaled_px(24.0)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(title.to_string()),
        )
        .when_some(subtitle.filter(|s| !s.is_empty()), |el, sub| {
            el.child(
                div()
                    .text_size(px(pal.scaled_px(13.0)))
                    .text_color(ob.text_muted())
                    .child(sub.to_string()),
            )
        })
        .into_any_element()
}

/// OpenBot `<SettingsSection>`: section heading + optional muted caption cleanly OUTSIDE above a card.
pub fn settings_section_block(
    title: &str,
    description: Option<&str>,
    pal: TdesktopThemePalette,
    body: impl IntoElement,
) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(pal.scaled_px(8.0)))
        .child(
            div()
                .px(px(2.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(pal.scaled_px(13.5)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(title.to_string()),
                )
                .when_some(description.filter(|s| !s.is_empty()), |el, desc| {
                    el.child(
                        div()
                            .text_size(px(pal.scaled_px(12.0)))
                            .text_color(ob.text_muted())
                            .child(desc.to_string()),
                    )
                }),
        )
        .child(body)
        .into_any_element()
}

/// OpenBot `<ItemGroup class="settings-modal-card">`: 12px rounded card with 1px inset hairline dividers between rows.
pub fn settings_group_card(pal: TdesktopThemePalette, rows: Vec<AnyElement>) -> AnyElement {
    let ob = pal.openbot_palette();
    let card_bg = settings_card_bg(pal);
    let divider_color = ob.border_hairline();
    let mut card = div()
        .w_full()
        .rounded(px(pal.scaled_px(SETTINGS_CARD_RADIUS_PX)))
        .bg(card_bg)
        .border_1()
        .border_color(ob.frame_card_border())
        .overflow_hidden()
        .flex()
        .flex_col();

    for (idx, row) in rows.into_iter().enumerate() {
        if idx > 0 {
            card = card.child(
                div()
                    .w_full()
                    .px(px(pal.scaled_px(16.0)))
                    .child(div().w_full().h(px(1.0)).bg(divider_color)),
            );
        }
        card = card.child(row);
    }
    card.into_any_element()
}

/// OpenBot `<Item class="settings-modal-row">`: title + optional description on left, custom control on right.
pub fn settings_static_row(
    title: &str,
    subtitle: Option<&str>,
    trailing: impl IntoElement,
    pal: TdesktopThemePalette,
) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .min_h(px(pal.scaled_px(56.0)))
        .px(px(pal.scaled_px(16.0)))
        .py(px(pal.scaled_px(12.0)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(pal.scaled_px(16.0)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
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
                .when_some(subtitle.filter(|s| !s.is_empty()), |el, sub| {
                    el.child(
                        div()
                            .text_size(px(pal.scaled_px(12.0)))
                            .text_color(ob.text_muted())
                            .child(sub.to_string()),
                    )
                }),
        )
        .child(div().flex_shrink_0().flex().items_center().child(trailing))
        .into_any_element()
}

pub fn settings_card(pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .rounded(px(pal.scaled_px(SETTINGS_CARD_RADIUS_PX)))
        .bg(settings_card_bg(pal))
        .border_1()
        .border_color(ob.frame_card_border())
        .px(px(pal.scaled_px(16.0)))
        .py(px(pal.scaled_px(12.0)))
        .flex()
        .flex_col()
        .gap(px(pal.scaled_px(8.0)))
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
        .gap(px(pal.scaled_px(10.0)))
        .pb(px(pal.scaled_px(2.0)))
        .child(tg_icon_rgba(icon, pal.scaled_px(17.0), ob.text_muted()))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(pal.scaled_px(15.0)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(title.to_string()),
                )
                .when_some(subtitle.filter(|s| !s.is_empty()), |el, sub| {
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
        .px(px(2.0))
        .text_size(px(pal.scaled_px(13.0)))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ob.text_primary())
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
        .min_h(px(pal.scaled_px(56.0)))
        .px(px(pal.scaled_px(16.0)))
        .py(px(pal.scaled_px(11.0)))
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(pal.scaled_px(14.0)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                on_toggle(this);
                cx.notify();
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(pal.scaled_px(10.0)))
                .child(tg_icon_rgba(icon, pal.scaled_px(15.0), ob.text_muted()))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
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
                        .when_some(subtitle.filter(|s| !s.is_empty()), |el, sub| {
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

pub fn settings_action_row_desc<V: 'static>(
    icon: Option<IconName>,
    title: &str,
    subtitle: Option<&str>,
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
    let clean_val = value_text.trim();
    let show_val = !clean_val.is_empty() && clean_val != "›" && clean_val != ">";

    div()
        .cursor_pointer()
        .w_full()
        .min_h(px(pal.scaled_px(56.0)))
        .px(px(pal.scaled_px(16.0)))
        .py(px(pal.scaled_px(11.0)))
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(pal.scaled_px(12.0)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                on_tap(this);
                cx.notify();
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(pal.scaled_px(10.0)))
                .when_some(icon, |el, ic| {
                    el.child(tg_icon_rgba(ic, pal.scaled_px(15.0), icon_fg))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(pal.scaled_px(13.5)))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg)
                                .child(title.to_string()),
                        )
                        .when_some(subtitle.filter(|s| !s.is_empty()), |el, sub| {
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
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(pal.scaled_px(6.0)))
                .when(show_val, |el| {
                    el.child(
                        div()
                            .text_size(px(pal.scaled_px(12.5)))
                            .text_color(val_fg)
                            .child(clean_val.to_string()),
                    )
                })
                .child(tg_icon_rgba(
                    IconName::ChevronRight,
                    pal.scaled_px(13.0),
                    ob.text_dim(),
                )),
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
    settings_action_row_desc(Some(icon), title, None, value_text, pal, danger, on_tap, cx)
}

pub fn settings_info_row(label: &str, value: &str, pal: TdesktopThemePalette) -> AnyElement {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .min_h(px(pal.scaled_px(50.0)))
        .px(px(pal.scaled_px(16.0)))
        .py(px(pal.scaled_px(11.0)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(pal.scaled_px(12.0)))
        .child(
            div()
                .text_size(px(pal.scaled_px(13.5)))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ob.text_primary())
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(pal.scaled_px(12.5)))
                .text_color(ob.text_muted())
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
        .child(tg_icon_rgba(icon, pal.scaled_px(24.0), ob.text_dim()))
        .child(
            div()
                .text_size(px(pal.scaled_px(12.5)))
                .text_color(ob.text_muted())
                .child(text.to_string()),
        )
        .into_any_element()
}
