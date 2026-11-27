//! OpenBot semantic design tokens, custom palette derivation (`paletteTokens`),
//! CSS Color 4 premultiplied sRGB mixing, OKLab chat-background readability clamping,
//! file-reference tones, bubble contrast math, and brand/scale constants (`OB-F05`).
//!
//! 1:1 port of upstream OpenBot (`23183891`):
//! - `src/renderer/src/theme-palette.ts`
//! - `packages/brand/src/{tokens.css, tokens-light.css, chat-colors.ts, file-reference.ts}`
//! - `src/renderer/src/styles/window-glass.css`
//! - `packages/contracts/src/{app-appearance.ts, app-interface-size.ts}`
//! - `packages/ui/src/features/conversation/own-bubble.ts`

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use gpui::Rgba;
use serde::{Deserialize, Serialize};

/// OpenBot base color scheme (`"dark"` or `"light"`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum OpenBotColorScheme {
    #[default]
    Dark,
    Light,
}

impl OpenBotColorScheme {
    /// Returns the canonical lowercase identifier (`"dark"` or `"light"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Parses `"dark"` or `"light"` strictly.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    /// Returns `true` when the color scheme is `Dark`.
    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }

    /// Returns `true` when the color scheme is `Light`.
    pub const fn is_light(self) -> bool {
        matches!(self, Self::Light)
    }
}

impl fmt::Display for OpenBotColorScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The 12 required semantic color slots in a custom OpenBot JSON theme file
/// (`APP_THEME_PALETTE_COLORS` in `@openbot/contracts/app-appearance`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotThemeColors {
    pub canvas: String,
    pub frame: String,
    pub sidebar: String,
    pub surface: String,
    pub text: String,
    pub text_muted: String,
    pub border: String,
    pub accent: String,
    pub accent_foreground: String,
    pub success: String,
    pub warning: String,
    pub danger: String,
}

impl OpenBotThemeColors {
    /// Exact 12 JSON property keys required in `"colors"`.
    pub const ALL_JSON_KEYS: [&'static str; 12] = [
        "canvas",
        "frame",
        "sidebar",
        "surface",
        "text",
        "textMuted",
        "border",
        "accent",
        "accentForeground",
        "success",
        "warning",
        "danger",
    ];

    /// Iterates `(json_key, value)` pairs across all 12 slots in canonical order.
    pub fn iter_entries(&self) -> [(&'static str, &str); 12] {
        [
            ("canvas", self.canvas.as_str()),
            ("frame", self.frame.as_str()),
            ("sidebar", self.sidebar.as_str()),
            ("surface", self.surface.as_str()),
            ("text", self.text.as_str()),
            ("textMuted", self.text_muted.as_str()),
            ("border", self.border.as_str()),
            ("accent", self.accent.as_str()),
            ("accentForeground", self.accent_foreground.as_str()),
            ("success", self.success.as_str()),
            ("warning", self.warning.as_str()),
            ("danger", self.danger.as_str()),
        ]
    }
}

/// Validated custom theme palette descriptor (`AppThemePalette`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppThemePalette {
    pub id: String,
    pub name: String,
    pub scheme: OpenBotColorScheme,
    pub colors: OpenBotThemeColors,
}

/// Structured validation and I/O errors for OpenBot theme palettes, colors, and backdrops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemePaletteError {
    Io(String),
    OversizedFile(String),
    InvalidPaletteId(String),
    InvalidJson(String),
    InvalidName(String),
    InvalidAppearance(String),
    InvalidColor(String),
    CssInjectionAttempt(String),
    UnknownBackdropScene(String),
}

impl ThemePaletteError {
    /// Canonical error code (`"io_error"` vs `"validation_error"`).
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io_error",
            Self::OversizedFile(_)
            | Self::InvalidPaletteId(_)
            | Self::InvalidJson(_)
            | Self::InvalidName(_)
            | Self::InvalidAppearance(_)
            | Self::InvalidColor(_)
            | Self::CssInjectionAttempt(_)
            | Self::UnknownBackdropScene(_) => "validation_error",
        }
    }

    /// Returns `true` when this error represents a schema, security, or domain validation failure.
    pub const fn is_validation_error(&self) -> bool {
        !matches!(self, Self::Io(_))
    }
}

impl fmt::Display for ThemePaletteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "theme I/O error: {msg}"),
            Self::OversizedFile(msg) => write!(f, "oversized theme palette file: {msg}"),
            Self::InvalidPaletteId(msg) => write!(f, "invalid theme palette id: {msg}"),
            Self::InvalidJson(msg) => write!(f, "invalid theme palette JSON: {msg}"),
            Self::InvalidName(msg) => write!(f, "invalid theme palette name: {msg}"),
            Self::InvalidAppearance(msg) => write!(f, "invalid theme appearance: {msg}"),
            Self::InvalidColor(msg) => write!(f, "invalid theme color: {msg}"),
            Self::CssInjectionAttempt(msg) => write!(f, "rejected CSS injection attempt: {msg}"),
            Self::UnknownBackdropScene(msg) => write!(f, "unknown chat backdrop scene: {msg}"),
        }
    }
}

impl std::error::Error for ThemePaletteError {}

impl From<std::io::Error> for ThemePaletteError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for ThemePaletteError {
    fn from(err: serde_json::Error) -> Self {
        Self::InvalidJson(err.to_string())
    }
}

/// Complete resolved OpenBot semantic palette with all 58 `--openbot-*` tokens
/// both as canonical CSS strings and GPUI `Rgba` values.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotPalette {
    pub palette_id: String,
    pub scheme: OpenBotColorScheme,
    pub custom_source: Option<AppThemePalette>,
    pub tokens: BTreeMap<String, String>,
    pub rgba_tokens: BTreeMap<String, Rgba>,
}

/// Exact 58 `--openbot-*` dark defaults from `packages/brand/src/tokens.css`.
pub const DARK_DEFAULT_TOKENS: [(&str, &str); 58] = [
    ("--openbot-bg-canvas", "#1a1a1a"),
    ("--openbot-bg-canvas-clear", "rgba(26, 26, 26, 0)"),
    ("--openbot-bg-frame", "#0d0d0d"),
    ("--openbot-bg-sidebar", "#1a1a1a"),
    ("--openbot-bg-native-sidebar", "#141414"),
    ("--openbot-bg-sheet", "#141414"),
    ("--openbot-bg-sheet-inset", "#101010"),
    ("--openbot-bg-grouped", "#212121"),
    ("--openbot-bg-surface", "#212121"),
    ("--openbot-bg-surface-raised", "#242424"),
    ("--openbot-bg-control", "#212121"),
    ("--openbot-bg-control-hover", "#242424"),
    ("--openbot-bg-control-active", "#2a2a2a"),
    ("--openbot-tabs-bar-bg", "#212121"),
    ("--openbot-tabs-pill-bg", "#2a2a2a"),
    ("--openbot-marketplace-detail-bg", "#111111"),
    ("--openbot-bg-overlay", "rgba(26, 26, 26, 0.92)"),
    ("--openbot-bg-glass", "rgba(33, 33, 33, 0.76)"),
    ("--openbot-scroll-edge-overlay", "rgba(0, 0, 0, 0.72)"),
    ("--openbot-border", "rgba(255, 255, 255, 0.035)"),
    ("--openbot-border-strong", "rgba(255, 255, 255, 0.045)"),
    ("--openbot-border-grouped", "#333335"),
    ("--openbot-dialog-ring", "rgba(255, 255, 255, 0.1)"),
    ("--openbot-image-outline", "rgba(255, 255, 255, 0.1)"),
    ("--openbot-hover", "rgba(255, 255, 255, 0.06)"),
    ("--openbot-text-primary", "#ffffff"),
    ("--openbot-text-secondary", "#dcdcdc"),
    ("--openbot-text-muted", "#979797"),
    ("--openbot-text-dim", "#6a6a6a"),
    ("--openbot-text-placeholder", "#bfbfbf"),
    ("--openbot-text-readable", "rgba(255, 255, 255, 0.52)"),
    ("--openbot-text-grouped-secondary", "#96969b"),
    ("--openbot-settings-text-muted", "rgba(255, 255, 255, 0.59)"),
    ("--openbot-surface-light", "#f0f0f0"),
    ("--openbot-text-on-light", "#141414"),
    ("--openbot-text-muted-on-light", "rgba(20, 20, 20, 0.52)"),
    ("--openbot-text-dim-on-light", "rgba(20, 20, 20, 0.36)"),
    ("--openbot-border-on-light", "rgba(20, 20, 20, 0.14)"),
    ("--openbot-accent", "#007cf7"),
    ("--openbot-accent-hover", "#1f8fff"),
    ("--openbot-accent-text", "#42a0ff"),
    ("--openbot-accent-soft", "rgba(0, 124, 247, 0.16)"),
    ("--openbot-accent-strong", "rgba(0, 124, 247, 0.34)"),
    ("--openbot-text-on-accent", "#ffffff"),
    ("--openbot-success", "#31cf76"),
    ("--openbot-success-text", "#31cf76"),
    ("--openbot-success-soft", "rgba(49, 207, 118, 0.12)"),
    ("--openbot-success-wash", "rgba(49, 207, 118, 0.045)"),
    ("--openbot-warning", "#ff9412"),
    ("--openbot-warning-text", "#ff9412"),
    ("--openbot-warning-soft", "rgba(255, 148, 18, 0.12)"),
    ("--openbot-danger", "#fd2f3b"),
    ("--openbot-danger-hover", "#fd5d67"),
    ("--openbot-danger-text", "#ff6069"),
    ("--openbot-danger-soft", "rgba(253, 47, 59, 0.16)"),
    ("--openbot-danger-soft-hover", "rgba(253, 47, 59, 0.28)"),
    ("--openbot-danger-focus", "rgba(253, 47, 59, 0.4)"),
    ("--openbot-danger-border", "rgba(253, 47, 59, 0.5)"),
];

