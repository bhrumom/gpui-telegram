//! Custom JSON theme validator, CSS-injection guard, `AppearancePreferenceStore`,
//! and live `ThemeDirectoryWatcher` (`OB-F05`).
//!
//! 1:1 port of upstream OpenBot (`23183891`):
//! - `packages/contracts/src/app-appearance.ts`
//! - `src/main/theme-palette-files.ts`
//! - `src/main/appearance-service.ts`
//! - `src/main/preference-file.ts`
//!
//! Wraps `mahayana_host_runtime::watched_directory::{ChangeListener, WatchedDirectory}`
//! (`40ms` debounce) with zero direct `notify` or `regex` imports.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mahayana_host_runtime::watched_directory::{ChangeListener, WatchedDirectory};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use super::tokens::{
    is_valid_app_font_family, AppThemePalette, OpenBotColorScheme, OpenBotPalette,
    OpenBotThemeColors, ThemePaletteError,
};

/// Default theme palette subdirectory name (`"themes"`).
pub const THEME_PALETTE_DIRECTORY: &str = "themes";
/// Canonical persisted appearance preference file name (`appearance-service.ts:L76`).
pub const APPEARANCE_PREFERENCE_FILE_NAME: &str = "openbot-appearance-preference-v1.json";
/// Canonical persisted interface zoom preference file name (`app-interface-size.ts`).
pub const INTERFACE_SIZE_PREFERENCE_FILE_NAME: &str = "openbot-interface-size-preference-v1.json";
/// Hard byte cap per `.json` theme file (`64 KiB`, `theme-palette-files.ts:L16`).
pub const MAX_PALETTE_FILE_BYTES: usize = 65_536;
/// Hard file count cap per themes directory (`32`, `theme-palette-files.ts:L18`).
pub const MAX_PALETTE_FILES: usize = 32;
/// OS file-watcher debounce interval (`40ms`).
pub const THEME_WATCHER_DEBOUNCE_MS: u64 = 40;

/// Validates `APP_THEME_PALETTE_ID_PATTERN = /^[a-z0-9][a-z0-9-]{0,63}$/u` (`app-appearance.ts:L41`).
pub fn is_valid_palette_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    if !matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9') {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// Extracts a valid palette ID from a `<id>.json` filename, skipping `.tmp` and non-matching files.
pub fn palette_id_from_file_name(file_name: &str) -> Option<&str> {
    let stem = file_name.strip_suffix(".json")?;
    is_valid_palette_id(stem).then_some(stem)
}

/// Validates theme `name`: trimmed, `1..=64` UTF-16 code units, zero Unicode control chars (`theme-palette-files.ts:L23-L25`).
pub fn is_valid_palette_name(name: &str) -> bool {
    if name.trim() != name {
        return false;
    }
    let u16_len: usize = name.chars().map(char::len_utf16).sum();
    if !(1..=64).contains(&u16_len) {
        return false;
    }
    !name.chars().any(char::is_control)
}

/// Detects CSS property/value injection payloads (`"red; background: url(...)"`, `var(...)`, etc.).
pub fn is_css_injection_like(value: &str) -> bool {
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, ';' | '{' | '}' | '<' | '>' | '\\' | '(' | ')'))
    {
        return true;
    }
    let lower = value.to_ascii_lowercase();
    lower.contains("url(")
        || lower.contains("var(")
        || lower.contains("expression(")
        || lower.contains("calc(")
        || lower.contains("/*")
        || lower.contains("*/")
        || lower.contains("@import")
        || lower.contains("background:")
}

/// Validates a custom palette color against `/^#[0-9a-fA-F]{6}(?:[0-9a-fA-F]{2})?$/u`.
///
/// Rejects CSS injection attempts with `ThemePaletteError::CssInjectionAttempt`.
pub fn validate_palette_hex_color(key: &str, value: &str) -> Result<String, ThemePaletteError> {
    let bytes = value.as_bytes();
    if (bytes.len() == 7 || bytes.len() == 9)
        && bytes[0] == b'#'
        && bytes[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Ok(value.to_string());
    }
    if is_css_injection_like(value) {
        return Err(ThemePaletteError::CssInjectionAttempt(format!(
            "CSS injection rejected for colors.{key}: {value}"
        )));
    }
    Err(ThemePaletteError::InvalidColor(format!(
        "invalid hex color for colors.{key} (expected #RRGGBB or #RRGGBBAA): {value}"
    )))
}

