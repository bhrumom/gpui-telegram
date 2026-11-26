//! Reusable Apple HIG surface primitives, cover artwork cards, status badges,
//! and UTF-8 char-safe `EditableFieldState` shared across Content Hub & Publish Review Modal.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, linear_color_stop, linear_gradient, px, rgb, Div, ElementId, FontWeight,
    InteractiveElement, ParentElement, SharedString, Stateful, Styled,
};
use gpui_telegram_proto::creations::{CoverArtwork, CreationKind, CreationReviewStatus};

use crate::components::ui::{delete_prev_word, pop_last_char, TextInputController};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

/// Char-boundary-safe text input state supporting ASCII, CJK IME preedit, cursor navigation,
/// Alt-word backspace, and Cmd+A full-selection overwrite.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditableFieldState {
    pub text: String,
    pub cursor_char: usize,
    pub select_all: bool,
    pub ime_preedit: Option<String>,
}

impl EditableFieldState {
    pub fn new(initial: impl Into<String>) -> Self {
        let text = initial.into();
        let cursor_char = text.chars().count();
        Self {
            text,
            cursor_char,
            select_all: false,
            ime_preedit: None,
        }
    }

    pub fn set_text(&mut self, value: impl Into<String>) {
        self.text = value.into();
        self.cursor_char = self.text.chars().count();
        self.select_all = false;
        self.ime_preedit = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor_char = 0;
        self.select_all = false;
        self.ime_preedit = None;
    }

    pub fn select_all_text(&mut self) {
        self.cursor_char = self.text.chars().count();
        self.select_all = !self.text.is_empty();
    }

    pub fn set_ime_preedit(&mut self, preedit: Option<&str>) {
        self.ime_preedit = preedit.filter(|s| !s.is_empty()).map(str::to_string);
    }

    fn char_to_byte_idx(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    /// Splits `self.text` into `(before_cursor, after_cursor)` strictly on a UTF-8 char boundary.
    pub fn split_at_cursor(&self) -> (&str, &str) {
        let clamped_char = self.cursor_char.min(self.text.chars().count());
        let byte_idx = self.char_to_byte_idx(clamped_char);
        (
            self.text.get(..byte_idx).unwrap_or(self.text.as_str()),
            self.text.get(byte_idx..).unwrap_or(""),
        )
    }

    pub fn insert_text(&mut self, text: &str) {
        self.ime_preedit = None;
        if text.is_empty() {
            return;
        }
        if self.select_all {
            self.text.clear();
            self.cursor_char = 0;
            self.select_all = false;
        }
        let clamped_char = self.cursor_char.min(self.text.chars().count());
        let byte_idx = self.char_to_byte_idx(clamped_char);
        self.text.insert_str(byte_idx, text);
        self.cursor_char = clamped_char + text.chars().count();
    }

    pub fn backspace(&mut self, is_alt: bool) -> bool {
        if self.ime_preedit.take().is_some() {
            return true;
        }
        if self.select_all {
            self.text.clear();
            self.cursor_char = 0;
            self.select_all = false;
            return true;
        }
        let total_chars = self.text.chars().count();
        let clamped_char = self.cursor_char.min(total_chars);
        if clamped_char == 0 {
            return false;
        }
        let byte_idx = self.char_to_byte_idx(clamped_char);
        let mut prefix = self.text[..byte_idx].to_string();
        let suffix = self.text[byte_idx..].to_string();
        let changed = if is_alt {
            delete_prev_word(&mut prefix)
        } else {
            pop_last_char(&mut prefix)
        };
        if changed {
            let new_cursor = prefix.chars().count();
            prefix.push_str(&suffix);
            self.text = prefix;
            self.cursor_char = new_cursor;
        }
        changed
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.select_all {
            self.text.clear();
            self.cursor_char = 0;
            self.select_all = false;
            return true;
        }
        let total_chars = self.text.chars().count();
        let clamped_char = self.cursor_char.min(total_chars);
        if clamped_char >= total_chars {
            return false;
        }
        let start = self.char_to_byte_idx(clamped_char);
        let end = self.char_to_byte_idx(clamped_char + 1);
        self.text.replace_range(start..end, "");
        true
    }

    pub fn move_left(&mut self) {
        self.select_all = false;
        self.cursor_char = self.cursor_char.min(self.text.chars().count()).saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.select_all = false;
        let max_c = self.text.chars().count();
        self.cursor_char = self.cursor_char.saturating_add(1).min(max_c);
    }

    pub fn move_home(&mut self) {
        self.select_all = false;
        self.cursor_char = 0;
    }

    pub fn move_end(&mut self) {
        self.select_all = false;
        self.cursor_char = self.text.chars().count();
    }

    pub fn text_and_preedit(&self, fallback_ime: &TextInputController) -> (String, Option<String>) {
        (
            self.text.clone(),
            self.ime_preedit
                .clone()
                .or_else(|| fallback_ime.ime_preedit.clone()),
        )
    }
}

/// Safely parses a `#RRGGBB` or `RRGGBB` hex color string into a `0xRRGGBB` integer.
pub fn parse_hex_color(hex: &str, fallback: u32) -> u32 {
    let trimmed = hex.trim().trim_start_matches('#');
    if trimmed.len() == 6 && trimmed.is_ascii() {
        u32::from_str_radix(trimmed, 16).unwrap_or(fallback)
    } else {
        fallback
    }
}

/// Returns the Apple HIG brand accent color for each creative vertical.
pub fn creation_kind_accent(kind: CreationKind) -> u32 {
    match kind {
        CreationKind::Book => 0xFF9500,
        CreationKind::Music => 0xFA2D48,
        CreationKind::Game => 0x5856D6,
        CreationKind::App => 0x007AFF,
    }
}

/// Localized display label for a [`CreationKind`].
pub fn creation_kind_label(kind: CreationKind, lang_id: &str) -> &'static str {
    match kind {
        CreationKind::Book => tr_str(lang_id, "图书", "Books"),
        CreationKind::Music => tr_str(lang_id, "音乐", "Music"),
        CreationKind::Game => tr_str(lang_id, "游戏", "Games"),
        CreationKind::App => tr_str(lang_id, "应用", "App"),
    }
}