/// Exact 58 `--openbot-*` light defaults from `packages/brand/src/tokens-light.css` & `tokens.css`.
pub const LIGHT_DEFAULT_TOKENS: [(&str, &str); 58] = [
    ("--openbot-bg-canvas", "#f5f5f6"),
    ("--openbot-bg-canvas-clear", "rgba(245, 245, 246, 0)"),
    ("--openbot-bg-frame", "#e8e8eb"),
    ("--openbot-bg-sidebar", "#f5f5f6"),
    ("--openbot-bg-native-sidebar", "#f5f5f6"),
    ("--openbot-bg-sheet", "#ffffff"),
    ("--openbot-bg-sheet-inset", "#f0f0f2"),
    ("--openbot-bg-grouped", "#ffffff"),
    ("--openbot-bg-surface", "#ffffff"),
    ("--openbot-bg-surface-raised", "#fcfcfc"),
    ("--openbot-bg-control", "#ffffff"),
    ("--openbot-bg-control-hover", "#f0f0f2"),
    ("--openbot-bg-control-active", "#e8e8eb"),
    ("--openbot-tabs-bar-bg", "#ececee"),
    ("--openbot-tabs-pill-bg", "#ffffff"),
    ("--openbot-marketplace-detail-bg", "#ececee"),
    ("--openbot-bg-overlay", "rgba(245, 245, 246, 0.92)"),
    ("--openbot-bg-glass", "rgba(255, 255, 255, 0.76)"),
    ("--openbot-scroll-edge-overlay", "rgba(245, 245, 246, 0.72)"),
    ("--openbot-border", "rgba(20, 20, 20, 0.07)"),
    ("--openbot-border-strong", "rgba(20, 20, 20, 0.1)"),
    ("--openbot-border-grouped", "#e3e3e6"),
    ("--openbot-dialog-ring", "rgba(20, 20, 20, 0.1)"),
    ("--openbot-image-outline", "rgba(20, 20, 20, 0.1)"),
    ("--openbot-hover", "rgba(20, 20, 20, 0.05)"),
    ("--openbot-text-primary", "#141414"),
    ("--openbot-text-secondary", "#3a3a3c"),
    ("--openbot-text-muted", "#6b6b70"),
    ("--openbot-text-dim", "#8e8e93"),
    ("--openbot-text-placeholder", "#5c5c61"),
    ("--openbot-text-readable", "rgba(20, 20, 20, 0.6)"),
    ("--openbot-text-grouped-secondary", "#6b6b70"),
    ("--openbot-settings-text-muted", "rgba(20, 20, 20, 0.6)"),
    ("--openbot-surface-light", "#1c1c1e"),
    ("--openbot-text-on-light", "#ffffff"),
    ("--openbot-text-muted-on-light", "rgba(255, 255, 255, 0.64)"),
    ("--openbot-text-dim-on-light", "rgba(255, 255, 255, 0.4)"),
    ("--openbot-border-on-light", "rgba(255, 255, 255, 0.16)"),
    ("--openbot-accent", "#007cf7"),
    ("--openbot-accent-hover", "#006ad4"),
    ("--openbot-accent-text", "#0064c8"),
    ("--openbot-accent-soft", "rgba(0, 124, 247, 0.12)"),
    ("--openbot-accent-strong", "rgba(0, 124, 247, 0.26)"),
    ("--openbot-text-on-accent", "#ffffff"),
    ("--openbot-success", "#1fa85a"),
    ("--openbot-success-text", "#187a41"),
    ("--openbot-success-soft", "rgba(31, 168, 90, 0.12)"),
    ("--openbot-success-wash", "rgba(31, 168, 90, 0.06)"),
    ("--openbot-warning", "#f08a00"),
    ("--openbot-warning-text", "#9c5200"),
    ("--openbot-warning-soft", "rgba(240, 138, 0, 0.14)"),
    ("--openbot-danger", "#e8222d"),
    ("--openbot-danger-hover", "#f04a53"),
    ("--openbot-danger-text", "#c0141d"),
    ("--openbot-danger-soft", "rgba(232, 34, 45, 0.12)"),
    ("--openbot-danger-soft-hover", "rgba(232, 34, 45, 0.2)"),
    ("--openbot-danger-focus", "rgba(232, 34, 45, 0.36)"),
    ("--openbot-danger-border", "rgba(232, 34, 45, 0.45)"),
];

fn format_css_alpha(alpha: f64) -> String {
    let clamped = alpha.clamp(0.0, 1.0);
    if clamped <= 1e-9 {
        return "0".to_string();
    }
    if (clamped - 1.0).abs() <= 1e-9 {
        return "1".to_string();
    }
    let formatted = format!("{clamped:.6}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn parse_hex_rgb_or_rgba(raw: &str) -> Result<(u8, u8, u8, f64), ThemePaletteError> {
    if super::watcher::is_css_injection_like(raw) {
        return Err(ThemePaletteError::CssInjectionAttempt(format!(
            "rejected potential CSS injection value: {raw}"
        )));
    }
    let bytes = raw.as_bytes();
    if (bytes.len() != 7 && bytes.len() != 9) || bytes[0] != b'#' {
        return Err(ThemePaletteError::InvalidColor(format!(
            "expected #RRGGBB or #RRGGBBAA hex color, got: {raw}"
        )));
    }
    if !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err(ThemePaletteError::InvalidColor(format!(
            "non-hex digit in color: {raw}"
        )));
    }
    let r = u8::from_str_radix(&raw[1..3], 16)
        .map_err(|e| ThemePaletteError::InvalidColor(e.to_string()))?;
    let g = u8::from_str_radix(&raw[3..5], 16)
        .map_err(|e| ThemePaletteError::InvalidColor(e.to_string()))?;
    let b = u8::from_str_radix(&raw[5..7], 16)
        .map_err(|e| ThemePaletteError::InvalidColor(e.to_string()))?;
    let a = if bytes.len() == 9 {
        let alpha_u8 = u8::from_str_radix(&raw[7..9], 16)
            .map_err(|e| ThemePaletteError::InvalidColor(e.to_string()))?;
        f64::from(alpha_u8) / 255.0
    } else {
        1.0
    };
    Ok((r, g, b, a))
}

fn parse_srgb_components(raw: &str) -> Result<(f64, f64, f64, f64), ThemePaletteError> {
    if raw == "transparent" {
        return Ok((0.0, 0.0, 0.0, 0.0));
    }
    if let Some(inner) = raw.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        if inner
            .bytes()
            .any(|b| matches!(b, b';' | b'{' | b'}' | b'(' | b')' | b'<' | b'>' | b'\\'))
        {
            return Err(ThemePaletteError::CssInjectionAttempt(format!(
                "rejected malformed rgba() color: {raw}"
            )));
        }
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 4 {
            let r = parts[0]
                .parse::<u8>()
                .map_err(|_| ThemePaletteError::InvalidColor(raw.to_string()))?;
            let g = parts[1]
                .parse::<u8>()
                .map_err(|_| ThemePaletteError::InvalidColor(raw.to_string()))?;
            let b = parts[2]
                .parse::<u8>()
                .map_err(|_| ThemePaletteError::InvalidColor(raw.to_string()))?;
            let a = parts[3]
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
                .ok_or_else(|| ThemePaletteError::InvalidColor(raw.to_string()))?;
            return Ok((f64::from(r), f64::from(g), f64::from(b), a));
        }
        return Err(ThemePaletteError::InvalidColor(raw.to_string()));
    }
    let (r, g, b, a) = parse_hex_rgb_or_rgba(raw)?;
    Ok((f64::from(r), f64::from(g), f64::from(b), a))
}