/// Validates a 6-digit `#RRGGBB` chat background color (`isChatBackgroundColor`).
pub fn validate_chat_background_hex(value: &str) -> Result<String, ThemePaletteError> {
    let bytes = value.as_bytes();
    if bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit) {
        return Ok(value.to_string());
    }
    if is_css_injection_like(value) {
        return Err(ThemePaletteError::CssInjectionAttempt(format!(
            "CSS injection rejected in chat background color: {value}"
        )));
    }
    Err(ThemePaletteError::InvalidColor(format!(
        "expected #RRGGBB chat background color, got: {value}"
    )))
}

/// Strictly validates a raw JSON string into an `AppThemePalette` (`theme-palette-files.ts:L22-L70`).
pub fn parse_theme_palette_json_str(
    id: &str,
    json: &str,
) -> Result<AppThemePalette, ThemePaletteError> {
    if !is_valid_palette_id(id) {
        return Err(ThemePaletteError::InvalidPaletteId(format!(
            "palette id '{id}' does not match ^[a-z0-9][a-z0-9-]{{0,63}}$"
        )));
    }
    if json.len() > MAX_PALETTE_FILE_BYTES {
        return Err(ThemePaletteError::OversizedFile(format!(
            "palette '{id}' exceeds {MAX_PALETTE_FILE_BYTES} bytes ({} bytes)",
            json.len()
        )));
    }

    let root: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| ThemePaletteError::InvalidJson(format!("palette '{id}': {e}")))?;
    let Some(obj) = root.as_object() else {
        return Err(ThemePaletteError::InvalidJson(format!(
            "palette '{id}' root must be a JSON object"
        )));
    };

    let Some(name_raw) = obj.get("name").and_then(serde_json::Value::as_str) else {
        return Err(ThemePaletteError::InvalidName(format!(
            "palette '{id}' is missing string field 'name'"
        )));
    };
    if !is_valid_palette_name(name_raw) {
        return Err(ThemePaletteError::InvalidName(format!(
            "palette '{id}' has invalid name (must be trimmed, 1..=64 UTF-16 units, control-free)"
        )));
    }

    let Some(appearance_raw) = obj.get("appearance").and_then(serde_json::Value::as_str) else {
        return Err(ThemePaletteError::InvalidAppearance(format!(
            "palette '{id}' is missing string field 'appearance'"
        )));
    };
    let Some(scheme) = OpenBotColorScheme::parse(appearance_raw) else {
        return Err(ThemePaletteError::InvalidAppearance(format!(
            "palette '{id}' appearance must be 'light' or 'dark', got '{appearance_raw}'"
        )));
    };

    let Some(colors_obj) = obj.get("colors").and_then(serde_json::Value::as_object) else {
        return Err(ThemePaletteError::InvalidColor(format!(
            "palette '{id}' is missing object field 'colors'"
        )));
    };

    let read_color = |key: &str| -> Result<String, ThemePaletteError> {
        let Some(val) = colors_obj.get(key).and_then(serde_json::Value::as_str) else {
            return Err(ThemePaletteError::InvalidColor(format!(
                "palette '{id}' is missing string field colors.{key}"
            )));
        };
        validate_palette_hex_color(key, val)
    };

    let colors = OpenBotThemeColors {
        canvas: read_color("canvas")?,
        frame: read_color("frame")?,
        sidebar: read_color("sidebar")?,
        surface: read_color("surface")?,
        text: read_color("text")?,
        text_muted: read_color("textMuted")?,
        border: read_color("border")?,
        accent: read_color("accent")?,
        accent_foreground: read_color("accentForeground")?,
        success: read_color("success")?,
        warning: read_color("warning")?,
        danger: read_color("danger")?,
    };

    Ok(AppThemePalette {
        id: id.to_string(),
        name: name_raw.to_string(),
        scheme,
        colors,
    })
}

