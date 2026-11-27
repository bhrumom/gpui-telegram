//! Module definitions for `crates/gpui_telegram_desktop/src/theme`

pub mod assets;
mod assets_embedded;
pub mod boxes;
pub mod chat_helpers;
pub mod dialogs;
pub mod history;
pub mod info;
pub mod intro;
pub mod palette;
mod palette_presets;
mod palette_schemes;
pub mod settings;
mod tdesktop_palette;
pub mod ui;
pub mod wallet;
pub mod window;

pub use assets::{IconName, TelegramAssetSource, tg_icon, tg_icon_path, tg_icon_rgba};
pub use palette::{
    AccentSwatch, PaletteTokens, TdesktopThemeMode, TdesktopThemePalette, ThemePalette,
    WallpaperPreset, ACCENT_SWATCHES, WALLPAPER_PRESETS,
};
pub mod api;
pub use api::*;
pub mod calls;
pub use calls::*;
pub mod editor;
pub use editor::*;
pub mod export;
pub use export::*;
pub mod iv;
pub use iv::*;
pub mod media;
pub use media::*;
pub mod passport;
pub use passport::*;
pub mod payments;
pub use payments::*;
pub mod statistics;
pub use statistics::*;

pub mod openbot_palette;
pub use openbot_palette::{
    AppThemePalette, CHAT_COLOR_PRESETS, ChatBackdropScene, ChatColorPreset,
    OPENBOT_BADGE_NEW_DARK, OPENBOT_BADGE_NEW_LIGHT, OPENBOT_FELT_AMBER, OPENBOT_FELT_CREAM,
    OPENBOT_FELT_PLUM, OPENBOT_FELT_RASPBERRY, OpenBotColorScheme, OpenBotPalette,
    ThemeDirectoryWatcher, ThemeEvalReport, ThemeEvalRequest, ThemeHotReloadProbeReport,
    ThemePaletteError, ThemeSweepReducedMotionProbeReport, evaluate_theme_spec,
    probe_theme_hot_reload, probe_theme_sweep_reduced_motion, readable_chat_background,
    readable_chat_background_for_dark, solid_bubble_takes_dark_text,
};
