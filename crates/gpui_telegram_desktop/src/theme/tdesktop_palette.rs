//! Full semantic `TdesktopThemePalette` struct and helpers.

use std::collections::HashMap;
use gpui::{
    hsla, linear_color_stop, linear_gradient, point, px, rgb, Background, BoxShadow, Rgba,
};

use super::openbot_palette::{OpenBotColorScheme, OpenBotPalette};
use super::palette::PaletteTokens;
use super::palette_presets::{apply_openbot_chrome, preset_openbot_palette, preset_palette};
use super::palette_schemes::{
    find_wallpaper_preset, parse_tdesktop_theme_colors, peer_color_index, rgba_from_hex_alpha,
    TdesktopThemeMode,
};

/// Complete semantic color, typography, bubble geometry, and peer palette derived
/// from Apple Messages macOS & Telegram Desktop `.tdesktop-theme` tokens across
/// `Night`, `Tinted`, `Day`, and `Classic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TdesktopThemePalette {
    pub mode: TdesktopThemeMode,
    pub window_bg: u32,
    pub hover_bg: u32,
    pub nav_rail_bg: u32,
    pub rail_bg: u32,
    pub dialogs_bg: u32,
    pub dialogs_hover_bg: u32,
    pub dialogs_active_bg: u32,
    pub history_bg: u32,
    pub stage_bg: u32,
    pub top_bar_bg: u32,
    pub topbar_bg: u32,
    pub compose_bg: u32,
    pub composer_bg: u32,
    pub input_bg: u32,
    pub card_bg: u32,
    pub modal_bg: u32,
    pub tooltip_bg: u32,
    pub tooltip_fg: u32,
    pub menu_bg: u32,
    pub menu_hover_bg: u32,
    pub scrollbar_bg: u32,
    pub scrollbar_hover_bg: u32,
    pub msg_in_bg: u32,
    pub msg_out_bg: u32,
    pub msg_out_bg_top: u32,
    pub msg_out_bg_bottom: u32,
    pub msg_out_fg: u32,
    pub msg_out_secondary_fg: u32,
    pub msg_out_link_fg: u32,
    pub msg_in_selected_bg: u32,
    pub msg_out_selected_bg: u32,
    pub msg_in_date_fg: u32,
    pub msg_out_date_fg: u32,
    pub reply_bar_fg: u32,
    pub reply_bar_bg: u32,
    pub code_bg: u32,
    pub code_fg: u32,
    pub service_pill_bg: u32,
    pub service_pill_fg: u32,
    pub unread_badge_bg: u32,
    pub unread_badge_fg: u32,
    pub muted_badge_bg: u32,
    pub online_dot_fg: u32,
    pub check_read_fg: u32,
    pub check_sent_fg: u32,
    pub icon_fg: u32,
    pub icon_active: u32,
    pub border_fg: u32,
    pub shadow_fg: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_link: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub active_button_fg: u32,
    pub reaction_bg: u32,
    pub reaction_active_bg: u32,
    pub danger: u32,
    pub success: u32,
    pub wallpaper_top: u32,
    pub wallpaper_bottom: u32,
    pub wallpaper_pattern: u32,
    pub peer_name_colors: [u32; 7],
    pub peer_avatar_colors: [u32; 7],
    pub peer_avatar_colors_bottom: [u32; 7],
    pub font_size_px: u32,
    pub bubble_radius_px: u32,
    pub ui_scale_pct: u32,
    pub blur_wallpaper: bool,
}

impl TdesktopThemePalette {
    /// Constructs a full semantic palette for the requested theme mode, custom accent color,
    /// message font scale (`12..=20`), and bubble corner radius (`6..=18`).
    pub fn build(
        mode: TdesktopThemeMode,
        accent: u32,
        font_size_px: u32,
        bubble_radius_px: u32,
    ) -> Self {
        preset_palette(mode, accent, font_size_px, bubble_radius_px)
    }

    /// Constructs a semantic palette synced directly to OpenBot `383a35d2` monochrome dark/light chrome.
    pub fn build_openbot(
        mode: TdesktopThemeMode,
        accent: u32,
        font_size_px: u32,
        bubble_radius_px: u32,
    ) -> Self {
        preset_openbot_palette(mode, accent, font_size_px, bubble_radius_px)
    }

    /// Overlays OpenBot `383a35d2` monochrome chrome onto `self` (idempotent; preserves `mode`,
    /// `Classic` green bubbles, typography, scale, bubble radius, and wallpaper blur).
    pub fn with_openbot_chrome(self) -> Self {
        apply_openbot_chrome(self)
    }

