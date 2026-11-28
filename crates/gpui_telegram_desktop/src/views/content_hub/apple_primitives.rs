//! Reusable Apple HIG surface primitives, cover artwork cards, status badges,
//! and UTF-8 char-safe `EditableFieldState` shared across Content Hub & Publish Review Modal.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, linear_color_stop, linear_gradient, px, rgb, AnyElement, Div, ElementId, FontWeight,
    InteractiveElement, ParentElement, SharedString, Stateful, StatefulInteractiveElement, Styled,
};
use gpui_telegram_proto::creations::{CoverArtwork, CreationKind, CreationReviewStatus};

use crate::components::ui::{delete_prev_word, pop_last_char, TextInputController};
use crate::i18n::tr_str;
use crate::theme::{tg_icon_rgba, IconName, TdesktopThemePalette};

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

    pub fn with_text(initial: impl Into<String>) -> Self {
        Self::new(initial)
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

/// Returns the cover fallback accent color for each creative vertical (neutral slate for Apps).
pub fn creation_kind_accent(kind: CreationKind) -> u32 {
    match kind {
        CreationKind::Book => 0x52463A,
        CreationKind::Music => 0x4E3840,
        CreationKind::Game => 0x3D3B52,
        CreationKind::App => 0x3A3A3E,
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

/// Canonical OpenBot 2-column Marketplace left navigation sidebar width (`280px`).
pub const OPENBOT_MARKETPLACE_LEFT_NAV_WIDTH: f32 = 280.0;
/// Canonical OpenBot Marketplace centered content max-width (`880px`).
pub const OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH: f32 = 880.0;
/// Canonical OpenBot top `PageHeader` bar height (`38px`).
pub const OPENBOT_PAGE_HEADER_HEIGHT: f32 = 38.0;

/// Returns `true` when `rgb_val` matches a legacy Telegram/Apple blue pill color (`#007aff`, `#3390ec`, `#2b5278`, `#2aabee`).
pub const fn is_legacy_blue_rgb(rgb_val: u32) -> bool {
    matches!(rgb_val, 0x007AFF | 0x3390EC | 0x2B5278 | 0x2AABEE)
}

/// Converts a GPUI `Rgba` color into a packed `0xRRGGBB` 24-bit integer.
pub fn rgba_to_rgb24(c: gpui::Rgba) -> u32 {
    let r = (c.r.clamp(0.0, 1.0) * 255.0).round() as u32;
    let g = (c.g.clamp(0.0, 1.0) * 255.0).round() as u32;
    let b = (c.b.clamp(0.0, 1.0) * 255.0).round() as u32;
    (r << 16) | (g << 8) | b
}

/// Formats a packed `0xRRGGBB` integer as lowercase `#rrggbb`.
pub fn rgb24_to_hex(rgb_val: u32) -> String {
    format!("#{:06x}", rgb_val & 0x00FF_FFFF)
}

/// Single-source-of-truth OpenBot layout snapshot for all secondary content hubs (`UI-F18`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContentHubShellLayoutSnapshot {
    pub active_page: &'static str,
    pub viewport_width_px: f32,
    pub page_header_height_px: f32,
    pub left_nav_width_px: f32,
    pub content_max_width_px: f32,
    pub canvas_hex: String,
    pub card_surface_hex: String,
    pub hairline_hex: String,
    pub primary_cta_bg_hex: String,
    pub primary_cta_fg_hex: String,
    pub legacy_blue_pill_count: usize,
}

/// Computes live OpenBot Content Hub shell geometry and monochrome token audit (`UI-F18`),
/// deriving `legacy_blue_pill_count` strictly from live palette, CTA, and accent colors.
pub fn compute_content_hub_shell_layout(
    pal: &TdesktopThemePalette,
    vp_w: f32,
    active_page: &'static str,
) -> ContentHubShellLayoutSnapshot {
    let ob = pal.openbot_palette();
    let canvas_rgb = rgba_to_rgb24(ob.bg_canvas());
    let card_rgb = rgba_to_rgb24(ob.bg_surface());
    let hairline_rgb = rgba_to_rgb24(ob.border_hairline());
    let primary_cta_bg_rgb = rgba_to_rgb24(ob.surface_light());
    let primary_cta_fg_rgb = rgba_to_rgb24(ob.text_on_light());
    let active_pill_rgb = rgba_to_rgb24(ob.bg_control_active());

    let mut legacy_blue_pill_count = 0usize;
    if !pal.is_apple_style() || is_legacy_blue_rgb(pal.accent) {
        legacy_blue_pill_count += 1;
    }
    if is_legacy_blue_rgb(primary_cta_bg_rgb) {
        legacy_blue_pill_count += 1;
    }
    if is_legacy_blue_rgb(active_pill_rgb) {
        legacy_blue_pill_count += 1;
    }
    if is_legacy_blue_rgb(super::app_store_view::APPSTORE_ACCENT)
        && (active_page == "app_store" || active_page == "marketplace")
    {
        legacy_blue_pill_count += 1;
    }
    if is_legacy_blue_rgb(creation_kind_accent(CreationKind::App)) {
        legacy_blue_pill_count += 1;
    }

    ContentHubShellLayoutSnapshot {
        active_page,
        viewport_width_px: if vp_w.is_finite() && vp_w > 0.0 { vp_w } else { 0.0 },
        page_header_height_px: OPENBOT_PAGE_HEADER_HEIGHT,
        left_nav_width_px: OPENBOT_MARKETPLACE_LEFT_NAV_WIDTH,
        content_max_width_px: OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH,
        canvas_hex: rgb24_to_hex(canvas_rgb),
        card_surface_hex: rgb24_to_hex(card_rgb),
        hairline_hex: rgb24_to_hex(hairline_rgb),
        primary_cta_bg_hex: rgb24_to_hex(primary_cta_bg_rgb),
        primary_cta_fg_hex: rgb24_to_hex(primary_cta_fg_rgb),
        legacy_blue_pill_count,
    }
}

/// OpenBot monochrome primary CTA button (`#f0f0f0`/`#141414` dark, `#1c1c1e`/`#ffffff` light).
pub(crate) fn openbot_primary_cta_btn(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let layout = compute_content_hub_shell_layout(&pal, 1280.0, "marketplace");
    let cta_bg = rgb(parse_hex_color(&layout.primary_cta_bg_hex, 0xF0F0F0));
    let cta_fg = rgb(parse_hex_color(&layout.primary_cta_fg_hex, 0x141414));
    let label_str: SharedString = label.into();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(14.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(cta_bg)
        .text_color(cta_fg)
        .hover(|s| s.opacity(0.84))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.5))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(label_str)
}

/// OpenBot monochrome secondary/outline CTA button with `1px` hairline border.
pub(crate) fn openbot_secondary_btn(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    let label_str: SharedString = label.into();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(ob.bg_control())
        .border_1()
        .border_color(ob.border_strong_hairline())
        .text_color(ob.text_primary())
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .child(label_str)
}

/// OpenBot `38px` `PageHeader` bar with optional clickable parent breadcrumb (`parent › title`)
/// and optional trailing controls slot.
pub(crate) fn openbot_page_header(
    title: impl Into<SharedString>,
    breadcrumb_parent: Option<Stateful<Div>>,
    trailing: Option<AnyElement>,
    pal: TdesktopThemePalette,
) -> Div {
    let ob = pal.openbot_palette();
    let layout = compute_content_hub_shell_layout(&pal, 1280.0, "marketplace");
    let title_str: SharedString = title.into();
    let mut left = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .gap(px(6.0));
    if let Some(parent_el) = breadcrumb_parent {
        left = left.child(parent_el).child(
            div()
                .text_size(px(13.0))
                .text_color(ob.text_muted())
                .child("›"),
        );
    }
    left = left.child(
        div()
            .min_w(px(0.0))
            .truncate()
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ob.text_primary())
            .child(title_str),
    );

    let mut bar = div()
        .h(px(layout.page_header_height_px))
        .flex_shrink_0()
        .w_full()
        .px(px(16.0))
        .bg(rgb(parse_hex_color(&layout.canvas_hex, 0x1A1A1A)))
        .border_b_1()
        .border_color(rgb(parse_hex_color(&layout.hairline_hex, 0x262626)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(left);
    if let Some(right) = trailing {
        bar = bar.child(right);
    }
    bar
}

/// OpenBot `MarketplaceNav` row (`32px` height, `8px` radius, monochrome active & hover states).
pub(crate) fn openbot_marketplace_nav_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    count_opt: Option<usize>,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let label_str: SharedString = label.into();
    let hover_bg = ob.bg_control_hover();
    let active_bg = ob.bg_control_active();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();
    let text_dim = ob.text_dim();

    div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .h(px(32.0))
        .px(px(10.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .when(active, |el| {
            el.bg(active_bg)
                .text_color(text_pri)
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |el| {
            el.text_color(text_muted)
                .hover(move |s| s.bg(hover_bg).text_color(text_pri))
        })
        .text_size(px(13.0))
        .child(div().flex_1().min_w(px(0.0)).truncate().child(label_str))
        .when_some(count_opt, |el, count| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(if active { text_muted } else { text_dim })
                    .child(format!("{count}")),
            )
        })
}

/// OpenBot 2-column `Marketplace` browse shell: `280px` left sidebar nav + right scrollable
/// `880px`-max centered canvas on `ob.bg_canvas()`.
pub(crate) fn openbot_marketplace_shell(
    left_nav: Div,
    body_scroll: Stateful<Div>,
    pal: TdesktopThemePalette,
) -> Div {
    let ob = pal.openbot_palette();
    let layout = compute_content_hub_shell_layout(&pal, 1280.0, "marketplace");
    div()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_hidden()
        .bg(rgb(parse_hex_color(&layout.canvas_hex, 0x1A1A1A)))
        .flex()
        .flex_row()
        .child(
            div()
                .id("openbot-marketplace-left-nav")
                .w(px(layout.left_nav_width_px))
                .flex_shrink_0()
                .h_full()
                .bg(ob.bg_sidebar())
                .border_r_1()
                .border_color(rgb(parse_hex_color(&layout.hairline_hex, 0x262626)))
                .p(px(12.0))
                .overflow_y_scroll()
                .child(left_nav),
        )
        .child(body_scroll.flex_1().min_w(px(0.0)).h_full().overflow_y_scroll())
}

/// Standard OpenBot grouped card surface (`14px` radius, `#212121`/`#ffffff`, crisp 1px border).
pub fn apple_grouped_card(pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    let layout = compute_content_hub_shell_layout(&pal, 1280.0, "marketplace");
    div()
        .w_full()
        .rounded(px(14.0))
        .bg(rgb(parse_hex_color(&layout.card_surface_hex, 0x212121)))
        .border_1()
        .border_color(ob.frame_card_border())
        .overflow_hidden()
}

/// Horizontal hairline divider inset for grouped cards.
pub fn apple_hairline(pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    div().w_full().h(px(1.0)).bg(ob.border_hairline())
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

/// Renders a semantic OpenBot pill badge for a creation's [`CreationReviewStatus`].
pub fn review_status_badge(
    status: CreationReviewStatus,
    lang_id: &str,
    pal: TdesktopThemePalette,
) -> Div {
    let ob = pal.openbot_palette();
    let (fg, bg_col, border_col, label) = match status {
        CreationReviewStatus::Draft => (
            ob.text_muted(),
            ob.bg_control_active(),
            ob.border_hairline(),
            tr_str(lang_id, "草稿", "Draft"),
        ),
        CreationReviewStatus::PendingReview => {
            let w = ob.warning();
            (
                w,
                gpui::Rgba { a: 0.14, ..w },
                gpui::Rgba { a: 0.28, ..w },
                tr_str(lang_id, "审核中", "Pending Review"),
            )
        }
        CreationReviewStatus::Published => {
            let s = ob.success();
            (
                s,
                gpui::Rgba { a: 0.14, ..s },
                gpui::Rgba { a: 0.28, ..s },
                tr_str(lang_id, "已上架", "Published"),
            )
        }
        CreationReviewStatus::Rejected => {
            let d = ob.danger();
            (
                d,
                gpui::Rgba { a: 0.14, ..d },
                gpui::Rgba { a: 0.28, ..d },
                tr_str(lang_id, "已退回", "Rejected"),
            )
        }
        CreationReviewStatus::Unpublished => (
            ob.text_dim(),
            ob.bg_control(),
            ob.border_hairline(),
            tr_str(lang_id, "已下架", "Unpublished"),
        ),
    };
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(999.0))
        .bg(bg_col)
        .border_1()
        .border_color(border_col)
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(label)
}

/// Renders an OpenBot monochrome segmented filter pill button.
pub fn filter_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let label_str: SharedString = label.into();
    let active_bg = ob.surface_light();
    let active_fg = ob.text_on_light();
    let inactive_bg = ob.bg_control();
    let hover_bg = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(12.0))
        .py(px(5.0))
        .rounded(px(999.0))
        .when(active, |el| {
            el.bg(active_bg)
                .text_color(active_fg)
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!active, |el| {
            el.bg(inactive_bg)
                .border_1()
                .border_color(ob.border_hairline())
                .text_color(ob.text_muted())
                .hover(move |s| s.bg(hover_bg).text_color(text_pri))
        })
        .text_size(px(12.0))
        .child(label_str)
}

/// Renders a compact monochrome star rating pill (`★ 4.8 (12)`).
pub fn rating_pill(rating_avg: f32, rating_count: u32, pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    let label = if rating_count > 0 {
        format!("{:.1} ({})", rating_avg, rating_count)
    } else {
        "New".to_string()
    };
    div()
        .px(px(7.0))
        .py(px(2.0))
        .rounded(px(6.0))
        .bg(ob.bg_control_active())
        .border_1()
        .border_color(ob.border_hairline())
        .flex()
        .items_center()
        .gap(px(3.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::MEDIUM)
        .child(div().text_color(ob.warning()).child("★"))
        .child(div().text_color(ob.text_secondary()).child(label))
}

/// Renders a flat OpenBot monochrome editorial hero banner card (zero neon gradients).
pub fn hero_banner(
    id: &'static str,
    kicker: &str,
    title: &str,
    subtitle: &str,
    _start_hex: &str,
    _end_hex: &str,
    icon: IconName,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    div()
        .id(id)
        .w_full()
        .rounded(px(14.0))
        .p(px(18.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.border_strong_hairline())
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
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_muted())
                        .child(kicker.to_string()),
                )
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(ob.text_secondary())
                        .child(subtitle.to_string()),
                ),
        )
        .child(
            div()
                .w(px(44.0))
                .h(px(44.0))
                .flex_shrink_0()
                .rounded(px(12.0))
                .bg(ob.bg_control_active())
                .border_1()
                .border_color(ob.border_hairline())
                .flex()
                .items_center()
                .justify_center()
                .child(tg_icon_rgba(icon, 22.0, ob.text_primary())),
        )
}

