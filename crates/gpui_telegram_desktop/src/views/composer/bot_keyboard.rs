//! BotKeyboard: Reply keyboard view component for bots.
//!
//! Upstream reference:
//! - `Telegram/SourceFiles/chat_helpers/bot_keyboard.h:L23-L69`
//! - `Telegram/SourceFiles/chat_helpers/bot_keyboard.cpp:L28-L450`

use gpui::prelude::FluentBuilder;
use gpui::*;
use crate::theme::TdesktopThemePalette;

/// Reply keyboard markup flags (upstream: `bot_keyboard.cpp:L344-L354`).
pub mod flags {
    pub const SINGLE_USE: u32 = 0x01;
    pub const RESIZE: u32 = 0x02;
    pub const PERSISTENT: u32 = 0x04;
    pub const FORCE_REPLY: u32 = 0x08;
}

/// Bot keyboard button kind (upstream: `HistoryMessageMarkupButton::Type`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotKeyboardButtonKind {
    Default,
    Callback(Vec<u8>),
    Url(String),
    RequestPhone,
    RequestLocation,
    RequestPoll,
    SwitchInline { query: String, same_peer: bool },
    WebApp(String),
    Buy,
}

/// Reply keyboard button item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotKeyboardButton {
    pub text: String,
    pub kind: BotKeyboardButtonKind,
}

impl BotKeyboardButton {
    pub fn new(text: impl Into<String>, kind: BotKeyboardButtonKind) -> Self {
        Self {
            text: text.into(),
            kind,
        }
    }
}

/// Authentic BotKeyboard component state.
pub struct BotKeyboard {
    pub rows: Vec<Vec<BotKeyboardButton>>,
    pub placeholder: Option<String>,
    pub single_use: bool,
    pub persistent: bool,
    pub force_reply: bool,
    pub maximize_size: bool,
    pub max_outer_height: f32,
    pub selected_button: Option<(usize, usize)>,
    pub updated_at: i64,
}

impl BotKeyboard {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            placeholder: None,
            single_use: false,
            persistent: false,
            force_reply: false,
            maximize_size: false,
            max_outer_height: 240.0,
            selected_button: None,
            updated_at: chrono::Utc::now().timestamp(),
        }
    }

    pub fn set_markup(
        &mut self,
        rows: Vec<Vec<BotKeyboardButton>>,
        flags_mask: u32,
        placeholder: Option<String>,
    ) {
        self.rows = rows;
        self.placeholder = placeholder;
        self.force_reply = (flags_mask & flags::FORCE_REPLY) != 0;
        self.maximize_size = (flags_mask & flags::RESIZE) == 0;
        self.single_use = (flags_mask & flags::SINGLE_USE) != 0;
        self.persistent = (flags_mask & flags::PERSISTENT) != 0;
        self.selected_button = None;
        self.updated_at = chrono::Utc::now().timestamp();
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.placeholder = None;
        self.single_use = false;
        self.persistent = false;
        self.force_reply = false;
        self.maximize_size = false;
        self.selected_button = None;
        self.updated_at = chrono::Utc::now().timestamp();
    }

    pub fn natural_height(&self) -> f32 {
        let row_height = 36.0;
        let spacing = 6.0;
        if self.rows.is_empty() {
            0.0
        } else {
            (self.rows.len() as f32) * row_height + ((self.rows.len() - 1) as f32) * spacing
        }
    }

    pub fn calculated_height(&self) -> f32 {
        let padding = 8.0; // deltat + deltab
        let h = padding + self.natural_height();
        if self.maximize_size {
            h.max(self.max_outer_height)
        } else {
            h.min(self.max_outer_height)
        }
    }

    pub fn has_markup(&self) -> bool {
        !self.rows.is_empty()
    }

    pub fn render_reply_keyboard<V: 'static>(
        &self,
        palette: &TdesktopThemePalette,
        cx: &mut Context<V>,
        on_press: impl Fn(&mut V, usize, usize, BotKeyboardButton, &mut Context<V>) + Clone + 'static,
    ) -> AnyElement {
        if self.rows.is_empty() {
            return div().into_any_element();
        }
        let is_apple = palette.is_apple_style();
        let container_bg = if is_apple {
            palette.composer_bg
        } else {
            palette.compose_bg
        };
        let btn_bg = if is_apple {
            palette.card_bg
        } else {
            palette.input_bg
        };
        let btn_sel_bg = TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16);
        let btn_hover = palette.menu_hover_bg;
        let border_col = palette.border_fg;
        let text_col = palette.text_primary;

        div()
            .id("composer-bot-reply-keyboard")
            .flex()
            .flex_col()
            .w_full()
            .max_h(px(palette.scaled_px(self.max_outer_height)))
            .overflow_y_scroll()
            .p(px(palette.scaled_px(6.0)))
            .gap(px(palette.scaled_px(6.0)))
            .when(is_apple, |d| {
                d.rounded(px(palette.scaled_px(14.0)))
                    .bg(rgb(container_bg))
                    .border_1()
                    .border_color(palette.separator_rgba())
                    .mb(px(palette.scaled_px(4.0)))
            })
            .when(!is_apple, |d| {
                d.bg(rgb(container_bg))
                    .border_t_1()
                    .border_color(rgb(border_col))
            })
            .children(self.rows.iter().enumerate().map(|(r_idx, row)| {
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap(px(palette.scaled_px(6.0)))
                    .children(row.iter().enumerate().map(|(c_idx, btn)| {
                        let is_sel = self.selected_button == Some((r_idx, c_idx));
                        let btn_clone = btn.clone();
                        let cb = on_press.clone();
                        div()
                            .id(SharedString::from(format!("bot-reply-kb-{r_idx}-{c_idx}")))
                            .flex_1()
                            .min_w(px(0.0))
                            .h(px(palette.scaled_px(36.0)))
                            .px(px(palette.scaled_px(8.0)))
                            .rounded(px(palette.scaled_px(8.0)))
                            .cursor_pointer()
                            .when(is_sel, |d| d.bg(btn_sel_bg))
                            .when(!is_sel, |d| d.bg(rgb(btn_bg)))
                            .hover(move |st| st.bg(rgb(btn_hover)))
                            .border_1()
                            .border_color(rgb(if is_sel { palette.accent } else { border_col }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb(this, r_idx, c_idx, btn_clone.clone(), cx);
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(palette.scaled_px(13.0)))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(text_col))
                                    .truncate()
                                    .child(btn.text.clone()),
                            )
                    }))
            }))
            .into_any_element()
    }
}

impl Default for BotKeyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for BotKeyboard {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.rows.is_empty() {
            return div();
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .p(px(6.0))
            .gap(px(6.0))
            .bg(rgb(0xEDEDED))
            .border_t_1()
            .border_color(rgb(0xD8D8D8))
            .children(self.rows.iter().enumerate().map(|(r_idx, row)| {
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap(px(6.0))
                    .children(row.iter().enumerate().map(|(c_idx, btn)| {
                        let is_sel = self.selected_button == Some((r_idx, c_idx));
                        div()
                            .flex_1()
                            .h(px(36.0))
                            .rounded(px(6.0))
                            .bg(if is_sel { rgb(0xE0E0E0) } else { rgb(0xFFFFFF) })
                            .hover(|st| st.bg(rgb(0xFAFAFA)))
                            .border_1()
                            .border_color(rgb(0xDCDCDC))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(0x222222))
                                    .child(btn.text.clone())
                            )
                    }))
            }))
    }
}