    /// Maps `self.mode` (`Night | Tinted => Dark`, `Day | Classic => Light`) to [`OpenBotColorScheme`].
    pub const fn openbot_scheme(&self) -> OpenBotColorScheme {
        if self.mode.is_dark() {
            OpenBotColorScheme::Dark
        } else {
            OpenBotColorScheme::Light
        }
    }

    /// Returns the resolved OpenBot `383a35d2` [`OpenBotPalette`] matching `self.mode`.
    pub fn openbot_palette(&self) -> OpenBotPalette {
        OpenBotPalette::for_scheme(self.openbot_scheme())
    }

    /// Applies live interface zoom percentage (`100..=300`) to scale fixed-pixel UI tokens.
    pub fn with_ui_scale(mut self, scale_pct: u32) -> Self {
        self.ui_scale_pct = scale_pct.clamp(50, 300);
        self
    }

    /// Applies wallpaper frosted-blur state to stage rendering.
    pub fn with_blur_wallpaper(mut self, blur: bool) -> Self {
        self.blur_wallpaper = blur;
        self
    }

    /// Multiplier (`1.0` at `100%`, `1.25` at `125%`, `1.5` at `150%`, `2.0` at `200%`).
    pub fn scale_factor(&self) -> f32 {
        let pct = if self.ui_scale_pct == 0 { 100 } else { self.ui_scale_pct.clamp(50, 300) };
        (pct as f32) / 100.0
    }

    /// Scales a base pixel metric by the active interface scale factor.
    pub fn scaled_px(&self, base_px: f32) -> f32 {
        base_px * self.scale_factor()
    }

    /// Applies a named built-in wallpaper preset (`default_night`, `aurora_dusk`, etc.).
    pub fn with_wallpaper(mut self, preset_id: &str) -> Self {
        if let Some(preset) = find_wallpaper_preset(preset_id) {
            self.wallpaper_top = preset.top_hex;
            self.wallpaper_bottom = preset.bottom_hex;
            self.wallpaper_pattern = preset.pattern_hex;
        }
        self
    }

    /// Deterministic Telegram 7-slot sender name color for `peer_id`.
    pub const fn peer_name_color(&self, peer_id: i64) -> u32 {
        self.peer_name_colors[peer_color_index(peer_id)]
    }

    /// Deterministic Telegram 7-slot avatar base color for `peer_id`.
    pub const fn peer_avatar_color(&self, peer_id: i64) -> u32 {
        self.peer_avatar_colors[peer_color_index(peer_id)]
    }

    /// Deterministic Telegram 7-slot `(top, bottom)` avatar gradient colors for `peer_id`.
    pub const fn peer_avatar_gradient(&self, peer_id: i64) -> (u32, u32) {
        let idx = peer_color_index(peer_id);
        (self.peer_avatar_colors[idx], self.peer_avatar_colors_bottom[idx])
    }

    /// Converts an `0xRRGGBB` hex integer plus float opacity (`0.0..=1.0`) to GPUI `Rgba`.
    pub fn rgba_with_alpha(hex: u32, alpha: f32) -> Rgba {
        rgba_from_hex_alpha(hex, alpha)
    }

    /// Returns `true` for Apple Messages macOS visual styles (`Night`, `Tinted`, `Day`).
    pub const fn is_apple_style(&self) -> bool {
        self.mode.is_fabushi_style()
    }

    /// Unified solid sidebar fill so sidebar, chat canvas, and right inspector drawer share one
    /// consistent surface luminance separated by crisp macOS hairlines.
    pub fn sidebar_bg_rgba(&self) -> Rgba {
        Self::rgba_with_alpha(self.dialogs_bg, 1.0)
    }

    /// Hairline translucent separator matching Grok Bot `sand-*` tokens
    /// (`rgba(28,28,28,0.08)` light / `rgba(255,255,255,0.10)` dark).
    pub fn separator_rgba(&self) -> Rgba {
        if self.mode.is_dark() {
            Self::rgba_with_alpha(0xFFFFFF, 0.10)
        } else {
            Self::rgba_with_alpha(0x1C1C1C, 0.08)
        }
    }

    /// Translucent card / popover border (`--sand-border-secondary-translucent`).
    pub fn hairline_border_rgba(&self) -> Rgba {
        if self.mode.is_dark() {
            Self::rgba_with_alpha(0xFFFFFF, 0.10)
        } else {
            Self::rgba_with_alpha(0x1C1C1C, 0.12)
        }
    }