/// Parses a `#RRGGBB`, `#RRGGBBAA`, `rgba(r, g, b, a)`, or `transparent` CSS color into `gpui::Rgba`.
pub fn parse_css_color_to_rgba(value: &str) -> Option<Rgba> {
    let (r, g, b, a) = parse_srgb_components(value.trim()).ok()?;
    Some(Rgba {
        r: (r as f32 / 255.0).clamp(0.0, 1.0),
        g: (g as f32 / 255.0).clamp(0.0, 1.0),
        b: (b as f32 / 255.0).clamp(0.0, 1.0),
        a: (a as f32).clamp(0.0, 1.0),
    })
}

/// CSS Color 4 premultiplied `color-mix(in srgb, <color> <percent>%, transparent)` (`theme-palette.ts:L3-L5`).
///
/// Preserves `(r, g, b)` of `color` unchanged and scales alpha by `percent / 100.0`.
pub fn srgb_clear(color: &str, percent: f64) -> Result<String, ThemePaletteError> {
    if !percent.is_finite() {
        return Err(ThemePaletteError::InvalidColor(format!(
            "non-finite clear percentage: {percent}"
        )));
    }
    let (r, g, b, a) = parse_srgb_components(color)?;
    let pct = (percent / 100.0).clamp(0.0, 1.0);
    let a_out = (a * pct).clamp(0.0, 1.0);
    let r_u8 = (r + 0.5).floor().clamp(0.0, 255.0) as u8;
    let g_u8 = (g + 0.5).floor().clamp(0.0, 255.0) as u8;
    let b_u8 = (b + 0.5).floor().clamp(0.0, 255.0) as u8;
    if (a_out - 1.0).abs() <= 1e-9 {
        Ok(format!("#{r_u8:02x}{g_u8:02x}{b_u8:02x}"))
    } else {
        Ok(format!(
            "rgba({r_u8}, {g_u8}, {b_u8}, {})",
            format_css_alpha(a_out)
        ))
    }
}

/// CSS Color 4 premultiplied `color-mix(in srgb, <color>, <other> <percent>%)` (`theme-palette.ts:L7-L9`).
pub fn srgb_mix(color: &str, other: &str, percent: f64) -> Result<String, ThemePaletteError> {
    if !percent.is_finite() {
        return Err(ThemePaletteError::InvalidColor(format!(
            "non-finite mix percentage: {percent}"
        )));
    }
    let (r1, g1, b1, a1) = parse_srgb_components(color)?;
    let (r2, g2, b2, a2) = parse_srgb_components(other)?;
    let p2 = (percent / 100.0).clamp(0.0, 1.0);
    let p1 = 1.0 - p2;
    let a_out = (a1 * p1 + a2 * p2).clamp(0.0, 1.0);
    if a_out <= 1e-9 {
        return Ok("rgba(0, 0, 0, 0)".to_string());
    }
    let r_u8 = ((r1 * a1 * p1 + r2 * a2 * p2) / a_out + 0.5)
        .floor()
        .clamp(0.0, 255.0) as u8;
    let g_u8 = ((g1 * a1 * p1 + g2 * a2 * p2) / a_out + 0.5)
        .floor()
        .clamp(0.0, 255.0) as u8;
    let b_u8 = ((b1 * a1 * p1 + b2 * a2 * p2) / a_out + 0.5)
        .floor()
        .clamp(0.0, 255.0) as u8;
    if (a_out - 1.0).abs() <= 1e-9 {
        Ok(format!("#{r_u8:02x}{g_u8:02x}{b_u8:02x}"))
    } else {
        Ok(format!(
            "rgba({r_u8}, {g_u8}, {b_u8}, {})",
            format_css_alpha(a_out)
        ))
    }
}

impl OpenBotPalette {
    fn build_from_pairs(
        palette_id: String,
        scheme: OpenBotColorScheme,
        custom_source: Option<AppThemePalette>,
        entries: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut tokens = BTreeMap::new();
        let mut rgba_tokens = BTreeMap::new();
        for (key, val) in entries {
            if let Some(rgba) = parse_css_color_to_rgba(&val) {
                rgba_tokens.insert(key.clone(), rgba);
            }
            tokens.insert(key, val);
        }
        Self {
            palette_id,
            scheme,
            custom_source,
            tokens,
            rgba_tokens,
        }
    }

    /// Default OpenBot dark palette (`"default-dark"`, exact 58 `tokens.css` defaults).
    pub fn dark() -> Self {
        Self::build_from_pairs(
            "default-dark".to_string(),
            OpenBotColorScheme::Dark,
            None,
            DARK_DEFAULT_TOKENS
                .iter()
                .map(|&(k, v)| (k.to_string(), v.to_string())),
        )
    }

    /// Default OpenBot light palette (`"default-light"`, exact 58 `tokens-light.css` defaults).
    pub fn light() -> Self {
        Self::build_from_pairs(
            "default-light".to_string(),
            OpenBotColorScheme::Light,
            None,
            LIGHT_DEFAULT_TOKENS
                .iter()
                .map(|&(k, v)| (k.to_string(), v.to_string())),
        )
    }

    /// Returns `OpenBotPalette::dark()` or `OpenBotPalette::light()` for `scheme`.
    pub fn for_scheme(scheme: OpenBotColorScheme) -> Self {
        match scheme {
            OpenBotColorScheme::Dark => Self::dark(),
            OpenBotColorScheme::Light => Self::light(),
        }
    }