/// Standard grouped macOS card surface with subtle hairline separator border.
pub fn apple_grouped_card(pal: TdesktopThemePalette) -> Div {
    div()
        .w_full()
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
        .overflow_hidden()
}

/// Horizontal hairline divider inset for grouped cards.
pub fn apple_hairline(pal: TdesktopThemePalette) -> Div {
    div().w_full().h(px(1.0)).bg(pal.separator_rgba())
}

/// Renders a 2:3 or 1:1 Apple-style gradient cover artwork card with safe `#RRGGBB` parsing.
pub fn cover_artwork_box(
    cover: &CoverArtwork,
    width_px: f32,
    height_px: f32,
    radius_px: f32,
    fallback_accent: u32,
) -> Div {
    let start = parse_hex_color(&cover.gradient_start_hex, fallback_accent);
    let end = parse_hex_color(&cover.gradient_end_hex, 0x1C1C1E);
    let glyph = if cover.icon_emoji.trim().is_empty() {
        "✦".to_string()
    } else {
        cover.icon_emoji.clone()
    };
    let emoji_size = (width_px.min(height_px) * 0.36).clamp(16.0, 44.0);
    div()
        .w(px(width_px))
        .h(px(height_px))
        .flex_shrink_0()
        .rounded(px(radius_px))
        .bg(linear_gradient(
            135.0,
            linear_color_stop(rgb(start), 0.0),
            linear_color_stop(rgb(end), 1.0),
        ))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.16))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .child(
            div()
                .text_size(px(emoji_size))
                .text_color(rgb(0xFFFFFF))
                .child(glyph),
        )
}

/// Renders a semantic Apple pill badge for a creation's [`CreationReviewStatus`].
pub fn review_status_badge(
    status: CreationReviewStatus,
    lang_id: &str,
    pal: TdesktopThemePalette,
) -> Div {
    let (fg, bg_alpha, label) = match status {
        CreationReviewStatus::Draft => (
            pal.text_secondary,
            0.14,
            tr_str(lang_id, "草稿", "Draft"),
        ),
        CreationReviewStatus::PendingReview => (
            0xFF9F0A,
            0.18,
            tr_str(lang_id, "审核中", "Pending Review"),
        ),
        CreationReviewStatus::Published => (
            0x30D158,
            0.18,
            tr_str(lang_id, "已上架", "Published"),
        ),
        CreationReviewStatus::Rejected => (
            pal.danger,
            0.18,
            tr_str(lang_id, "已退回", "Rejected"),
        ),
        CreationReviewStatus::Unpublished => (
            0x8E8E93,
            0.18,
            tr_str(lang_id, "已下架", "Unpublished"),
        ),
    };
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(TdesktopThemePalette::rgba_with_alpha(fg, bg_alpha))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(fg, 0.30))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(fg))
        .child(label)
}