/// Reads and strictly validates a single `<id>.json` theme palette file from disk.
pub fn read_theme_palette_file(path: &Path) -> Result<AppThemePalette, ThemePaletteError> {
    let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
        return Err(ThemePaletteError::InvalidPaletteId(format!(
            "non-UTF8 or missing filename: {}",
            path.display()
        )));
    };
    let Some(id) = palette_id_from_file_name(file_name) else {
        return Err(ThemePaletteError::InvalidPaletteId(format!(
            "invalid theme filename '{file_name}' (expected <id>.json matching ^[a-z0-9][a-z0-9-]{{0,63}}$)"
        )));
    };

    let meta = fs::metadata(path).map_err(|e| ThemePaletteError::Io(e.to_string()))?;
    if !meta.is_file() {
        return Err(ThemePaletteError::Io(format!(
            "theme path is not a regular file: {}",
            path.display()
        )));
    }
    if meta.len() > MAX_PALETTE_FILE_BYTES as u64 {
        return Err(ThemePaletteError::OversizedFile(format!(
            "{} is {} bytes (max {MAX_PALETTE_FILE_BYTES})",
            path.display(),
            meta.len()
        )));
    }

    let bytes = fs::read(path).map_err(|e| ThemePaletteError::Io(e.to_string()))?;
    if bytes.len() > MAX_PALETTE_FILE_BYTES {
        return Err(ThemePaletteError::OversizedFile(format!(
            "{} is {} bytes (max {MAX_PALETTE_FILE_BYTES})",
            path.display(),
            bytes.len()
        )));
    }
    let text = String::from_utf8(bytes).map_err(|e| {
        ThemePaletteError::InvalidJson(format!("invalid UTF-8 in {file_name}: {e}"))
    })?;

    parse_theme_palette_json_str(id, &text)
}

/// Scans `dir` for up to `MAX_PALETTE_FILES` (`32`) sorted `<id>.json` files,
/// gracefully skipping `.tmp`, oversized, malformed, or CSS-injected files (`theme-palette-files.ts:L72-L81`).
pub fn read_theme_palettes_dir(dir: &Path) -> Vec<AppThemePalette> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| palette_id_from_file_name(&name).map(str::to_string))
        .collect();
    ids.sort();
    ids.dedup();
    ids.truncate(MAX_PALETTE_FILES);

    let mut valid = Vec::with_capacity(ids.len());
    for id in ids {
        let candidate = dir.join(format!("{id}.json"));
        if let Ok(palette) = read_theme_palette_file(&candidate) {
            valid.push(palette);
        }
    }
    valid
}

/// User-facing appearance preference mode (`"system" | "light" | "dark"`, `app-appearance.ts:L4-L8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppAppearanceMode {
    System,
    Light,
    #[default]
    Dark,
}

impl AppAppearanceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub const fn resolve_scheme(self, system_scheme: OpenBotColorScheme) -> OpenBotColorScheme {
        match self {
            Self::Dark => OpenBotColorScheme::Dark,
            Self::Light => OpenBotColorScheme::Light,
            Self::System => system_scheme,
        }
    }
}

/// Persisted appearance preference record (`openbot-appearance-preference-v1.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearancePreference {
    pub version: u32,
    pub appearance: AppAppearanceMode,
    pub transparent_window: bool,
    pub palette: Option<String>,
    pub chat_background_light: Option<String>,
    pub chat_background_dark: Option<String>,
    pub interface_font: Option<String>,
    pub code_font: Option<String>,
    pub theme_chosen: bool,
}

impl Default for AppearancePreference {
    fn default() -> Self {
        Self {
            version: 1,
            appearance: AppAppearanceMode::Dark,
            transparent_window: false,
            palette: None,
            chat_background_light: None,
            chat_background_dark: None,
            interface_font: None,
            code_font: None,
            theme_chosen: false,
        }
    }
}

/// Partial update payload matching `SetAppAppearancePreferenceInput` (`app-appearance.ts:L63-L74`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppearancePreferencePatch {
    pub appearance: Option<AppAppearanceMode>,
    pub transparent_window: Option<bool>,
    pub palette: Option<Option<String>>,
    pub chat_background_light: Option<Option<String>>,
    pub chat_background_dark: Option<Option<String>>,
    pub interface_font: Option<Option<String>>,
    pub code_font: Option<Option<String>>,
}

fn write_json_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(first) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(&tmp, path).map_err(|_| first)
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