    /// Derives all 58 `--openbot-*` tokens from a custom 12-color `AppThemePalette`
    /// using the exact formulas from `src/renderer/src/theme-palette.ts:L11-L79`.
    pub fn from_custom_palette(palette: AppThemePalette) -> Self {
        let base_fallback = Self::for_scheme(palette.scheme);
        let c = &palette.colors;
        let mix_or = |a: &str, b: &str, pct: f64, fallback_key: &str| -> String {
            srgb_mix(a, b, pct).unwrap_or_else(|_| {
                base_fallback
                    .tokens
                    .get(fallback_key)
                    .cloned()
                    .unwrap_or_else(|| "#000000".to_string())
            })
        };
        let clear_or = |a: &str, pct: f64, fallback_key: &str| -> String {
            srgb_clear(a, pct).unwrap_or_else(|_| {
                base_fallback
                    .tokens
                    .get(fallback_key)
                    .cloned()
                    .unwrap_or_else(|| "rgba(0, 0, 0, 0)".to_string())
            })
        };

        let raised = if palette.scheme == OpenBotColorScheme::Light {
            mix_or(&c.canvas, "#ffffff", 70.0, "--openbot-bg-surface")
        } else {
            c.surface.clone()
        };
        let tabs_bar = if palette.scheme == OpenBotColorScheme::Light {
            mix_or(&c.canvas, &c.surface, 30.0, "--openbot-tabs-bar-bg")
        } else {
            c.surface.clone()
        };
        let tabs_pill = if palette.scheme == OpenBotColorScheme::Light {
            raised.clone()
        } else {
            mix_or(&c.surface, &c.text, 8.0, "--openbot-tabs-pill-bg")
        };

        let derived: [(&str, String); 58] = [
            ("--openbot-bg-canvas", c.canvas.clone()),
            (
                "--openbot-bg-canvas-clear",
                clear_or(&c.canvas, 0.0, "--openbot-bg-canvas-clear"),
            ),
            ("--openbot-bg-frame", c.frame.clone()),
            ("--openbot-bg-sidebar", c.sidebar.clone()),
            ("--openbot-bg-native-sidebar", c.sidebar.clone()),
            ("--openbot-bg-sheet", raised.clone()),
            ("--openbot-bg-sheet-inset", c.frame.clone()),
            ("--openbot-bg-grouped", raised.clone()),
            ("--openbot-bg-surface", raised.clone()),
            (
                "--openbot-bg-surface-raised",
                mix_or(&raised, &c.text, 3.0, "--openbot-bg-surface-raised"),
            ),
            ("--openbot-bg-control", raised.clone()),
            (
                "--openbot-bg-control-hover",
                mix_or(&raised, &c.text, 4.0, "--openbot-bg-control-hover"),
            ),
            (
                "--openbot-bg-control-active",
                mix_or(&raised, &c.text, 8.0, "--openbot-bg-control-active"),
            ),
            ("--openbot-tabs-bar-bg", tabs_bar),
            ("--openbot-tabs-pill-bg", tabs_pill),
            ("--openbot-marketplace-detail-bg", c.frame.clone()),
            (
                "--openbot-bg-overlay",
                clear_or(&c.canvas, 92.0, "--openbot-bg-overlay"),
            ),
            (
                "--openbot-bg-glass",
                clear_or(&raised, 76.0, "--openbot-bg-glass"),
            ),
            (
                "--openbot-scroll-edge-overlay",
                clear_or(&c.canvas, 72.0, "--openbot-scroll-edge-overlay"),
            ),
            (
                "--openbot-border",
                clear_or(&c.border, 35.0, "--openbot-border"),
            ),
            (
                "--openbot-border-strong",
                clear_or(&c.border, 50.0, "--openbot-border-strong"),
            ),
            ("--openbot-border-grouped", c.border.clone()),
            (
                "--openbot-dialog-ring",
                clear_or(&c.border, 70.0, "--openbot-dialog-ring"),
            ),
            (
                "--openbot-image-outline",
                clear_or(&c.border, 60.0, "--openbot-image-outline"),
            ),
            ("--openbot-hover", clear_or(&c.text, 5.0, "--openbot-hover")),
            ("--openbot-text-primary", c.text.clone()),
            (
                "--openbot-text-secondary",
                mix_or(&c.text, &c.text_muted, 25.0, "--openbot-text-secondary"),
            ),
            ("--openbot-text-muted", c.text_muted.clone()),
            (
                "--openbot-text-dim",
                mix_or(&c.text_muted, &c.canvas, 30.0, "--openbot-text-dim"),
            ),
            ("--openbot-text-placeholder", c.text_muted.clone()),
            ("--openbot-text-readable", c.text_muted.clone()),
            ("--openbot-text-grouped-secondary", c.text_muted.clone()),
            ("--openbot-settings-text-muted", c.text_muted.clone()),
            ("--openbot-surface-light", c.text.clone()),
            ("--openbot-text-on-light", c.canvas.clone()),
            (
                "--openbot-text-muted-on-light",
                clear_or(&c.canvas, 64.0, "--openbot-text-muted-on-light"),
            ),
            (
                "--openbot-text-dim-on-light",
                clear_or(&c.canvas, 40.0, "--openbot-text-dim-on-light"),
            ),
            (
                "--openbot-border-on-light",
                clear_or(&c.canvas, 16.0, "--openbot-border-on-light"),
            ),
            ("--openbot-accent", c.accent.clone()),
            (
                "--openbot-accent-hover",
                mix_or(&c.accent, &c.text, 15.0, "--openbot-accent-hover"),
            ),
            ("--openbot-accent-text", c.accent.clone()),
            (
                "--openbot-accent-soft",
                clear_or(&c.accent, 16.0, "--openbot-accent-soft"),
            ),
            (
                "--openbot-accent-strong",
                clear_or(&c.accent, 34.0, "--openbot-accent-strong"),
            ),
            ("--openbot-text-on-accent", c.accent_foreground.clone()),
            ("--openbot-success", c.success.clone()),
            ("--openbot-success-text", c.success.clone()),
            (
                "--openbot-success-soft",
                clear_or(&c.success, 14.0, "--openbot-success-soft"),
            ),
            (
                "--openbot-success-wash",
                clear_or(&c.success, 6.0, "--openbot-success-wash"),
            ),
            ("--openbot-warning", c.warning.clone()),
            ("--openbot-warning-text", c.warning.clone()),
            (
                "--openbot-warning-soft",
                clear_or(&c.warning, 14.0, "--openbot-warning-soft"),
            ),
            ("--openbot-danger", c.danger.clone()),
            (
                "--openbot-danger-hover",
                mix_or(&c.danger, &c.text, 15.0, "--openbot-danger-hover"),
            ),
            ("--openbot-danger-text", c.danger.clone()),
            (
                "--openbot-danger-soft",
                clear_or(&c.danger, 14.0, "--openbot-danger-soft"),
            ),
            (
                "--openbot-danger-soft-hover",
                clear_or(&c.danger, 22.0, "--openbot-danger-soft-hover"),
            ),
            (
                "--openbot-danger-focus",
                clear_or(&c.danger, 36.0, "--openbot-danger-focus"),
            ),
            (
                "--openbot-danger-border",
                clear_or(&c.danger, 45.0, "--openbot-danger-border"),
            ),
        ];

        let palette_id = palette.id.clone();
        let scheme = palette.scheme;
        Self::build_from_pairs(
            palette_id,
            scheme,
            Some(palette),
            derived.into_iter().map(|(k, v)| (k.to_string(), v)),
        )
    }

    /// Parses and validates a raw theme JSON string for `id` and derives its 58-token `OpenBotPalette`.
    pub fn from_theme_json_str(id: &str, json: &str) -> Result<Self, ThemePaletteError> {
        let parsed = super::watcher::parse_theme_palette_json_str(id, json)?;
        Ok(Self::from_custom_palette(parsed))
    }

    /// Reads, validates, and derives an `OpenBotPalette` from a `<id>.json` file on disk.
    pub fn from_theme_json_path(path: &Path) -> Result<Self, ThemePaletteError> {
        let parsed = super::watcher::read_theme_palette_file(path)?;
        Ok(Self::from_custom_palette(parsed))
    }

    /// Returns the CSS string value for `name` (e.g. `"--openbot-bg-canvas"`).
    pub fn token(&self, name: &str) -> Option<&str> {
        self.tokens.get(name).map(String::as_str)
    }

    /// Returns the parsed GPUI `Rgba` value for `name`.
    pub fn rgba(&self, name: &str) -> Option<Rgba> {
        self.rgba_tokens.get(name).copied()
    }

    /// Returns the full token map (`58` `--openbot-*` entries).
    pub fn tokens_map(&self) -> &BTreeMap<String, String> {
        &self.tokens
    }

    /// Returns the number of semantic tokens in `self.tokens` (`58`).
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    #[inline]
    fn hex_rgba(hex: u32, alpha: f32) -> Rgba {
        let mut color = gpui::rgb(hex);
        color.a = alpha;
        color
    }

    #[inline]
    fn token_rgba_or(&self, name: &str, dark_hex: u32, light_hex: u32) -> Rgba {
        self.rgba(name).unwrap_or_else(|| {
            gpui::rgb(if self.scheme.is_dark() {
                dark_hex
            } else {
                light_hex
            })
        })
    }

    #[inline]
    fn token_rgba_alpha_or(
        &self,
        name: &str,
        dark_hex: u32,
        dark_alpha: f32,
        light_hex: u32,
        light_alpha: f32,
    ) -> Rgba {
        self.rgba(name).unwrap_or_else(|| {
            if self.scheme.is_dark() {
                Self::hex_rgba(dark_hex, dark_alpha)
            } else {
                Self::hex_rgba(light_hex, light_alpha)
            }
        })
    }

    /// Outer window frame (`#0d0d0d` dark / `#e8e8eb` light).
    pub fn bg_frame(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-frame", 0x0D0D0D, 0xE8E8EB)
    }

    /// Main inset workspace canvas (`#1a1a1a` dark / `#f5f5f6` light).
    pub fn bg_canvas(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-canvas", 0x1A1A1A, 0xF5F5F6)
    }

    /// Chat sidebar surface (`#1a1a1a` dark / `#f5f5f6` light).
    pub fn bg_sidebar(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-sidebar", 0x1A1A1A, 0xF5F5F6)
    }

    /// Primary elevated card/menu/control surface (`#212121` dark / `#ffffff` light).
    pub fn bg_surface(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-surface", 0x212121, 0xFFFFFF)
    }

    /// Raised card / own-bubble neutral surface (`#242424` dark / `#fcfcfc` light).
    pub fn bg_surface_raised(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-surface-raised", 0x242424, 0xFCFCFC)
    }

    /// Default interactive control background (`#212121` dark / `#ffffff` light).
    pub fn bg_control(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-control", 0x212121, 0xFFFFFF)
    }

    /// Hovered control background (`#242424` dark / `#f0f0f2` light).
    pub fn bg_control_hover(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-control-hover", 0x242424, 0xF0F0F2)
    }

    /// Active/selected control background (`#2a2a2a` dark / `#e8e8eb` light).
    pub fn bg_control_active(&self) -> Rgba {
        self.token_rgba_or("--openbot-bg-control-active", 0x2A2A2A, 0xE8E8EB)
    }