/// Renders an Apple segmented filter pill button.
pub fn filter_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let label_str: SharedString = label.into();
    let active_bg = rgb(pal.accent);
    let inactive_bg = pal.card_surface_rgba();
    let hover_bg = pal.row_hover_rgba();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(12.0))
        .py(px(5.0))
        .rounded(px(999.0))
        .when(active, |el| {
            el.bg(active_bg)
                .text_color(rgb(pal.active_button_fg))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!active, |el| {
            el.bg(inactive_bg)
                .border_1()
                .border_color(pal.separator_rgba())
                .text_color(rgb(pal.text_secondary))
                .hover(move |s| s.bg(hover_bg))
        })
        .text_size(px(12.0))
        .child(label_str)
}

/// Renders a compact star rating pill (`★ 4.8 (12)`).
pub fn rating_pill(rating_avg: f32, rating_count: u32, pal: TdesktopThemePalette) -> Div {
    let text = if rating_count > 0 {
        format!("★ {:.1} ({})", rating_avg, rating_count)
    } else {
        "★ New".to_string()
    };
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(6.0))
        .bg(TdesktopThemePalette::rgba_with_alpha(0xFF9F0A, 0.14))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(if pal.mode.is_dark() { 0xFFD60A } else { 0xB25000 }))
        .child(text)
}

/// Renders a full-bleed Apple vertical hero banner card.
pub fn hero_banner(
    id: &'static str,
    kicker: &str,
    title: &str,
    subtitle: &str,
    start_hex: &str,
    end_hex: &str,
    icon: IconName,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let start = parse_hex_color(start_hex, pal.accent);
    let end = parse_hex_color(end_hex, 0x1C1C1E);
    div()
        .id(id)
        .w_full()
        .rounded(px(16.0))
        .p(px(20.0))
        .bg(linear_gradient(
            135.0,
            linear_color_stop(rgb(start), 0.0),
            linear_color_stop(rgb(end), 1.0),
        ))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.18))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.80))
                        .child(kicker.to_string()),
                )
                .child(
                    div()
                        .text_size(px(22.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.88))
                        .child(subtitle.to_string()),
                ),
        )
        .child(
            div()
                .w(px(52.0))
                .h(px(52.0))
                .rounded(px(14.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.18))
                .flex()
                .items_center()
                .justify_center()
                .child(tg_icon(icon, 26.0, 0xFFFFFF)),
        )
}