fn decode_stored_preference(raw_json: &str) -> AppearancePreference {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(raw_json) else {
        return AppearancePreference {
            theme_chosen: true,
            ..AppearancePreference::default()
        };
    };
    let Some(obj) = val.as_object() else {
        return AppearancePreference {
            theme_chosen: true,
            ..AppearancePreference::default()
        };
    };
    let version_ok = obj.get("version").and_then(serde_json::Value::as_u64) == Some(1);
    let appearance = obj
        .get("appearance")
        .and_then(serde_json::Value::as_str)
        .and_then(AppAppearanceMode::parse);
    let (true, Some(appearance)) = (version_ok, appearance) else {
        return AppearancePreference {
            theme_chosen: true,
            ..AppearancePreference::default()
        };
    };

    let palette = obj
        .get("palette")
        .and_then(serde_json::Value::as_str)
        .filter(|id| is_valid_palette_id(id))
        .map(str::to_string);
    let chat_background_light = obj
        .get("chatBackgroundLight")
        .and_then(serde_json::Value::as_str)
        .and_then(|c| validate_chat_background_hex(c).ok());
    let chat_background_dark = obj
        .get("chatBackgroundDark")
        .and_then(serde_json::Value::as_str)
        .and_then(|c| validate_chat_background_hex(c).ok());
    let interface_font = obj
        .get("interfaceFont")
        .and_then(serde_json::Value::as_str)
        .filter(|f| is_valid_app_font_family(f))
        .map(str::to_string);
    let code_font = obj
        .get("codeFont")
        .and_then(serde_json::Value::as_str)
        .filter(|f| is_valid_app_font_family(f))
        .map(str::to_string);
    let theme_chosen = obj.get("themeChosen").and_then(serde_json::Value::as_bool) != Some(false);
    let transparent_window = obj
        .get("transparentWindow")
        .and_then(serde_json::Value::as_bool)
        == Some(true);

    AppearancePreference {
        version: 1,
        appearance,
        transparent_window,
        palette,
        chat_background_light,
        chat_background_dark,
        interface_font,
        code_font,
        theme_chosen,
    }
}

/// Stateful OpenBot appearance & custom palette store (`src/main/appearance-service.ts`).
///
/// Preserves `stored.palette = Some("<id>")` when an active palette file becomes invalid or deleted,
/// while `effective_palette()` immediately reverts to the saved base `appearance` (`"dark"`/`"light"`)
/// and automatically restores the custom palette when a valid `<id>.json` returns.
#[derive(Debug, Clone)]
pub struct AppearancePreferenceStore {
    preference_path: PathBuf,
    themes_dir: PathBuf,
    supports_transparent_window: bool,
    system_scheme: OpenBotColorScheme,
    stored: AppearancePreference,
    palettes: Vec<AppThemePalette>,
}

impl AppearancePreferenceStore {
    /// Creates an `AppearancePreferenceStore` backed by `preference_path` and `themes_dir`.
    pub fn new(preference_path: impl Into<PathBuf>, themes_dir: impl Into<PathBuf>) -> Self {
        Self {
            preference_path: preference_path.into(),
            themes_dir: themes_dir.into(),
            supports_transparent_window: true,
            system_scheme: OpenBotColorScheme::Dark,
            stored: AppearancePreference::default(),
            palettes: Vec::new(),
        }
    }

    /// Creates an `AppearancePreferenceStore` inside `<data_dir>` using canonical filenames.
    pub fn for_data_dir(data_dir: impl AsRef<Path>) -> Self {
        let base = data_dir.as_ref();
        Self::new(
            base.join(APPEARANCE_PREFERENCE_FILE_NAME),
            base.join(THEME_PALETTE_DIRECTORY),
        )
    }

    pub fn with_transparent_window_support(mut self, supported: bool) -> Self {
        self.supports_transparent_window = supported;
        self
    }

    pub fn set_system_scheme(&mut self, scheme: OpenBotColorScheme) {
        self.system_scheme = scheme;
    }

    pub fn preference_path(&self) -> &Path {
        &self.preference_path
    }

    pub fn themes_dir(&self) -> &Path {
        &self.themes_dir
    }

    fn show_first_palette(&mut self) {
        if self.stored.theme_chosen {
            return;
        }
        if let Some(ref shown) = self.stored.palette {
            if self.palettes.iter().any(|p| &p.id == shown) {
                return;
            }
        }
        self.stored.palette = self.palettes.first().map(|p| p.id.clone());
    }