    /// Subtle translucent border (`rgba(255,255,255,0.035)` dark / `rgba(20,20,20,0.07)` light).
    pub fn border(&self) -> Rgba {
        self.token_rgba_alpha_or("--openbot-border", 0xFFFFFF, 0.035, 0x141414, 0.07)
    }

    /// Stronger translucent border (`rgba(255,255,255,0.045)` dark / `rgba(20,20,20,0.10)` light).
    pub fn border_strong(&self) -> Rgba {
        self.token_rgba_alpha_or("--openbot-border-strong", 0xFFFFFF, 0.045, 0x141414, 0.10)
    }

    /// Crisp opaque 1px hairline border (`#262626` dark / `#e8e8eb` light).
    pub fn border_hairline(&self) -> Rgba {
        gpui::rgb(if self.scheme.is_dark() {
            OPENBOT_BORDER_SOLID_DARK_HEX
        } else {
            OPENBOT_BORDER_SOLID_LIGHT_HEX
        })
    }

    /// Crisp opaque 1px popover/card border (`#323232` dark / `#dcdce0` light).
    pub fn border_strong_hairline(&self) -> Rgba {
        gpui::rgb(if self.scheme.is_dark() {
            OPENBOT_BORDER_STRONG_SOLID_DARK_HEX
        } else {
            OPENBOT_BORDER_STRONG_SOLID_LIGHT_HEX
        })
    }

    /// Crisp opaque 1px frame-to-card / surface border (`#262626` dark / `#dcdce0` light)
    /// providing clear edge separation against both `#0d0d0d` dark and `#e8e8eb` light frames.
    pub fn frame_card_border(&self) -> Rgba {
        gpui::rgb(if self.scheme.is_dark() {
            OPENBOT_BORDER_SOLID_DARK_HEX
        } else {
            OPENBOT_BORDER_STRONG_SOLID_LIGHT_HEX
        })
    }

    /// Translucent hover wash (`rgba(255,255,255,0.06)` dark / `rgba(20,20,20,0.05)` light).
    pub fn hover(&self) -> Rgba {
        self.token_rgba_alpha_or("--openbot-hover", 0xFFFFFF, 0.06, 0x141414, 0.05)
    }

    /// Primary foreground ink (`#ffffff` dark / `#141414` light).
    pub fn text_primary(&self) -> Rgba {
        self.token_rgba_or("--openbot-text-primary", 0xFFFFFF, 0x141414)
    }

    /// Secondary foreground ink (`#dcdcdc` dark / `#3a3a3c` light).
    pub fn text_secondary(&self) -> Rgba {
        self.token_rgba_or("--openbot-text-secondary", 0xDCDCDC, 0x3A3A3C)
    }

    /// Muted label/icon ink (`#979797` dark / `#6b6b70` light).
    pub fn text_muted(&self) -> Rgba {
        self.token_rgba_or("--openbot-text-muted", 0x979797, 0x6B6B70)
    }

    /// Dim hint/timestamp ink (`#6a6a6a` dark / `#8e8e93` light).
    pub fn text_dim(&self) -> Rgba {
        self.token_rgba_or("--openbot-text-dim", 0x6A6A6A, 0x8E8E93)
    }

    /// Translucent readable body text (`rgba(255,255,255,0.52)` dark / `rgba(20,20,20,0.60)` light).
    pub fn text_readable(&self) -> Rgba {
        self.token_rgba_alpha_or("--openbot-text-readable", 0xFFFFFF, 0.52, 0x141414, 0.60)
    }

    /// High-contrast monochrome primary CTA surface (`#f0f0f0` dark / `#1c1c1e` light).
    pub fn surface_light(&self) -> Rgba {
        self.token_rgba_or("--openbot-surface-light", 0xF0F0F0, 0x1C1C1E)
    }

    /// High-contrast text on `surface_light()` (`#141414` dark / `#ffffff` light).
    pub fn text_on_light(&self) -> Rgba {
        self.token_rgba_or("--openbot-text-on-light", 0x141414, 0xFFFFFF)
    }

    /// Brand blue accent (`#007cf7`).
    pub fn accent(&self) -> Rgba {
        self.token_rgba_or("--openbot-accent", 0x007CF7, 0x007CF7)
    }

    /// Hovered accent (`#1f8fff` dark / `#006ad4` light).
    pub fn accent_hover(&self) -> Rgba {
        self.token_rgba_or("--openbot-accent-hover", 0x1F8FFF, 0x006AD4)
    }

    /// Readable accent text (`#42a0ff` dark / `#0064c8` light).
    pub fn accent_text(&self) -> Rgba {
        self.token_rgba_or("--openbot-accent-text", 0x42A0FF, 0x0064C8)
    }

    /// Soft accent wash (`rgba(0,124,247,0.16)` dark / `rgba(0,124,247,0.12)` light).
    pub fn accent_soft(&self) -> Rgba {
        self.token_rgba_alpha_or("--openbot-accent-soft", 0x007CF7, 0.16, 0x007CF7, 0.12)
    }

    /// Success green (`#31cf76` dark / `#1fa85a` light).
    pub fn success(&self) -> Rgba {
        self.token_rgba_or("--openbot-success", 0x31CF76, 0x1FA85A)
    }

    /// Warning amber (`#ff9412` dark / `#f08a00` light).
    pub fn warning(&self) -> Rgba {
        self.token_rgba_or("--openbot-warning", 0xFF9412, 0xF08A00)
    }

    /// Danger red (`#fd2f3b` dark / `#e8222d` light).
    pub fn danger(&self) -> Rgba {
        self.token_rgba_or("--openbot-danger", 0xFD2F3B, 0xE8222D)
    }

    /// Unread indicator surface (`#74b9ff`).
    pub fn unread_surface(&self) -> Rgba {
        gpui::rgb(0x74B9FF)
    }

    /// Unread indicator foreground text (`#07131f`).
    pub fn unread_text(&self) -> Rgba {
        gpui::rgb(0x07131F)
    }

    /// Update / "new" notice dot (`#ff1e5e` dark / `#e0004a` light).
    pub fn badge_new(&self) -> Rgba {
        gpui::rgb(if self.scheme.is_dark() {
            0xFF1E5E
        } else {
            0xE0004A
        })
    }

    /// Felt unread plum gradient stops `(#705083, #4a285c)`.
    pub fn felt_plum(&self) -> (Rgba, Rgba) {
        (gpui::rgb(0x705083), gpui::rgb(0x4A285C))
    }

    /// Felt waiting amber gradient stops `(#ff9511, #cb680c)`.
    pub fn felt_amber(&self) -> (Rgba, Rgba) {
        (gpui::rgb(0xFF9511), gpui::rgb(0xCB680C))
    }

    /// Felt error raspberry gradient stops `(#f52070, #b40641)`.
    pub fn felt_raspberry(&self) -> (Rgba, Rgba) {
        (gpui::rgb(0xF52070), gpui::rgb(0xB40641))
    }

    /// Felt badge cream foreground (`#f6efe2`).
    pub fn felt_cream(&self) -> Rgba {
        gpui::rgb(0xF6EFE2)
    }

    /// Computes the OKLab-clamped readable chat background for `hex` in this palette's scheme.
    pub fn readable_chat_background(&self, hex: &str) -> Result<String, ThemePaletteError> {
        readable_chat_background(hex, self.scheme)
    }
}

impl OpenBotPalette {
    /// Derives the 15 `--window-glass-*` translucent tokens from `window-glass.css:L2-L17`.
    pub fn window_glass_tokens(&self) -> BTreeMap<String, String> {
        const SPECS: [(&str, &str, f64); 15] = [
            ("--window-glass-frame", "--openbot-bg-frame", 20.0),
            ("--window-glass-card", "--openbot-bg-canvas", 30.0),
            ("--window-glass-canvas", "--openbot-bg-canvas", 25.0),
            ("--window-glass-sidebar", "--openbot-bg-sidebar", 25.0),
            ("--window-glass-frame-fill", "--openbot-bg-frame", 30.0),
            ("--window-glass-sheet", "--openbot-bg-sheet", 35.0),
            (
                "--window-glass-sheet-inset",
                "--openbot-bg-sheet-inset",
                25.0,
            ),
            ("--window-glass-grouped", "--openbot-bg-grouped", 35.0),
            ("--window-glass-surface", "--openbot-bg-surface", 35.0),
            (
                "--window-glass-surface-raised",
                "--openbot-bg-surface-raised",
                40.0,
            ),
            ("--window-glass-control", "--openbot-bg-control", 35.0),
            (
                "--window-glass-control-hover",
                "--openbot-bg-control-hover",
                45.0,
            ),
            (
                "--window-glass-control-active",
                "--openbot-bg-control-active",
                55.0,
            ),
            ("--window-glass-tabs-bar", "--openbot-tabs-bar-bg", 35.0),
            ("--window-glass-tabs-pill", "--openbot-tabs-pill-bg", 45.0),
        ];
        let mut out = BTreeMap::new();
        for (glass_key, src_key, pct) in SPECS {
            if let Some(src_val) = self.token(src_key) {
                if let Ok(cleared) = srgb_clear(src_val, pct) {
                    out.insert(glass_key.to_string(), cleared);
                }
            }
        }
        out
    }
}