/// Renders an OpenBot empty-state card with a monochrome icon tile, description, and CTA button.
pub fn empty_state_card_shell(
    icon: IconName,
    title: &str,
    subtitle: &str,
    cta_btn: Stateful<Div>,
    pal: TdesktopThemePalette,
) -> Div {
    let ob = pal.openbot_palette();
    apple_grouped_card(pal)
        .border_color(ob.border_strong_hairline())
        .px(px(24.0))
        .py(px(32.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .child(
            div()
                .w(px(44.0))
                .h(px(44.0))
                .rounded(px(12.0))
                .bg(ob.bg_control_active())
                .border_1()
                .border_color(ob.border_hairline())
                .flex()
                .items_center()
                .justify_center()
                .child(tg_icon_rgba(icon, 22.0, ob.text_secondary())),
        )
        .child(
            div()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(ob.text_muted())
                .child(subtitle.to_string()),
        )
        .child(div().pt(px(6.0)).child(cta_btn))
}

/// Renders a monochrome OpenBot input field showing committed text, inline IME preedit,
/// focus border, and `Cmd+A` selection highlight.
pub fn render_editable_field_box(
    id: &'static str,
    field: &EditableFieldState,
    placeholder: &str,
    focused: bool,
    multiline: bool,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let border_col = if focused {
        ob.border_strong_hairline()
    } else {
        ob.border_hairline()
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
    let caret_col = ob.text_primary();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();
    let sel_bg = ob.hover();

    div()
        .id(id)
        .cursor_text()
        .w_full()
        .when(multiline, |el| el.min_h(px(64.0)).items_start())
        .when(!multiline, |el| el.min_h(px(34.0)).items_center())
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .bg(ob.bg_control())
        .border_1()
        .border_color(border_col)
        .flex()
        .flex_wrap()
        .gap(px(0.0))
        .text_size(px(13.0))
        .when(is_empty, |el| {
            el.when(focused, |f_el| {
                f_el.child(div().w(px(1.5)).h(px(15.0)).mr(px(1.0)).bg(caret_col))
            })
            .child(div().text_color(text_muted).child(placeholder.to_string()))
        })
        .when(!is_empty && selected_all, |el| {
            el.child(
                div()
                    .bg(sel_bg)
                    .rounded(px(3.0))
                    .text_color(text_pri)
                    .child(display_committed.clone()),
            )
            .when_some(preedit_opt.clone(), |p_el, pre| {
                p_el.child(div().text_color(text_pri).underline().child(pre))
            })
            .when(focused, |f_el| {
                f_el.child(div().w(px(1.5)).h(px(15.0)).bg(caret_col))
            })
        })
        .when(!is_empty && !selected_all && !focused, |el| {
            el.child(div().text_color(text_pri).child(display_committed))
        })
        .when(!is_empty && !selected_all && focused, |el| {
            el.when(!before_owned.is_empty(), |b_el| {
                b_el.child(div().text_color(text_pri).child(before_owned))
            })
            .when_some(preedit_opt, |p_el, pre| {
                p_el.child(div().text_color(text_pri).underline().child(pre))
            })
            .when(focused, |f_el| {
                f_el.child(div().w(px(1.5)).h(px(15.0)).bg(caret_col))
            })
            .when(!after_owned.is_empty(), |a_el| {
                a_el.child(div().text_color(text_pri).child(after_owned))
            })
        })
}

/// Formats seconds into `M:SS` / `MM:SS` for Music durations and timers.
pub fn format_duration_secs(secs: u64) -> String {
    let mins = secs / 60;
    let rem = secs % 60;
    format!("{mins}:{rem:02}")
}

/// Formats milliseconds into `M:SS` for playback progress & game timers.
pub fn format_duration_ms(ms: u64) -> String {
    format_duration_secs(ms / 1000)
}

/// Renders an OpenBot monochrome horizontal progress bar (`0.0..=1.0`).
pub fn apple_progress_bar(fraction: f32, _accent_rgb: u32, pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    let clamped = fraction.clamp(0.0, 1.0);
    div()
        .w_full()
        .h(px(4.0))
        .rounded(px(999.0))
        .bg(ob.bg_control_active())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(gpui::relative(clamped))
                .rounded(px(999.0))
                .bg(ob.surface_light()),
        )
}

/// Renders a subtle section shelf header with title and optional trailing count/badge.
pub fn apple_shelf_header(
    title: impl Into<SharedString>,
    subtitle: Option<String>,
    pal: TdesktopThemePalette,
) -> Div {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .flex()
        .items_baseline()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(title.into()),
        )
        .when_some(subtitle, |el, sub| {
            el.child(div().text_size(px(12.0)).text_color(ob.text_muted()).child(sub))
        })
}

/// Renders an OpenBot monochrome segmented sub-tab button for vertical navigation bars.
pub fn vertical_sub_tab_pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    _accent_rgb: u32,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let label_str: SharedString = label.into();
    let hover_bg = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .when(active, |el| {
            el.bg(ob.bg_control_active())
                .border_1()
                .border_color(ob.border_strong_hairline())
                .text_color(text_pri)
                .font_weight(FontWeight::SEMIBOLD)
        })
        .when(!active, |el| {
            el.text_color(ob.text_muted())
                .hover(move |s| s.bg(hover_bg).text_color(text_pri))
        })
        .text_size(px(12.5))
        .whitespace_nowrap()
        .child(label_str)
}