    /// Loads `openbot-appearance-preference-v1.json` and scans `themes_dir` (`appearance-service.ts:L112-L119`).
    pub fn load(&mut self) -> AppearancePreference {
        self.stored = match fs::read_to_string(&self.preference_path) {
            Ok(raw) => decode_stored_preference(&raw),
            Err(err) if err.kind() == io::ErrorKind::NotFound => AppearancePreference::default(),
            Err(_) => AppearancePreference {
                theme_chosen: true,
                ..AppearancePreference::default()
            },
        };
        self.palettes = read_theme_palettes_dir(&self.themes_dir);
        self.show_first_palette();
        self.effective_preference()
    }

    /// Rescans `themes_dir`, returning `true` if the discovered palette list changed.
    pub fn refresh_palettes(&mut self) -> bool {
        let next = read_theme_palettes_dir(&self.themes_dir);
        self.refresh_palettes_with(next)
    }

    /// Applies an already-scanned palette list, returning `true` if `self.palettes` changed.
    pub fn refresh_palettes_with(&mut self, next: Vec<AppThemePalette>) -> bool {
        if next == self.palettes {
            return false;
        }
        self.palettes = next;
        self.show_first_palette();
        true
    }

    /// Returns the raw stored preference (keeping `stored.palette` even if its JSON file is refused/missing).
    pub fn stored_preference(&self) -> &AppearancePreference {
        &self.stored
    }

    /// Returns the effective UI preference where `palette` is `None` if missing/invalid (`appearance-service.ts:L97-L110`).
    pub fn effective_preference(&self) -> AppearancePreference {
        let active_id = self
            .stored
            .palette
            .as_ref()
            .filter(|wanted| self.palettes.iter().any(|p| &p.id == *wanted))
            .cloned();
        AppearancePreference {
            version: 1,
            appearance: self.stored.appearance,
            transparent_window: self.supports_transparent_window && self.stored.transparent_window,
            palette: active_id,
            chat_background_light: self.stored.chat_background_light.clone(),
            chat_background_dark: self.stored.chat_background_dark.clone(),
            interface_font: self.stored.interface_font.clone(),
            code_font: self.stored.code_font.clone(),
            theme_chosen: self.stored.theme_chosen,
        }
    }

    /// Returns all currently valid custom palettes loaded from `themes_dir`.
    pub fn palettes(&self) -> &[AppThemePalette] {
        &self.palettes
    }

    /// Returns the currently active custom `AppThemePalette` if `stored.palette` is present in `self.palettes`.
    pub fn selected_custom_palette(&self) -> Option<&AppThemePalette> {
        let wanted = self.stored.palette.as_deref()?;
        self.palettes.iter().find(|p| p.id == wanted)
    }

    /// Resolves the live `OpenBotPalette`, falling back to base `appearance` when the selected custom
    /// palette is `None`, deleted, or refused by validation, and auto-recovering when valid JSON returns.
    pub fn effective_palette(&self) -> OpenBotPalette {
        if let Some(custom) = self.selected_custom_palette() {
            OpenBotPalette::from_custom_palette(custom.clone())
        } else {
            let scheme = self.stored.appearance.resolve_scheme(self.system_scheme);
            OpenBotPalette::for_scheme(scheme)
        }
    }

    /// Atomically persists `self.stored` to `openbot-appearance-preference-v1.json`.
    pub fn save_atomic(&self) -> Result<(), ThemePaletteError> {
        let mut json = serde_json::to_vec_pretty(&self.stored)?;
        json.push(b'\n');
        write_json_atomic(&self.preference_path, &json)?;
        Ok(())
    }