/// Canonical OpenBot chat color preset (`packages/brand/src/chat-colors.ts:L3-L48`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChatColorPreset {
    pub id: &'static str,
    pub hex: &'static str,
    pub light_swatch: &'static str,
    pub dark_swatch: &'static str,
}

/// All 11 OpenBot chat color presets and static light/dark background swatches.
pub const CHAT_COLOR_PRESETS: [ChatColorPreset; 11] = [
    ChatColorPreset {
        id: "red",
        hex: "#f75d59",
        light_swatch: "#ffefed",
        dark_swatch: "#311b19",
    },
    ChatColorPreset {
        id: "orange",
        hex: "#eb7007",
        light_swatch: "#fef0e9",
        dark_swatch: "#2f1d12",
    },
    ChatColorPreset {
        id: "yellow",
        hex: "#e5a50a",
        light_swatch: "#faf2e6",
        dark_swatch: "#2a200d",
    },
    ChatColorPreset {
        id: "green",
        hex: "#07b655",
        light_swatch: "#ebf7ed",
        dark_swatch: "#152619",
    },
    ChatColorPreset {
        id: "cyan",
        hex: "#0eafa2",
        light_swatch: "#e7f8f5",
        dark_swatch: "#0a2724",
    },
    ChatColorPreset {
        id: "sky",
        hex: "#06a8d3",
        light_swatch: "#e7f6fd",
        dark_swatch: "#0c252e",
    },
    ChatColorPreset {
        id: "blue",
        hex: "#4699fe",
        light_swatch: "#ecf4ff",
        dark_swatch: "#162232",
    },
    ChatColorPreset {
        id: "indigo",
        hex: "#8688ff",
        light_swatch: "#f1f2ff",
        dark_swatch: "#1e2032",
    },
    ChatColorPreset {
        id: "violet",
        hex: "#b474f4",
        light_swatch: "#f6f1fe",
        dark_swatch: "#251d2f",
    },
    ChatColorPreset {
        id: "magenta",
        hex: "#dc63c6",
        light_swatch: "#fceff8",
        dark_swatch: "#2c1b28",
    },
    ChatColorPreset {
        id: "pink",
        hex: "#f05b93",
        light_swatch: "#ffeff3",
        dark_swatch: "#2f1a21",
    },
];

/// Looks up one of the 11 `CHAT_COLOR_PRESETS` by preset `id` or `hex` (case-insensitive).
pub fn find_chat_color_preset(id_or_hex: &str) -> Option<&'static ChatColorPreset> {
    let needle = id_or_hex.trim();
    CHAT_COLOR_PRESETS
        .iter()
        .find(|p| p.id.eq_ignore_ascii_case(needle) || p.hex.eq_ignore_ascii_case(needle))
}

#[inline]
fn srgb_channel_to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_channel_to_gamma(channel: f64) -> f64 {
    let value = if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.max(0.0).powf(1.0 / 2.4) - 0.055
    };
    value.clamp(0.0, 1.0)
}

#[inline]
fn gamma_hex_byte(linear_ch: f64) -> u8 {
    (linear_channel_to_gamma(linear_ch) * 255.0 + 0.5).floor() as u8
}

fn srgb_u8_to_oklab(r_u8: u8, g_u8: u8, b_u8: u8) -> (f64, f64, f64) {
    let r = srgb_channel_to_linear(f64::from(r_u8) / 255.0);
    let g = srgb_channel_to_linear(f64::from(g_u8) / 255.0);
    let b = srgb_channel_to_linear(f64::from(b_u8) / 255.0);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let lightness = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let b_axis = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    (lightness, a, b_axis)
}

fn oklab_to_srgb_bytes(lightness: f64, a: f64, b_axis: f64) -> (u8, u8, u8) {
    let l2 = (lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b_axis).powi(3);
    let m2 = (lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b_axis).powi(3);
    let s2 = (lightness - 0.089_484_177_5 * a - 1.291_485_548 * b_axis).powi(3);
    let red = 4.076_741_662_1 * l2 - 3.307_711_591_3 * m2 + 0.230_969_929_2 * s2;
    let green = -1.268_438_004_6 * l2 + 2.609_757_401_1 * m2 - 0.341_319_396_5 * s2;
    let blue = -0.004_196_086_3 * l2 - 0.703_418_614_7 * m2 + 1.707_614_701 * s2;
    (
        gamma_hex_byte(red),
        gamma_hex_byte(green),
        gamma_hex_byte(blue),
    )
}

/// Interpolates two `#RRGGBB` / `#RRGGBBAA` colors in perceptual OKLab space (`weight_c1` in `0.0..=1.0`).
pub fn oklab_mix(c1_hex: &str, c2_hex: &str, weight_c1: f64) -> Result<String, ThemePaletteError> {
    if !weight_c1.is_finite() {
        return Err(ThemePaletteError::InvalidColor(format!(
            "non-finite OKLab weight: {weight_c1}"
        )));
    }
    let (r1, g1, b1, a1) = parse_hex_rgb_or_rgba(c1_hex)?;
    let (r2, g2, b2, a2) = parse_hex_rgb_or_rgba(c2_hex)?;
    let w1 = weight_c1.clamp(0.0, 1.0);
    let w2 = 1.0 - w1;
    let a_out = (a1 * w1 + a2 * w2).clamp(0.0, 1.0);
    if a_out <= 1e-9 {
        return Ok("rgba(0, 0, 0, 0)".to_string());
    }
    let (l1, ok_a1, ok_b1) = srgb_u8_to_oklab(r1, g1, b1);
    let (l2, ok_a2, ok_b2) = srgb_u8_to_oklab(r2, g2, b2);
    let l_mix = (l1 * a1 * w1 + l2 * a2 * w2) / a_out;
    let a_mix = (ok_a1 * a1 * w1 + ok_a2 * a2 * w2) / a_out;
    let b_mix = (ok_b1 * a1 * w1 + ok_b2 * a2 * w2) / a_out;
    let (ro, go, bo) = oklab_to_srgb_bytes(l_mix, a_mix, b_mix);
    if (a_out - 1.0).abs() <= 1e-9 {
        Ok(format!("#{ro:02x}{go:02x}{bo:02x}"))
    } else {
        Ok(format!(
            "rgba({ro}, {go}, {bo}, {})",
            format_css_alpha(a_out)
        ))
    }
}

/// Computes the OKLab-clamped readable chat background color (`packages/brand/src/chat-colors.ts:L70-L96`).
///
/// For `#f75d59`, produces `#fceae8` in `Light` (`L >= 0.95, C <= 0.02`) and `#442321` in `Dark` (`L <= 0.30, C <= 0.05`).
pub fn readable_chat_background(
    hex: &str,
    scheme: OpenBotColorScheme,
) -> Result<String, ThemePaletteError> {
    let validated = super::watcher::validate_chat_background_hex(hex)?;
    let (r_u8, g_u8, b_u8, _) = parse_hex_rgb_or_rgba(&validated)?;
    let (lightness, a, b_axis) = srgb_u8_to_oklab(r_u8, g_u8, b_u8);
    let chroma = a.hypot(b_axis);
    let (next_lightness, next_chroma) = match scheme {
        OpenBotColorScheme::Light => (lightness.max(0.95), chroma.min(0.02)),
        OpenBotColorScheme::Dark => (lightness.min(0.30), chroma.min(0.05)),
    };
    let scale = if chroma == 0.0 {
        0.0
    } else {
        next_chroma / chroma
    };
    let next_a = a * scale;
    let next_b = b_axis * scale;
    let (ro, go, bo) = oklab_to_srgb_bytes(next_lightness, next_a, next_b);
    Ok(format!("#{ro:02x}{go:02x}{bo:02x}"))
}

/// Convenience wrapper for [`readable_chat_background`] accepting `is_dark: bool`.
pub fn readable_chat_background_for_dark(
    hex: &str,
    is_dark: bool,
) -> Result<String, ThemePaletteError> {
    readable_chat_background(
        hex,
        if is_dark {
            OpenBotColorScheme::Dark
        } else {
            OpenBotColorScheme::Light
        },
    )
}

