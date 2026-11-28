use crate::i18n::tr_str;
use crate::theme::{
    assets::{IconName, tg_icon_rgba},
    TdesktopThemePalette,
};
use crate::views::boxes::modal_types::ActiveTextField;
use crate::views::modal_boxes_suite::ActiveModalDialog;
use gpui::*;

pub type DialogMut<V> = fn(&mut V) -> &mut ActiveModalDialog;
pub type DialogClose<V> = fn(&mut V);

pub fn modal_card(width: f32, pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    let card_bg = if ob.scheme.is_dark() {
        rgb(0x161616)
    } else {
        ob.bg_surface()
    };
    let dialog_ring = ob
        .rgba("--openbot-dialog-ring")
        .unwrap_or_else(|| ob.border_strong_hairline());
    div()
        .w(px(width))
        .max_h(px(620.0))
        .rounded(px(16.0))
        .bg(card_bg)
        .border_1()
        .border_color(dialog_ring)
        .p(px(20.0))
        .shadow(pal.popover_shadows())
        .flex()
        .flex_col()
        .gap(px(12.0))
        .on_mouse_down(MouseButton::Left, |_e, _w, cx| {
            cx.stop_propagation();
        })
}

pub const CONFIRM_DIALOG_MAX_WIDTH_PX: f32 = 460.0;
pub const CONFIRM_DIALOG_RADIUS_PX: f32 = 16.0;
pub const CONFIRM_DIALOG_DARK_BG: u32 = 0x161616;

/// Interactive state machine for OpenBot `ConfirmDialog` (`UI-F19`):
/// focuses `"cancel"` by default on destructive actions, blocks backdrop dismissal while pending,
/// and restores focus to the opener upon dismissal/resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialogState {
    pub open: bool,
    pub destructive: bool,
    pub pending: bool,
    pub initial_focus: &'static str,
    pub focus_restored_to_opener: bool,
}

impl ConfirmDialogState {
    pub fn open_dialog(destructive: bool) -> Self {
        Self {
            open: true,
            destructive,
            pending: false,
            initial_focus: if destructive { "cancel" } else { "confirm" },
            focus_restored_to_opener: false,
        }
    }

    pub fn closed(destructive: bool) -> Self {
        Self {
            open: false,
            destructive,
            pending: false,
            initial_focus: if destructive { "cancel" } else { "confirm" },
            focus_restored_to_opener: false,
        }
    }

    pub fn set_pending(&mut self, pending: bool) {
        self.pending = pending;
    }

    pub fn backdrop_dismiss_blocked(&self) -> bool {
        self.pending
    }

    pub fn try_dismiss_backdrop(&mut self) -> bool {
        if !self.open || self.pending {
            return false;
        }
        self.open = false;
        self.focus_restored_to_opener = true;
        true
    }

    pub fn resolve_and_restore_focus(&mut self) -> bool {
        self.pending = false;
        self.open = false;
        self.focus_restored_to_opener = true;
        true
    }
}

/// Single-source-of-truth layout snapshot for OpenBot `ConfirmDialog` (`UI-F19`).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmDialogLayoutSnapshot {
    pub open: bool,
    pub destructive: bool,
    pub pending: bool,
    pub max_width_px: f32,
    pub radius_px: f32,
    pub bg_hex: String,
    pub initial_focus: &'static str,
    pub backdrop_dismiss_blocked: bool,
    pub focus_restored_to_opener: bool,
    pub z_index: usize,
}

pub fn compute_confirm_dialog_layout(
    state: &ConfirmDialogState,
    pal: &TdesktopThemePalette,
) -> ConfirmDialogLayoutSnapshot {
    let ob = pal.openbot_palette();
    let bg_rgb = if ob.scheme.is_dark() {
        CONFIRM_DIALOG_DARK_BG
    } else {
        crate::views::content_hub::apple_primitives::rgba_to_rgb24(ob.bg_surface())
    };
    ConfirmDialogLayoutSnapshot {
        open: state.open,
        destructive: state.destructive,
        pending: state.pending,
        max_width_px: CONFIRM_DIALOG_MAX_WIDTH_PX,
        radius_px: CONFIRM_DIALOG_RADIUS_PX,
        bg_hex: crate::views::content_hub::apple_primitives::rgb24_to_hex(bg_rgb),
        initial_focus: state.initial_focus,
        backdrop_dismiss_blocked: state.backdrop_dismiss_blocked(),
        focus_restored_to_opener: state.focus_restored_to_opener,
        z_index: crate::theme::OPENBOT_LAYER_DIALOG,
    }
}

