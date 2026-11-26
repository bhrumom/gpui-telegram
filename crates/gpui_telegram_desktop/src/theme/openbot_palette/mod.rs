//! OpenBot semantic theme engine (`OB-F05`):
//! - `tokens`: 58 `--openbot-*` tokens, CSS Color 4 premultiplied mixing, OKLab `readable_chat_background`
//! - `watcher`: `<id>.json` theme validator, CSS-injection guard, `AppearancePreferenceStore`, `ThemeDirectoryWatcher`
//! - `transitions`: `sweepTheme` (`1200ms`) & `beam` (`1400ms`) shaders and GPUI overlays
//! - `backdrops`: 6 procedural pixel-art `ChatBackdrop` scenes, `CALM_CENTER = 0.55`, and `CustomBackdropModel`

pub mod backdrops;
pub mod tokens;
pub mod transitions;
pub mod watcher;

pub use backdrops::*;
pub use tokens::*;
pub use transitions::*;
pub use watcher::*;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThemeEvalRequest {
    pub mode: Option<String>,
    pub theme_json: Option<PathBuf>,
    pub themes_dir: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub backdrop: Option<String>,
    pub chat_color: Option<String>,
    pub reduced_motion: bool,
    pub persist: bool,
    pub viewport_width_px: Option<u32>,
    pub viewport_height_px: Option<u32>,
    pub elapsed_sec: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeChatBackgroundEval {
    pub light: String,
    pub dark: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeBackdropEval {
    pub scene: String,
    pub canonical_scene: String,
    pub base_hex: String,
    pub cell_px: u32,
    pub calm_center_alpha: f64,
    pub animated: bool,
    pub time_sec: f64,
    pub draw_cells: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeTransitionEval {
    pub kind: String,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeEvalReport {
    pub mode: String,
    pub palette_id: String,
    pub token_count: usize,
    pub tokens: BTreeMap<String, String>,
    pub chat_background: ThemeChatBackgroundEval,
    pub backdrop: ThemeBackdropEval,
    pub transition: ThemeTransitionEval,
    pub loaded_palettes: Vec<AppThemePalette>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persisted_path: Option<PathBuf>,
}

/// Evaluates an OpenBot theme, chat background, backdrop, and transition specification (`openbot theme-eval`).
pub fn evaluate_theme_spec(req: &ThemeEvalRequest) -> Result<ThemeEvalReport, ThemePaletteError> {
    let explicit_scheme = match req.mode.as_deref() {
        Some(raw) => {
            let parsed = OpenBotColorScheme::parse(raw.trim())
                .ok_or_else(|| ThemePaletteError::InvalidAppearance(raw.to_string()))?;
            Some(parsed)
        }
        None => None,
    };

    let resolved_themes_dir = req.themes_dir.clone().or_else(|| {
        req.data_dir
            .as_ref()
            .map(|d| d.join(THEME_PALETTE_DIRECTORY))
    });
    let loaded_palettes = resolved_themes_dir
        .as_deref()
        .map(read_theme_palettes_dir)
        .unwrap_or_default();

    let resolved_data_dir = req.data_dir.clone().or_else(|| {
        req.themes_dir
            .as_ref()
            .and_then(|dir| dir.parent().map(Path::to_path_buf))
    });

    let palette = if let Some(ref json_path) = req.theme_json {
        OpenBotPalette::from_theme_json_path(json_path)?
    } else if let Some(scheme) = explicit_scheme {
        OpenBotPalette::for_scheme(scheme)
    } else if let Some(ref data_dir) = req.data_dir {
        let pref_path = data_dir.join(APPEARANCE_PREFERENCE_FILE_NAME);
        let t_dir = resolved_themes_dir
            .clone()
            .unwrap_or_else(|| data_dir.join(THEME_PALETTE_DIRECTORY));
        let mut store = AppearancePreferenceStore::new(pref_path, t_dir);
        store.load();
        store.effective_palette()
    } else if let Some(first_custom) = loaded_palettes.first() {
        OpenBotPalette::from_custom_palette(first_custom.clone())
    } else {
        OpenBotPalette::dark()
    };

    let raw_chat_color = req
        .chat_color
        .as_deref()
        .map(|raw| find_chat_color_preset(raw).map(|p| p.hex).unwrap_or(raw))
        .unwrap_or(CHAT_COLOR_PRESETS[0].hex);
    let chat_background = ThemeChatBackgroundEval {
        light: readable_chat_background(raw_chat_color, OpenBotColorScheme::Light)?,
        dark: readable_chat_background(raw_chat_color, OpenBotColorScheme::Dark)?,
    };

    let (scene_enum, requested_scene) = match req.backdrop.as_deref() {
        Some(raw) => ChatBackdropScene::parse_with_alias(raw)
            .ok_or_else(|| ThemePaletteError::UnknownBackdropScene(raw.to_string()))?,
        None => (ChatBackdropScene::Pond, "pond"),
    };
    let raster = rasterize_chat_backdrop(
        scene_enum,
        palette.scheme,
        req.viewport_width_px.unwrap_or(960),
        req.viewport_height_px.unwrap_or(640),
        req.elapsed_sec.unwrap_or(0.0),
        req.reduced_motion,
    );
    let backdrop = ThemeBackdropEval {
        scene: requested_scene.to_string(),
        canonical_scene: scene_enum.canonical_str().to_string(),
        base_hex: raster.base_hex.to_string(),
        cell_px: raster.cell_px,
        calm_center_alpha: raster.calm_center_alpha,
        animated: raster.animated,
        time_sec: raster.time_sec,
        draw_cells: raster.draw_cells,
    };

    let opposite_scheme = match palette.scheme {
        OpenBotColorScheme::Dark => OpenBotColorScheme::Light,
        OpenBotColorScheme::Light => OpenBotColorScheme::Dark,
    };
    let transition_spec =
        ThemeTransitionSpec::sweep_theme(opposite_scheme, palette.scheme, req.reduced_motion);
    let transition = ThemeTransitionEval {
        kind: transition_spec.kind.as_str().to_string(),
        duration_ms: transition_spec.duration_ms,
    };

    let mut persisted_path = None;
    if req.persist {
        if let Some(ref base_dir) = resolved_data_dir {
            let pref_file = base_dir.join(APPEARANCE_PREFERENCE_FILE_NAME);
            let t_dir = resolved_themes_dir
                .clone()
                .unwrap_or_else(|| base_dir.join(THEME_PALETTE_DIRECTORY));
            let mut store = AppearancePreferenceStore::new(pref_file.clone(), t_dir);
            store.load();
            let app_mode = match palette.scheme {
                OpenBotColorScheme::Dark => AppAppearanceMode::Dark,
                OpenBotColorScheme::Light => AppAppearanceMode::Light,
            };
            let custom_id = palette.custom_source.as_ref().map(|src| src.id.clone());
            let patch = AppearancePreferencePatch {
                appearance: Some(app_mode),
                palette: Some(custom_id),
                chat_background_light: req
                    .chat_color
                    .is_some()
                    .then(|| Some(chat_background.light.clone())),
                chat_background_dark: req
                    .chat_color
                    .is_some()
                    .then(|| Some(chat_background.dark.clone())),
                ..AppearancePreferencePatch::default()
            };
            store.update(patch)?;
            persisted_path = Some(pref_file);
        }
    }

    Ok(ThemeEvalReport {
        mode: palette.scheme.as_str().to_string(),
        palette_id: palette.palette_id,
        token_count: palette.tokens.len(),
        tokens: palette.tokens,
        chat_background,
        backdrop,
        transition,
        loaded_palettes,
        persisted_path,
    })
}

struct ScopedProbeDir {
    path: PathBuf,
}

impl ScopedProbeDir {
    fn create(base_dir: Option<&Path>) -> Result<Self, ThemePaletteError> {
        static PROBE_SEQ: AtomicU64 = AtomicU64::new(1);
        let seq = PROBE_SEQ.fetch_add(1, Ordering::Relaxed);
        let parent = base_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        let path = parent.join(format!("openbot-theme-probe-{}-{seq}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for ScopedProbeDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeHotReloadProbeReport {
    pub watcher_triggered: bool,
    pub reload_count: u64,
    pub palette_id: String,
    pub active_canvas: String,
    pub rewritten_canvas: String,
    pub refused_fallback_canvas: String,
    pub restored_canvas: String,
    pub css_injection_refused: bool,
}

fn wait_for_watcher_reload(
    watcher: &ThemeDirectoryWatcher,
    min_count: u64,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if watcher.reload_count() >= min_count {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    watcher.reload_count() >= min_count
}

fn publish_probe_palette_atomic(
    themes_dir: &Path,
    id: &str,
    json_body: &str,
) -> Result<(), ThemePaletteError> {
    fs::create_dir_all(themes_dir)?;
    let tmp_path = themes_dir.join(format!("{id}.json.tmp"));
    let dst_path = themes_dir.join(format!("{id}.json"));
    fs::write(&tmp_path, json_body.as_bytes())?;
    fs::rename(&tmp_path, &dst_path)?;
    Ok(())
}

/// Runs the live `theme-hot-reload` probe (`scripts/theme-palette-e2e.ts`, `TEST_INFRA.md:L2005`).
pub fn probe_theme_hot_reload(
    base_dir: Option<&Path>,
) -> Result<ThemeHotReloadProbeReport, ThemePaletteError> {
    const PALETTE_ID: &str = "e2e-omarchy";
    const TOKYO_NIGHT_JSON: &str = r##"{
  "name": "E2E Omarchy",
  "appearance": "dark",
  "colors": {
    "canvas": "#1a1b26", "frame": "#13141c", "sidebar": "#13141c", "surface": "#24283b",
    "text": "#a9b1d6", "textMuted": "#565f89", "border": "#414868", "accent": "#7aa2f7",
    "accentForeground": "#1a1b26", "success": "#9ece6a", "warning": "#e0af68", "danger": "#f7768e"
  }
}
"##;
    const CATPPUCCIN_LATTE_JSON: &str = r##"{
  "name": "E2E Omarchy",
  "appearance": "light",
  "colors": {
    "canvas": "#eff1f5", "frame": "#e6e9ef", "sidebar": "#e6e9ef", "surface": "#ccd0da",
    "text": "#4c4f69", "textMuted": "#8c8fa1", "border": "#bcc0cc", "accent": "#1e66f5",
    "accentForeground": "#eff1f5", "success": "#40a02b", "warning": "#df8e1d", "danger": "#d20f39"
  }
}
"##;
    const INJECTED_JSON: &str = r##"{
  "name": "E2E Omarchy",
  "appearance": "dark",
  "colors": {
    "canvas": "#1a1b26", "frame": "#13141c", "sidebar": "#13141c", "surface": "#24283b",
    "text": "#a9b1d6", "textMuted": "#565f89", "border": "#414868",
    "accent": "red; background: url(https://example.com)",
    "accentForeground": "#1a1b26", "success": "#9ece6a", "warning": "#e0af68", "danger": "#f7768e"
  }
}
"##;

    let scratch = ScopedProbeDir::create(base_dir)?;
    let store = AppearancePreferenceStore::for_data_dir(&scratch.path);
    let themes_dir = store.themes_dir().to_path_buf();
    let watcher = ThemeDirectoryWatcher::start_with_store(store, |_palettes, _effective| {})
        .map_err(ThemePaletteError::Io)?;

    {
        let store_handle = watcher.preference_store();
        let mut guard = store_handle.write();
        guard.set_appearance(AppAppearanceMode::Dark)?;
        guard.set_palette(Some(PALETTE_ID.to_string()))?;
    }

    publish_probe_palette_atomic(&themes_dir, PALETTE_ID, TOKYO_NIGHT_JSON)?;
    let triggered_1 = wait_for_watcher_reload(&watcher, 1, Duration::from_millis(1500));
    if !triggered_1 {
        let _ = watcher.rescan_now();
    }
    let chosen = watcher.effective_palette();
    let active_canvas = chosen
        .token("--openbot-bg-canvas")
        .unwrap_or_default()
        .to_string();

    let prev = watcher.reload_count();
    publish_probe_palette_atomic(&themes_dir, PALETTE_ID, CATPPUCCIN_LATTE_JSON)?;
    if !wait_for_watcher_reload(&watcher, prev + 1, Duration::from_millis(1500)) {
        let _ = watcher.rescan_now();
    }
    let rewritten_canvas = watcher
        .effective_palette()
        .token("--openbot-bg-canvas")
        .unwrap_or_default()
        .to_string();

    let prev = watcher.reload_count();
    publish_probe_palette_atomic(&themes_dir, PALETTE_ID, INJECTED_JSON)?;
    if !wait_for_watcher_reload(&watcher, prev + 1, Duration::from_millis(1500)) {
        let _ = watcher.rescan_now();
    }
    let css_injection_refused = watcher.palettes().is_empty();
    let refused_fallback_canvas = watcher
        .effective_palette()
        .token("--openbot-bg-canvas")
        .unwrap_or_default()
        .to_string();

    let prev = watcher.reload_count();
    publish_probe_palette_atomic(&themes_dir, PALETTE_ID, TOKYO_NIGHT_JSON)?;
    if !wait_for_watcher_reload(&watcher, prev + 1, Duration::from_millis(1500)) {
        let _ = watcher.rescan_now();
    }
    let restored_canvas = watcher
        .effective_palette()
        .token("--openbot-bg-canvas")
        .unwrap_or_default()
        .to_string();

    let mut final_palette_id = PALETTE_ID.to_string();
    let mut final_active_canvas = active_canvas;
    if let Some(caller_dir) = base_dir {
        let caller_themes = caller_dir.join(THEME_PALETTE_DIRECTORY);
        if let Ok(entries) = fs::read_dir(&caller_themes) {
            let mut caller_files: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                })
                .collect();
            caller_files.sort();
            for src_file in caller_files {
                let Some(fname) = src_file.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let Some(stem_id) = palette_id_from_file_name(fname) else {
                    continue;
                };
                let Ok(raw_body) = fs::read_to_string(&src_file) else {
                    continue;
                };
                let prev_cnt = watcher.reload_count();
                publish_probe_palette_atomic(&themes_dir, stem_id, &raw_body)?;
                if !wait_for_watcher_reload(&watcher, prev_cnt + 1, Duration::from_millis(1500)) {
                    let _ = watcher.rescan_now();
                }
                if watcher.palettes().iter().any(|p| p.id == stem_id) {
                    {
                        let store_handle = watcher.preference_store();
                        let mut guard = store_handle.write();
                        guard.set_palette(Some(stem_id.to_string()))?;
                    }
                    final_palette_id = stem_id.to_string();
                    final_active_canvas = watcher
                        .effective_palette()
                        .token("--openbot-bg-canvas")
                        .unwrap_or_default()
                        .to_string();
                    break;
                }
            }
        }
    }

    let reload_count = watcher.reload_count();
    watcher.stop();

    Ok(ThemeHotReloadProbeReport {
        watcher_triggered: triggered_1 && reload_count > 0,
        reload_count,
        palette_id: final_palette_id,
        active_canvas: final_active_canvas,
        rewritten_canvas,
        refused_fallback_canvas,
        restored_canvas,
        css_injection_refused,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeSweepReducedMotionProbeReport {
    pub reduced_motion: bool,
    pub transition_kind: String,
    pub sweep_duration_ms: u32,
    pub beam_duration_ms: u32,
    pub down: bool,
    pub backdrop_animated: bool,
    pub backdrop_time_sec: f64,
    pub calm_center_alpha: f64,
}

/// Evaluates `sweepTheme` (`1200ms -> 0ms`), `beam` (`1400ms -> 0ms`), and `ChatBackdrop` (`STILL_TIME = 42s`) under `reduced_motion`.
pub fn probe_theme_sweep_reduced_motion(
    reduced_motion: bool,
) -> ThemeSweepReducedMotionProbeReport {
    let sweep = ThemeTransitionSpec::sweep_theme(
        OpenBotColorScheme::Dark,
        OpenBotColorScheme::Light,
        reduced_motion,
    );
    let beam = ThemeTransitionSpec::beam(
        gpui::point(gpui::px(32.0), gpui::px(120.0)),
        true,
        reduced_motion,
    );
    let raster = rasterize_chat_backdrop(
        ChatBackdropScene::Pond,
        OpenBotColorScheme::Dark,
        960,
        640,
        0.0,
        reduced_motion,
    );
    ThemeSweepReducedMotionProbeReport {
        reduced_motion,
        transition_kind: sweep.kind.as_str().to_string(),
        sweep_duration_ms: sweep.duration_ms,
        beam_duration_ms: beam.duration_ms,
        down: sweep.down,
        backdrop_animated: raster.animated,
        backdrop_time_sec: raster.time_sec,
        calm_center_alpha: raster.calm_center_alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_theme_spec_matches_test_infra_schema_and_aliases() {
        let report = evaluate_theme_spec(&ThemeEvalRequest::default())
            .expect("default dark theme evaluation succeeds");
        assert_eq!(report.mode, "dark");
        assert_eq!(report.palette_id, "default-dark");
        assert_eq!(report.token_count, 58);
        assert_eq!(report.token_count, report.tokens.len());
        assert_eq!(
            report.tokens.get("--openbot-bg-frame").map(String::as_str),
            Some("#0d0d0d")
        );
        assert_eq!(
            report.tokens.get("--openbot-bg-canvas").map(String::as_str),
            Some("#1a1a1a")
        );
        assert_eq!(
            report.tokens.get("--openbot-accent").map(String::as_str),
            Some("#007cf7")
        );
        assert_eq!(report.chat_background.light, "#fceae8");
        assert_eq!(report.chat_background.dark, "#442321");
        assert_eq!(report.backdrop.scene, "pond");
        assert_eq!(report.backdrop.canonical_scene, "pond");
        assert_eq!(report.backdrop.base_hex, "#0f2426");
        assert_eq!(report.backdrop.cell_px, 6);
        assert!((report.backdrop.calm_center_alpha - 0.55).abs() < 1e-12);
        assert!(report.backdrop.animated);
        assert_eq!(report.backdrop.time_sec, 0.0);
        assert!(report.backdrop.draw_cells > 0);
        assert_eq!(report.transition.kind, "sweep_theme");
        assert_eq!(report.transition.duration_ms, 1200);
        assert!(report.loaded_palettes.is_empty());

        let alias_report = evaluate_theme_spec(&ThemeEvalRequest {
            mode: Some("light".to_string()),
            backdrop: Some("forest".to_string()),
            reduced_motion: true,
            ..ThemeEvalRequest::default()
        })
        .expect("forest alias + reduced motion succeeds");
        assert_eq!(alias_report.mode, "light");
        assert_eq!(alias_report.palette_id, "default-light");
        assert_eq!(alias_report.backdrop.scene, "forest");
        assert_eq!(alias_report.backdrop.canonical_scene, "jellyfish");
        assert_eq!(alias_report.backdrop.base_hex, "#ebf3fa");
        assert!(!alias_report.backdrop.animated);
        assert_eq!(alias_report.backdrop.time_sec, 42.0);
        assert_eq!(alias_report.transition.kind, "instant");
        assert_eq!(alias_report.transition.duration_ms, 0);
    }

    #[test]
    fn persist_flag_and_hot_reload_and_reduced_motion_probes_work() {
        let dir = tempfile::tempdir().expect("tempdir");
        let eval = evaluate_theme_spec(&ThemeEvalRequest {
            mode: Some("light".to_string()),
            data_dir: Some(dir.path().to_path_buf()),
            chat_color: Some("#f75d59".to_string()),
            persist: true,
            ..ThemeEvalRequest::default()
        })
        .expect("persist evaluation succeeds");
        let persisted = eval.persisted_path.expect("persisted path returned");
        assert!(persisted.is_file());

        let hot = probe_theme_hot_reload(Some(dir.path())).expect("hot reload probe succeeds");
        assert!(hot.watcher_triggered);
        assert_eq!(hot.active_canvas, "#1a1b26");
        assert_eq!(hot.rewritten_canvas, "#eff1f5");
        assert!(hot.css_injection_refused);
        assert_eq!(hot.refused_fallback_canvas, "#1a1a1a");
        assert_eq!(hot.restored_canvas, "#1a1b26");

        let reduced = probe_theme_sweep_reduced_motion(true);
        assert_eq!(reduced.transition_kind, "instant");
        assert_eq!(reduced.sweep_duration_ms, 0);
        assert_eq!(reduced.beam_duration_ms, 0);
        assert!(!reduced.backdrop_animated);
        assert_eq!(reduced.backdrop_time_sec, 42.0);
    }
}