/// Validates `color` (`#RRGGBB` or `#RRGGBBAA`) and checks WCAG white-text contrast
/// (`1.05 / (L + 0.05) < 3.0`, `own-bubble.ts:L43-L50`).
pub fn try_solid_bubble_takes_dark_text(color: &str) -> Result<bool, ThemePaletteError> {
    let (r, g, b, _) = parse_hex_rgb_or_rgba(color.trim())?;
    let luminance = 0.2126 * srgb_channel_to_linear(f64::from(r) / 255.0)
        + 0.7152 * srgb_channel_to_linear(f64::from(g) / 255.0)
        + 0.0722 * srgb_channel_to_linear(f64::from(b) / 255.0);
    Ok(1.05 / (luminance + 0.05) < 3.0)
}

/// Returns `true` when a solid message bubble of `color` (`#RRGGBB`) requires dark text (`own-bubble.ts:L43-L50`).
pub fn solid_bubble_takes_dark_text(color: &str) -> bool {
    try_solid_bubble_takes_dark_text(color).unwrap_or(false)
}

/// Supported OpenBot interface zoom percentages (`@openbot/contracts/app-interface-size`).
pub const APP_INTERFACE_SIZES: [u16; 10] = [67, 75, 80, 90, 100, 110, 125, 150, 175, 200];
/// Default OpenBot interface zoom percentage (`100%`).
pub const DEFAULT_APP_INTERFACE_SIZE: u16 = 100;

/// Checks whether `percent` is a valid member of `APP_INTERFACE_SIZES`.
pub fn is_app_interface_size(percent: u16) -> bool {
    APP_INTERFACE_SIZES.contains(&percent)
}

/// Validates an OpenBot custom font family name (`/^[^\p{Cc}"\\;{}<>]{1,128}$/u` trimmed, `app-appearance.ts:L35-L39`).
pub fn is_valid_app_font_family(value: &str) -> bool {
    if value.trim() != value {
        return false;
    }
    let u16_len: usize = value.chars().map(char::len_utf16).sum();
    if !(1..=128).contains(&u16_len) {
        return false;
    }
    !value
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '"' | '\\' | ';' | '{' | '}' | '<' | '>'))
}

/// Supported OpenBot own-bubble styles (`own-bubble.ts:L7-L9`).
pub const CHAT_BUBBLE_STYLES: [&str; 3] = ["solid", "tint", "classic"];
/// Default OpenBot own-bubble style (`"solid"`).
pub const DEFAULT_CHAT_BUBBLE_STYLE: &str = "solid";

/// Static brand logo & felt surface colors (`tokens.css:L153-L170`).
pub const OPENBOT_LOGO_PRODUCTION: &str = "#d6adf2";
pub const OPENBOT_LOGO_DEV: &str = "#e3b866";
pub const OPENBOT_LOGO_PREVIEW: &str = "#61c985";
pub const OPENBOT_LOGO_EYE: &str = "#040007";
pub const OPENBOT_FELT_PLUM: (&str, &str) = ("#705083", "#4a285c");
pub const OPENBOT_FELT_RASPBERRY: (&str, &str) = ("#f52070", "#b40641");
pub const OPENBOT_FELT_AMBER: (&str, &str) = ("#ff9511", "#cb680c");
pub const OPENBOT_FELT_CREAM: &str = "#f6efe2";
pub const OPENBOT_FELT_CREAM_STROKE: &str = "#efe4d2";
pub const OPENBOT_ACTION_BG: &str = "#f0f0f0";
pub const OPENBOT_ACTION_FG: &str = "#141414";
pub const OPENBOT_ACTION_LIGHT_BG: &str = "#1c1c1e";
pub const OPENBOT_ACTION_LIGHT_FG: &str = "#ffffff";
pub const OPENBOT_PAPER_BG: &str = "#f0f0f0";
pub const OPENBOT_PAPER_TEXT: &str = "#141414";
pub const OPENBOT_UNREAD_SURFACE: &str = "#74b9ff";
pub const OPENBOT_UNREAD_TEXT: &str = "#07131f";
pub const OPENBOT_BADGE_NEW_DARK: &str = "#ff1e5e";
pub const OPENBOT_BADGE_NEW_LIGHT: &str = "#e0004a";
pub const OPENBOT_BORDER_SOLID_DARK_HEX: u32 = 0x262626;
pub const OPENBOT_BORDER_STRONG_SOLID_DARK_HEX: u32 = 0x323232;
pub const OPENBOT_BORDER_SOLID_LIGHT_HEX: u32 = 0xE8E8EB;
pub const OPENBOT_BORDER_STRONG_SOLID_LIGHT_HEX: u32 = 0xDCDCE0;

/// Provider & chart series brand tokens (`tokens.css:L136-L152`, `tokens-light.css:L57-L64`).
pub const OPENBOT_PROVIDER_CLAUDE: &str = "#d97757";
pub const OPENBOT_PROVIDER_ANTIGRAVITY: &str = "#8e75b2";
pub const OPENBOT_CHART_CODEX: (&str, &str) = ("#d6adf2", "#8e4fc0");
pub const OPENBOT_CHART_GROK: (&str, &str) = ("#6bc7d9", "#16849b");
pub const OPENBOT_STORAGE_SERIES: [(&str, &str, &str); 5] = [
    ("workspaces", "#3987e5", "#2f74c9"),
    ("files", "#d95926", "#c64d1d"),
    ("chats", "#199e70", "#148a61"),
    ("downloads", "#9085e9", "#6f62d6"),
    ("runtimes", "#c98500", "#a86f00"),
];

/// Static glass surface tokens for dark and light modes (`tokens.css:L32-L36`, `tokens-light.css:L26-L30`).
pub const OPENBOT_GLASS_TOKENS_DARK: [(&str, &str); 5] = [
    ("--openbot-glass-surface", "rgba(255, 255, 255, 0.07)"),
    ("--openbot-glass-surface-hover", "rgba(255, 255, 255, 0.11)"),
    ("--openbot-glass-border", "rgba(255, 255, 255, 0.18)"),
    ("--openbot-glass-highlight", "rgba(255, 255, 255, 0.12)"),
    ("--openbot-glass-shadow", "rgba(0, 0, 0, 0.28)"),
];
pub const OPENBOT_GLASS_TOKENS_LIGHT: [(&str, &str); 5] = [
    ("--openbot-glass-surface", "rgba(20, 20, 20, 0.04)"),
    ("--openbot-glass-surface-hover", "rgba(20, 20, 20, 0.07)"),
    ("--openbot-glass-border", "rgba(20, 20, 20, 0.12)"),
    ("--openbot-glass-highlight", "rgba(255, 255, 255, 0.6)"),
    ("--openbot-glass-shadow", "transparent"),
];

/// Typography `(font_size_px, line_height_px)` scale (`tokens.css:L250-L265`).
pub const OPENBOT_TYPE_SCALE_PX: [(&str, f32, f32); 7] = [
    ("sm", 13.0, 18.0),
    ("md", 14.0, 20.0),
    ("lg", 15.0, 22.0),
    ("xl", 16.0, 22.0),
    ("2xl", 18.0, 24.0),
    ("3xl", 24.0, 30.0),
    ("display", 32.0, 38.0),
];
/// Spacing scale in px (`tokens.css:L278-L285`).
pub const OPENBOT_SPACE_SCALE_PX: [f32; 8] = [2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0];
/// Control height scale in px (`tokens.css:L299-L303`).
pub const OPENBOT_CONTROL_HEIGHTS_PX: [(&str, f32); 5] = [
    ("xs", 24.0),
    ("sm", 28.0),
    ("md", 32.0),
    ("lg", 36.0),
    ("xl", 40.0),
];
/// Corner radius scale in px (`tokens.css:L287-L297`).
pub const OPENBOT_RADII_PX: [(&str, f32); 11] = [
    ("xs", 4.0),
    ("sm", 6.0),
    ("md", 8.0),
    ("lg", 12.0),
    ("card", 14.0),
    ("grouped", 16.0),
    ("xl", 20.0),
    ("bubble", 22.0),
    ("bubble-inset", 9.0),
    ("modal", 30.0),
    ("pill", 999.0),
];
/// Motion duration constants in ms (`tokens.css:L325-L338`).
pub const OPENBOT_DURATIONS_MS: [(&str, u32); 9] = [
    ("dialog", 100),
    ("fast", 120),
    ("normal", 160),
    ("slow", 200),
    ("toast-exit", 220),
    ("overlay", 240),
    ("toast-enter", 340),
    ("activity-exit", 500),
    ("activity-show", 750),
];