/// OpenBot `ConfirmDialog` card shell (`460px` max-width, `16px` radius, `#161616`/`#ffffff`,
/// `1px` dialog ring, `40px` circular icon badge, body slot, and `border-t` actions bar).
pub fn openbot_confirm_dialog_shell<V: 'static>(
    icon: IconName,
    title: &str,
    destructive: bool,
    pal: TdesktopThemePalette,
    body: AnyElement,
    confirm_btn: AnyElement,
    lang: &str,
    on_close: DialogClose<V>,
    cx: &mut Context<V>,
) -> Div {
    let ob = pal.openbot_palette();
    let state = ConfirmDialogState::open_dialog(destructive);
    let layout = compute_confirm_dialog_layout(&state, &pal);
    let card_bg = rgb(crate::views::content_hub::apple_primitives::parse_hex_color(
        &layout.bg_hex,
        CONFIRM_DIALOG_DARK_BG,
    ));
    let dialog_ring = ob
        .rgba("--openbot-dialog-ring")
        .unwrap_or_else(|| ob.border_strong_hairline());
    let badge_bg = if destructive {
        ob.rgba("--openbot-danger-soft").unwrap_or_else(|| {
            TdesktopThemePalette::rgba_with_alpha(
                if ob.scheme.is_dark() { 0xFD2F3B } else { 0xE8222D },
                if ob.scheme.is_dark() { 0.16 } else { 0.12 },
            )
        })
    } else {
        ob.bg_control_active()
    };
    let badge_icon_color = if destructive {
        ob.danger()
    } else {
        ob.text_secondary()
    };
    let hover_close = ob.hover();

    div()
        .w_full()
        .max_w(px(layout.max_width_px))
        .max_h(px(620.0))
        .rounded(px(layout.radius_px))
        .bg(card_bg)
        .border_1()
        .border_color(dialog_ring)
        .shadow(pal.popover_shadows())
        .overflow_hidden()
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_e, _w, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .w_full()
                .px(px(24.0))
                .pt(px(24.0))
                .pb(px(18.0))
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .w(px(40.0))
                                        .h(px(40.0))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(badge_bg)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(tg_icon_rgba(icon, 20.0, badge_icon_color)),
                                )
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .child(title.to_string()),
                                ),
                        )
                        .child(
                            div()
                                .cursor_pointer()
                                .w(px(28.0))
                                .h(px(28.0))
                                .flex_shrink_0()
                                .rounded_full()
                                .hover(move |s| s.bg(hover_close))
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _e, _w, cx| {
                                        on_close(this);
                                        cx.notify();
                                    }),
                                )
                                .child(tg_icon_rgba(IconName::Close, 15.0, ob.text_muted())),
                        ),
                )
                .child(body),
        )
        .child(
            div()
                .w_full()
                .px(px(24.0))
                .py(px(16.0))
                .border_t_1()
                .border_color(dialog_ring)
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(modal_footer_cancel(lang, pal, on_close, cx))
                .child(confirm_btn),
        )
}

pub fn modal_header<V: 'static>(
    icon: IconName,
    title: &str,
    pal: TdesktopThemePalette,
    on_close: DialogClose<V>,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let ob = pal.openbot_palette();
    let hover_close = ob.hover();
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .pb(px(10.0))
        .border_b_1()
        .border_color(ob.border_hairline())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded_full()
                        .bg(ob.bg_control_active())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon_rgba(icon, 15.0, ob.text_primary())),
                )
                .child(
                    div()
                        .text_size(px(15.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(title.to_string()),
                ),
        )
        .child(
            div()
                .cursor_pointer()
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .hover(move |s| s.bg(hover_close))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _e, _w, cx| {
                        on_close(this);
                        cx.notify();
                    }),
                )
                .child(tg_icon_rgba(IconName::Close, 15.0, ob.text_muted())),
        )
}

pub fn modal_field<V: 'static>(
    label: &str,
    value: &str,
    placeholder: &str,
    field: ActiveTextField,
    is_focused: bool,
    pal: TdesktopThemePalette,
    access: DialogMut<V>,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let ob = pal.openbot_palette();
    let display = if value.is_empty() {
        placeholder.to_string()
    } else {
        value.to_string()
    };
    let text_col = if value.is_empty() {
        ob.text_muted()
    } else {
        ob.text_primary()
    };

    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ob.text_secondary())
                .child(label.to_string()),
        )
        .child(
            div()
                .cursor_pointer()
                .w_full()
                .px(px(11.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(ob.bg_control())
                .border_1()
                .border_color(if is_focused {
                    ob.text_muted()
                } else {
                    ob.border_strong_hairline()
                })
                .text_size(px(13.0))
                .text_color(text_col)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _e, _w, cx| {
                        access(this).active_field = field;
                        cx.notify();
                    }),
                )
                .child(display),
        )
}

pub fn modal_error_banner(err: Option<&String>, pal: TdesktopThemePalette) -> AnyElement {
    let ob = pal.openbot_palette();
    let err_bg = ob.rgba("--openbot-danger-soft").unwrap_or_else(|| {
        TdesktopThemePalette::rgba_with_alpha(
            if ob.scheme.is_dark() { 0xFD2F3B } else { 0xE8222D },
            0.14,
        )
    });
    match err {
        Some(msg) => div()
            .w_full()
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(8.0))
            .bg(err_bg)
            .text_size(px(12.0))
            .text_color(ob.danger())
            .child(msg.clone())
            .into_any_element(),
        None => div().into_any_element(),
    }
}

pub fn modal_footer_cancel<V: 'static>(
    lang: &str,
    pal: TdesktopThemePalette,
    on_close: DialogClose<V>,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let ob = pal.openbot_palette();
    let hover_bg = ob.hover();
    let hover_fg = ob.text_primary();
    div()
        .cursor_pointer()
        .px(px(14.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .text_size(px(13.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ob.text_secondary())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                on_close(this);
                cx.notify();
            }),
        )
        .child(tr_str(lang, "取消", "Cancel").to_string())
}

pub fn modal_toggle_chip<V: 'static>(
    label: &str,
    active: bool,
    pal: TdesktopThemePalette,
    on_click: impl Fn(&mut V) + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    div()
        .cursor_pointer()
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(if active {
            ob.bg_control_active()
        } else {
            ob.bg_control()
        })
        .hover(move |s| s.bg(hover_bg))
        .border_1()
        .border_color(if active {
            ob.border_strong_hairline()
        } else {
            ob.border_hairline()
        })
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active {
            ob.text_primary()
        } else {
            ob.text_secondary()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                on_click(this);
                cx.notify();
            }),
        )
        .child(tg_icon_rgba(
            IconName::SelectCheck,
            13.0,
            if active {
                ob.text_primary()
            } else {
                ob.text_dim()
            },
        ))
        .child(label.to_string())
}