    /// Updates preference fields, marks `theme_chosen` when `appearance` or `palette` is touched, and saves atomically.
    pub fn update(
        &mut self,
        patch: AppearancePreferencePatch,
    ) -> Result<AppearancePreference, ThemePaletteError> {
        if let Some(ref maybe_id) = patch.palette {
            if let Some(id) = maybe_id {
                if !is_valid_palette_id(id) {
                    return Err(ThemePaletteError::InvalidPaletteId(id.clone()));
                }
            }
        }
        if let Some(Some(ref light)) = patch.chat_background_light {
            validate_chat_background_hex(light)?;
        }
        if let Some(Some(ref dark)) = patch.chat_background_dark {
            validate_chat_background_hex(dark)?;
        }
        if let Some(Some(ref font)) = patch.interface_font {
            if !is_valid_app_font_family(font) {
                return Err(ThemePaletteError::InvalidFontFamily(font.clone()));
            }
        }
        if let Some(Some(ref font)) = patch.code_font {
            if !is_valid_app_font_family(font) {
                return Err(ThemePaletteError::InvalidFontFamily(font.clone()));
            }
        }

        let theme_touched = patch.appearance.is_some() || patch.palette.is_some();
        if let Some(app) = patch.appearance {
            self.stored.appearance = app;
        }
        if let Some(tw) = patch.transparent_window {
            self.stored.transparent_window = tw;
        }
        if let Some(pal) = patch.palette {
            self.stored.palette = pal;
        }
        if let Some(c) = patch.chat_background_light {
            self.stored.chat_background_light = c;
        }
        if let Some(c) = patch.chat_background_dark {
            self.stored.chat_background_dark = c;
        }
        if let Some(f) = patch.interface_font {
            self.stored.interface_font = f;
        }
        if let Some(f) = patch.code_font {
            self.stored.code_font = f;
        }
        self.stored.version = 1;
        self.stored.theme_chosen = self.stored.theme_chosen || theme_touched;
        self.save_atomic()?;
        Ok(self.effective_preference())
    }

    pub fn set_appearance(
        &mut self,
        appearance: AppAppearanceMode,
    ) -> Result<AppearancePreference, ThemePaletteError> {
        self.update(AppearancePreferencePatch {
            appearance: Some(appearance),
            ..AppearancePreferencePatch::default()
        })
    }

    pub fn set_palette(
        &mut self,
        palette_id: Option<String>,
    ) -> Result<AppearancePreference, ThemePaletteError> {
        self.update(AppearancePreferencePatch {
            palette: Some(palette_id),
            ..AppearancePreferencePatch::default()
        })
    }
}

type PaletteListCallback = Arc<dyn Fn(Vec<AppThemePalette>) + Send + Sync + 'static>;
type StoreUpdateCallback =
    Arc<dyn Fn(Vec<AppThemePalette>, OpenBotPalette) + Send + Sync + 'static>;

struct WatcherSharedState {
    store: Arc<RwLock<AppearancePreferenceStore>>,
    subscribers: Mutex<Vec<PaletteListCallback>>,
    store_callback: Option<StoreUpdateCallback>,
    reload_count: AtomicU64,
}

impl WatcherSharedState {
    fn handle_directory_scan(&self, dir: &Path) -> Vec<AppThemePalette> {
        let next = read_theme_palettes_dir(dir);
        let (changed, effective) = {
            let mut guard = self.store.write();
            let changed = guard.refresh_palettes_with(next.clone());
            let effective = guard.effective_palette();
            (changed, effective)
        };
        if changed {
            self.reload_count.fetch_add(1, Ordering::AcqRel);
            if let Some(ref cb) = self.store_callback {
                cb(next.clone(), effective);
            }
            let subs = self.subscribers.lock().clone();
            for sub in subs {
                sub(next.clone());
            }
        }
        next
    }
}

/// Live OS directory watcher (`40ms` debounce) over `<themes_dir>/*.json`
/// backed by `mahayana_host_runtime::watched_directory::WatchedDirectory`.
#[derive(Clone)]
pub struct ThemeDirectoryWatcher {
    watched: WatchedDirectory,
    shared: Arc<WatcherSharedState>,
}

impl std::fmt::Debug for ThemeDirectoryWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThemeDirectoryWatcher")
            .field("dir", &self.watched.get_location())
            .field("reload_count", &self.reload_count())
            .finish()
    }
}