/// Renders an Apple HIG empty-state card with an icon, description, and `@CreatorStudioBot` CTA button.
pub fn empty_state_card_shell(
    icon: IconName,
    title: &str,
    subtitle: &str,
    cta_btn: Stateful<Div>,
    pal: TdesktopThemePalette,
) -> Div {
    apple_grouped_card(pal)
        .p(px(28.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(
            div()
                .w(px(48.0))
                .h(px(48.0))
                .rounded(px(14.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                .flex()
                .items_center()
                .justify_center()
                .child(tg_icon(icon, 24.0, pal.accent)),
        )
        .child(
            div()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_primary))
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgb(pal.text_secondary))
                .child(subtitle.to_string()),
        )
        .child(div().pt(px(4.0)).child(cta_btn))
}

/// Renders a styled Apple input field showing committed text, inline IME preedit, focus ring,
/// and `Cmd+A` selection highlight.
pub fn render_editable_field_box(
    id: &'static str,
    field: &EditableFieldState,
    placeholder: &str,
    focused: bool,
    multiline: bool,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let border_col = if focused {
        TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.85)
    } else {
        pal.separator_rgba()
    };
    let preedit_opt = if focused {
        field.ime_preedit.clone()
    } else {
        None
    };
    let has_preedit = preedit_opt.is_some();
    let is_empty = field.text.is_empty() && !has_preedit;
    let display_committed = field.text.clone();
    let (before_cur, after_cur) = field.split_at_cursor();
    let before_owned = before_cur.to_string();
    let after_owned = after_cur.to_string();
    let selected_all = focused && field.select_all && !field.text.is_empty();

    div()
        .id(id)
        .cursor_text()
        .w_full()
        .when(multiline, |el| el.min_h(px(64.0)).items_start())
        .when(!multiline, |el| el.min_h(px(34.0)).items_center())
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(rgb(pal.input_bg))
        .border_1()
        .border_color(border_col)
        .flex()
        .flex_wrap()
        .gap(px(0.0))
        .text_size(px(13.0))
        .when(is_empty, |el| {
            el.when(focused, |f_el| {
                f_el.child(
                    div()
                        .w(px(1.5))
                        .h(px(15.0))
                        .mr(px(1.0))
                        .bg(rgb(pal.accent)),
                )
            })
            .child(
                div()
                    .text_color(rgb(pal.text_secondary))
                    .child(placeholder.to_string()),
            )
        })
        .when(!is_empty && selected_all, |el| {
            el.child(
                div()
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.28))
                    .rounded(px(3.0))
                    .text_color(rgb(pal.text_primary))
                    .child(display_committed.clone()),
            )
            .when_some(preedit_opt.clone(), |p_el, pre| {
                p_el.child(
                    div()
                        .text_color(rgb(pal.accent))
                        .underline()
                        .child(pre),
                )
            })
            .when(focused, |f_el| {
                f_el.child(div().w(px(1.5)).h(px(15.0)).bg(rgb(pal.accent)))
            })
        })
        .when(!is_empty && !selected_all && !focused, |el| {
            el.child(
                div()
                    .text_color(rgb(pal.text_primary))
                    .child(display_committed),
            )
        })
        .when(!is_empty && !selected_all && focused, |el| {
            el.when(!before_owned.is_empty(), |b_el| {
                b_el.child(div().text_color(rgb(pal.text_primary)).child(before_owned))
            })
            .when_some(preedit_opt, |p_el, pre| {
                p_el.child(
                    div()
                        .text_color(rgb(pal.accent))
                        .underline()
                        .child(pre),
                )
            })
            .when(focused, |f_el| {
                f_el.child(div().w(px(1.5)).h(px(15.0)).bg(rgb(pal.accent)))
            })
            .when(!after_owned.is_empty(), |a_el| {
                a_el.child(div().text_color(rgb(pal.text_primary)).child(after_owned))
            })
        })
}

/// Formats seconds into `M:SS` / `MM:SS` for Apple Music durations and timers.
pub fn format_duration_secs(secs: u64) -> String {
    let mins = secs / 60;
    let rem = secs % 60;
    format!("{mins}:{rem:02}")
}

/// Formats milliseconds into `M:SS` for playback progress & game timers.
pub fn format_duration_ms(ms: u64) -> String {
    format_duration_secs(ms / 1000)
}

/// Renders an Apple HIG slim horizontal progress bar (`0.0..=1.0`).
pub fn apple_progress_bar(fraction: f32, accent_rgb: u32, pal: TdesktopThemePalette) -> Div {
    let clamped = fraction.clamp(0.0, 1.0);
    div()
        .w_full()
        .h(px(4.0))
        .rounded(px(999.0))
        .bg(pal.separator_rgba())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(gpui::relative(clamped))
                .rounded(px(999.0))
                .bg(rgb(accent_rgb)),
        )
}

/// Renders a subtle section shelf header with title and optional trailing count/badge.
pub fn apple_shelf_header(
    title: impl Into<SharedString>,
    subtitle: Option<String>,
    pal: TdesktopThemePalette,
) -> Div {
    div()
        .w_full()
        .flex()
        .items_baseline()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(17.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(title.into()),
        )
        .when_some(subtitle, |el, sub| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(pal.text_secondary))
                    .child(sub),
            )
        })
}

/// Renders an Apple-style accent-colored segmented sub-tab button for vertical navigation bars.
pub fn vertical_sub_tab_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    accent_rgb: u32,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let label_str: SharedString = label.into();
    let hover_bg = pal.row_hover_rgba();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(13.0))
        .py(px(6.0))
        .rounded(px(999.0))
        .when(active, |el| {
            el.bg(rgb(accent_rgb))
                .text_color(rgb(0xFFFFFF))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!active, |el| {
            el.bg(TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.06))
                .text_color(rgb(pal.text_secondary))
                .hover(move |s| s.bg(hover_bg))
        })
        .text_size(px(12.0))
        .whitespace_nowrap()
        .child(label_str)
}