/// Semantic file-reference badge & chip tone (`packages/brand/src/file-reference.ts:L41`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FileReferenceTone {
    Source,
    Script,
    Markup,
    Style,
    Data,
    Document,
    Media,
    #[default]
    Default,
}

impl FileReferenceTone {
    /// Returns the canonical lowercase tone identifier (`"source"`, `"script"`, ..., `"default"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Script => "script",
            Self::Markup => "markup",
            Self::Style => "style",
            Self::Data => "data",
            Self::Document => "document",
            Self::Media => "media",
            Self::Default => "default",
        }
    }

    /// Returns `(foreground_hex, soft_bg_rgba)` for `scheme` (`tokens.css:L215-L230`, `tokens-light.css:L92-L107`).
    pub const fn colors(self, scheme: OpenBotColorScheme) -> (&'static str, &'static str) {
        match (self, scheme) {
            (Self::Source, OpenBotColorScheme::Dark) => ("#74b9ff", "rgba(116, 185, 255, 0.14)"),
            (Self::Source, OpenBotColorScheme::Light) => ("#1a66c4", "rgba(31, 111, 209, 0.12)"),
            (Self::Script, OpenBotColorScheme::Dark) => ("#ff9412", "rgba(255, 148, 18, 0.12)"),
            (Self::Script, OpenBotColorScheme::Light) => ("#9c5200", "rgba(240, 138, 0, 0.14)"),
            (Self::Markup, OpenBotColorScheme::Dark) => ("#f0a06a", "rgba(240, 160, 106, 0.14)"),
            (Self::Markup, OpenBotColorScheme::Light) => ("#a8521a", "rgba(181, 90, 31, 0.12)"),
            (Self::Style, OpenBotColorScheme::Dark) => ("#6bc7d9", "rgba(107, 199, 217, 0.14)"),
            (Self::Style, OpenBotColorScheme::Light) => ("#127589", "rgba(22, 132, 155, 0.12)"),
            (Self::Data, OpenBotColorScheme::Dark) => ("#31cf76", "rgba(49, 207, 118, 0.12)"),
            (Self::Data, OpenBotColorScheme::Light) => ("#187a41", "rgba(31, 168, 90, 0.12)"),
            (Self::Document, OpenBotColorScheme::Dark) => ("#fd2f3b", "rgba(253, 47, 59, 0.16)"),
            (Self::Document, OpenBotColorScheme::Light) => ("#c0141d", "rgba(232, 34, 45, 0.12)"),
            (Self::Media, OpenBotColorScheme::Dark) => ("#d98ac9", "rgba(217, 138, 201, 0.14)"),
            (Self::Media, OpenBotColorScheme::Light) => ("#a3478f", "rgba(163, 71, 143, 0.12)"),
            (Self::Default, OpenBotColorScheme::Dark) => ("#42a0ff", "rgba(0, 124, 247, 0.14)"),
            (Self::Default, OpenBotColorScheme::Light) => ("#0064c8", "rgba(0, 124, 247, 0.12)"),
        }
    }
}

impl fmt::Display for FileReferenceTone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for FileReferenceTone {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<&str> for FileReferenceTone {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<str> for FileReferenceTone {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

fn file_reference_extension(name: &str) -> String {
    name.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

/// Resolves the `FileReferenceTone` for a file name (`packages/brand/src/file-reference.ts:L60-L70`).
pub fn file_reference_tone(name: &str) -> FileReferenceTone {
    match file_reference_extension(name).as_str() {
        "c" | "cc" | "cpp" | "md" | "py" | "ts" | "tsx" | "doc" | "docx" | "odt" | "rtf" => {
            FileReferenceTone::Source
        }
        "js" | "jsx" | "json" | "7z" | "gz" | "rar" | "tar" | "zip" => FileReferenceTone::Script,
        "html" | "java" | "rs" | "swift" | "odp" | "ppt" | "pptx" => FileReferenceTone::Markup,
        "css" | "go" | "sql" => FileReferenceTone::Style,
        "vue" | "csv" | "ods" | "xls" | "xlsx" => FileReferenceTone::Data,
        "rb" | "pdf" => FileReferenceTone::Document,
        "avif" | "gif" | "heic" | "jpeg" | "jpg" | "png" | "svg" | "webp" | "avi" | "flac"
        | "m4a" | "mkv" | "mov" | "mp3" | "mp4" | "ogg" | "wav" | "webm" => {
            FileReferenceTone::Media
        }
        _ => FileReferenceTone::Default,
    }
}

/// Returns up to 4 uppercase characters for known badged file extensions (`file-reference.ts:L55-L58`).
pub fn file_reference_badge(name: &str) -> Option<String> {
    let ext = file_reference_extension(name);
    let is_badged = matches!(
        ext.as_str(),
        "c" | "cc"
            | "cpp"
            | "cs"
            | "css"
            | "go"
            | "html"
            | "java"
            | "js"
            | "jsx"
            | "json"
            | "md"
            | "php"
            | "py"
            | "rb"
            | "rs"
            | "sql"
            | "swift"
            | "ts"
            | "tsx"
            | "vue"
            | "doc"
            | "docx"
            | "odt"
            | "pdf"
            | "rtf"
            | "avif"
            | "gif"
            | "heic"
            | "jpeg"
            | "jpg"
            | "png"
            | "svg"
            | "webp"
            | "7z"
            | "gz"
            | "rar"
            | "tar"
            | "zip"
            | "csv"
            | "ods"
            | "xls"
            | "xlsx"
            | "odp"
            | "ppt"
            | "pptx"
            | "avi"
            | "flac"
            | "m4a"
            | "mkv"
            | "mov"
            | "mp3"
            | "mp4"
            | "ogg"
            | "wav"
            | "webm"
    );
    if !is_badged {
        return None;
    }
    Some(ext.chars().take(4).collect::<String>().to_ascii_uppercase())
}

fn decode_uri_component_strict(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            if idx + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[idx + 1..idx + 3]).ok()?;
            let decoded = u8::from_str_radix(hex, 16).ok()?;
            out.push(decoded);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Extracts and URI-decodes the display basename of a file reference (`file-reference.ts:L72-L79`).
pub fn file_reference_name(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let raw_last = normalized.rsplit('/').next().unwrap_or("");
    let name = if raw_last.is_empty() {
        "file"
    } else {
        raw_last
    };
    decode_uri_component_strict(name).unwrap_or_else(|| name.to_string())
}

/// Checks whether `value` is a recognized OpenBot inline file reference (`file-reference.ts:L81-L88`).
pub fn is_file_reference(value: &str) -> bool {
    if value.is_empty()
        || value.bytes().any(|b| matches!(b, 0 | b'\r' | b'\n'))
        || value.ends_with('/')
        || value.ends_with('\\')
    {
        return false;
    }
    let name = file_reference_name(value);
    if name.eq_ignore_ascii_case("Dockerfile")
        || name.eq_ignore_ascii_case("Makefile")
        || name.eq_ignore_ascii_case(".gitignore")
    {
        return true;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "avif"
            | "bash"
            | "c"
            | "conf"
            | "cpp"
            | "cs"
            | "css"
            | "csv"
            | "env"
            | "fish"
            | "gif"
            | "go"
            | "h"
            | "heic"
            | "hpp"
            | "htm"
            | "html"
            | "ini"
            | "java"
            | "jpeg"
            | "jpg"
            | "js"
            | "jsx"
            | "json"
            | "kt"
            | "kts"
            | "log"
            | "markdown"
            | "md"
            | "pdf"
            | "php"
            | "png"
            | "ps1"
            | "py"
            | "rb"
            | "rs"
            | "scala"
            | "sh"
            | "sql"
            | "svg"
            | "swift"
            | "toml"
            | "ts"
            | "tsx"
            | "txt"
            | "webp"
            | "xml"
            | "yml"
            | "yaml"
            | "zsh"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_card_border_provides_crisp_light_contrast_and_preserves_hairline_tokens() {
        let dark = OpenBotPalette::for_scheme(OpenBotColorScheme::Dark);
        let light = OpenBotPalette::for_scheme(OpenBotColorScheme::Light);

        assert_eq!(dark.token_count(), 58);
        assert_eq!(light.token_count(), 58);
        assert_eq!(dark.border_hairline(), gpui::rgb(0x262626));
        assert_eq!(light.border_hairline(), gpui::rgb(0xE8E8EB));
        assert_eq!(dark.frame_card_border(), gpui::rgb(0x262626));
        assert_eq!(light.frame_card_border(), gpui::rgb(0xDCDCE0));
        assert_ne!(light.frame_card_border(), light.bg_frame());
    }
}