impl ThemeDirectoryWatcher {
    /// Starts watching `dir` (`40ms` debounce) and invokes `on_update` whenever valid `<id>.json` palettes change.
    pub fn start(
        dir: impl Into<PathBuf>,
        on_update: impl Fn(Vec<AppThemePalette>) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let themes_dir = dir.into();
        fs::create_dir_all(&themes_dir).map_err(|e| e.to_string())?;
        let pref_path = themes_dir
            .parent()
            .unwrap_or(themes_dir.as_path())
            .join(APPEARANCE_PREFERENCE_FILE_NAME);
        let mut store = AppearancePreferenceStore::new(pref_path, themes_dir.clone());
        store.load();

        let primary_cb: PaletteListCallback = Arc::new(on_update);
        let shared = Arc::new(WatcherSharedState {
            store: Arc::new(RwLock::new(store)),
            subscribers: Mutex::new(vec![primary_cb]),
            store_callback: None,
            reload_count: AtomicU64::new(0),
        });
        Self::attach_watcher(themes_dir, shared)
    }

    /// Starts watching `store.themes_dir()` (`40ms` debounce) with live `AppearancePreferenceStore` synchronization.
    pub fn start_with_store(
        mut store: AppearancePreferenceStore,
        on_update: impl Fn(Vec<AppThemePalette>, OpenBotPalette) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let themes_dir = store.themes_dir().to_path_buf();
        fs::create_dir_all(&themes_dir).map_err(|e| e.to_string())?;
        store.load();

        let shared = Arc::new(WatcherSharedState {
            store: Arc::new(RwLock::new(store)),
            subscribers: Mutex::new(Vec::new()),
            store_callback: Some(Arc::new(on_update)),
            reload_count: AtomicU64::new(0),
        });
        Self::attach_watcher(themes_dir, shared)
    }

    fn attach_watcher(
        themes_dir: PathBuf,
        shared: Arc<WatcherSharedState>,
    ) -> Result<Self, String> {
        let watched = WatchedDirectory::new(themes_dir.clone(), THEME_WATCHER_DEBOUNCE_MS);
        let shared_for_cb = Arc::clone(&shared);
        let dir_for_cb = themes_dir;
        let listener: ChangeListener = Arc::new(move || {
            let _ = shared_for_cb.handle_directory_scan(&dir_for_cb);
        });
        watched.set_on_change(Some(listener))?;
        Ok(Self { watched, shared })
    }

    /// Returns the watched themes directory path.
    pub fn dir(&self) -> &Path {
        self.watched.get_location()
    }

    /// Returns a snapshot of currently loaded custom palettes.
    pub fn palettes(&self) -> Vec<AppThemePalette> {
        self.shared.store.read().palettes().to_vec()
    }

    /// Returns the live resolved `OpenBotPalette` (custom palette or fallback base scheme).
    pub fn effective_palette(&self) -> OpenBotPalette {
        self.shared.store.read().effective_palette()
    }

    /// Returns the thread-safe `AppearancePreferenceStore` handle.
    pub fn preference_store(&self) -> Arc<RwLock<AppearancePreferenceStore>> {
        Arc::clone(&self.shared.store)
    }

    /// Number of distinct palette-set hot-reload transitions observed since watcher start.
    pub fn reload_count(&self) -> u64 {
        self.shared.reload_count.load(Ordering::Acquire)
    }

    /// Synchronously rescans `self.dir()` and triggers listeners if the palette list changed.
    pub fn rescan_now(&self) -> Vec<AppThemePalette> {
        self.shared.handle_directory_scan(self.dir())
    }

    /// Registers an additional listener notified whenever the valid palette list changes.
    pub fn subscribe(&self, listener: impl Fn(Vec<AppThemePalette>) + Send + Sync + 'static) {
        self.shared.subscribers.lock().push(Arc::new(listener));
    }

    /// Atomically writes `<id>.json` via a `.tmp` staging file inside `self.dir()`.
    pub fn write_palette_atomic(&self, id: &str, contents: &[u8]) -> Result<(), ThemePaletteError> {
        if !is_valid_palette_id(id) {
            return Err(ThemePaletteError::InvalidPaletteId(id.to_string()));
        }
        let target = self.dir().join(format!("{id}.json"));
        self.watched.write_file_atomic(target, contents)?;
        Ok(())
    }

    /// Underlying `mahayana_host_runtime::watched_directory::WatchedDirectory`.
    pub fn watched_directory(&self) -> &WatchedDirectory {
        &self.watched
    }

    /// Stops OS filesystem watching immediately.
    pub fn stop(&self) {
        self.watched.stop_watching();
    }
}