    /// Top specular highlight for dark-mode elevation separation without muddy shadows
    /// (`--sand-specular-primary`: 12% white in dark mode; crisp white bevel in light mode).
    pub fn specular_top_highlight_rgba(&self) -> Rgba {
        if self.mode.is_dark() {
            Self::rgba_with_alpha(0xFFFFFF, 0.12)
        } else {
            Self::rgba_with_alpha(0xFFFFFF, 0.72)
        }
    }

    /// Subtle translucent row hover fill matching Grok Bot `--sand-bg-hover` (6% ink).
    pub fn row_hover_rgba(&self) -> Rgba {
        if self.mode.is_dark() {
            Self::rgba_with_alpha(0xF5F5F5, 0.06)
        } else {
            Self::rgba_with_alpha(0x1C1C1C, 0.06)
        }
    }

    /// Subtle translucent row active/selected fill matching Grok Bot `--sand-bg-active` (8% ink).
    pub fn row_active_rgba(&self) -> Rgba {
        if self.mode.is_dark() {
            Self::rgba_with_alpha(0xF5F5F5, 0.08)
        } else {
            Self::rgba_with_alpha(0x1C1C1C, 0.08)
        }
    }

    /// Grok Bot 9-hue per-bot color palette lookup returning `(bubble_fill, pill_bg, pill_text)`
    /// across `blue`, `red`, `green`, `yellow`, `orange`, `cyan`, `purple`, `magenta`, `brown`.
    /// Used for 1:1 bot chat user bubbles and clickable hue-tinted group sender pills.
    pub fn bot_hue_colors(hue_name: &str, is_dark: bool) -> (u32, u32, u32) {
        let normalized = hue_name.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "red" | "danger" | "rose" | "crimson" => {
                if is_dark { (0xCF2B35, 0x36191C, 0xFF6463) } else { (0xC82532, 0xFFEAEC, 0xCD2834) }
            }
            "green" | "success" | "emerald" | "mint" => {
                if is_dark { (0x0E9F5A, 0x162E22, 0x00BA69) } else { (0x0D9655, 0xE7F9F0, 0x00854A) }
            }
            "yellow" | "warn" | "amber" | "gold" => {
                if is_dark { (0xC8770C, 0x352815, 0xE28600) } else { (0xC2730B, 0xFEF4E7, 0xAA6300) }
            }
            "orange" | "coral" => {
                if is_dark { (0xCD5309, 0x362115, 0xFE6600) } else { (0xD2550A, 0xFFF0E7, 0xC14C00) }
            }
            "cyan" | "teal" | "aqua" => {
                if is_dark { (0x0E9886, 0x162D2A, 0x00B7A2) } else { (0x0D9381, 0xE7F8F6, 0x008273) }
            }
            "purple" | "violet" | "lavender" | "indigo" => {
                if is_dark { (0x7845D7, 0x271F39, 0xA582FF) } else { (0x7442D1, 0xF4EEFE, 0x7547D0) }
            }
            "magenta" | "pink" | "fuchsia" => {
                if is_dark { (0xCF297E, 0x361A28, 0xFF55A4) } else { (0xC82779, 0xFEEBF5, 0xC62378) }
            }
            "brown" | "bronze" | "earth" => {
                if is_dark { (0x8C5D31, 0x2A221B, 0xC59E7D) } else { (0x86592E, 0xF5F0EB, 0x996B42) }
            }
            // Default / "blue" / "accent" / neutral fallback
            _ => {
                if is_dark { (0x116DD2, 0x162639, 0x4D9BFF) } else { (0x126FD4, 0xE7F2FF, 0x0B6DD3) }
            }
        }
    }

    /// Deterministic 9-hue selector for a peer or bot numeric ID.
    pub const fn bot_hue_name_for_id(id: u64) -> &'static str {
        const HUES: [&str; 9] = [
            "blue", "purple", "cyan", "green", "orange", "magenta", "red", "yellow", "brown",
        ];
        HUES[(id as usize) % HUES.len()]
    }

    /// Returns whether `hue_name` represents an explicit chromatic bot tint (excluding neutral black/gray).
    pub fn is_tinted_bot_hue(hue_name: &str) -> bool {
        let lower = hue_name.trim().to_ascii_lowercase();
        !lower.is_empty() && !matches!(lower.as_str(), "black" | "gray" | "grey" | "neutral" | "default")
    }

    /// Grouped rounded card surface fill for Inspector drawers and macOS modal cards.
    pub fn card_surface_rgba(&self) -> Rgba {
        let alpha = if self.is_apple_style() { 0.96 } else { 1.0 };
        Self::rgba_with_alpha(self.card_bg, alpha)
    }

    /// Outgoing message bubble fill: vertical 180deg Apple Messages blue sheen in Apple style,
    /// solid legacy green fill in `Classic`.
    pub fn msg_out_gradient(&self) -> Background {
        if self.is_apple_style() {
            linear_gradient(
                180.0,
                linear_color_stop(rgb(self.msg_out_bg_top), 0.0),
                linear_color_stop(rgb(self.msg_out_bg_bottom), 1.0),
            )
        } else {
            rgb(self.msg_out_bg).into()
        }
    }

    /// Elevation shadows for popovers, menus, and modals.
    /// In Grok Bot dark mode (`sand-dark`), dark shadows are eliminated (`Vec::new()`) in favor of
    /// 4-step luminance separation, translucent hairline borders, and top specular highlights.
    pub fn popover_shadows(&self) -> Vec<BoxShadow> {
        if self.mode.is_dark() {
            if self.is_apple_style() {
                Vec::new()
            } else {
                vec![
                    BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.36),
                        offset: point(px(0.0), px(8.0)),
                        blur_radius: px(22.0),
                        spread_radius: px(0.0),
                    },
                ]
            }
        } else {
            vec![
                BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.10),
                    offset: point(px(0.0), px(8.0)),
                    blur_radius: px(24.0),
                    spread_radius: px(0.0),
                },
                BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.08),
                    offset: point(px(0.0), px(2.0)),
                    blur_radius: px(6.0),
                    spread_radius: px(0.0),
                },
            ]
        }
    }

    /// Translucent service pill / date badge background matching Fabushi / Telegram Desktop stage overlays.
    pub fn service_pill_rgba(&self) -> Rgba {
        let alpha = if self.mode.is_fabushi_style() {
            0.92
        } else if self.mode.is_dark() {
            0.80
        } else {
            0.52
        };
        Self::rgba_with_alpha(self.service_pill_bg, alpha)
    }

    /// Translucent idle scrollbar thumb color.
    pub fn scrollbar_rgba(&self) -> Rgba {
        let alpha = if self.mode.is_dark() { 0.28 } else { 0.34 };
        Self::rgba_with_alpha(self.scrollbar_bg, alpha)
    }

    /// Translucent hovered scrollbar thumb color.
    pub fn scrollbar_hover_rgba(&self) -> Rgba {
        let alpha = if self.mode.is_dark() { 0.48 } else { 0.55 };
        Self::rgba_with_alpha(self.scrollbar_hover_bg, alpha)
    }

    /// Subtle drop-shadow color for floating menus, popovers, and modal boxes.
    pub fn shadow_rgba(&self) -> Rgba {
        let alpha = if self.mode.is_dark() { 0.42 } else { 0.16 };
        Self::rgba_with_alpha(self.shadow_fg, alpha)
    }

    /// Converts this semantic palette into legacy [`PaletteTokens`] for existing widgets/tests.
    pub fn to_palette_tokens(&self) -> PaletteTokens {
        PaletteTokens {
            window_bg: rgb(self.window_bg),
            window_fg: rgb(self.text_primary),
            active: rgb(self.accent),
            bubble_in_bg: rgb(self.msg_in_bg),
            bubble_in_fg: rgb(self.text_primary),
            bubble_out_bg: rgb(self.msg_out_bg),
            bubble_out_fg: rgb(self.msg_out_fg),
            sidebar_bg: rgb(self.dialogs_bg),
            sidebar_active: rgb(self.dialogs_active_bg),
            sidebar_hover: rgb(self.dialogs_hover_bg),
            unread_badge_bg: rgb(self.unread_badge_bg),
            unread_badge_fg: rgb(self.unread_badge_fg),
            unread_badge_muted: rgb(self.muted_badge_bg),
        }
    }

    /// Parses `.tdesktop-theme` color definitions (`key: #rrggbb;` / aliases) into `0xRRGGBB`.
    pub fn parse_tdesktop_theme_colors(content: &str) -> HashMap<String, u32> {
        parse_tdesktop_theme_colors(content)
    }
}

impl Default for TdesktopThemePalette {
    fn default() -> Self {
        Self::build(TdesktopThemeMode::Night, 0x0A84FF, 14, 18)
    }
}
