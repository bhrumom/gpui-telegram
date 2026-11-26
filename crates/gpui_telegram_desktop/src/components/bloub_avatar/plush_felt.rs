//! 3D Tactile Felt `PlushBot` Renderer, 73 `.webp` Asset Store, Physically-Measured
//! sRGB<->Linear `tint_ramp`, 9-Slice `plus-lighter` Eye Pads, 4-Band Soft `lid_mask`,
//! 7 `PlushState`s, and 5 Body Transitions (`Flip`, `Pop`, `Warp`, `Dissolve`, `Cut`).
//!
//! 1:1 port of `@norbert_bodziony/bloub@0.7.0/plush` (`plush.js:L1-L1210`) and
//! `apps/mobile/src/features/agents/model/plush/{plush-felt,plush-draw,plush-assets,plush-frames}.ts`.

use std::collections::{HashMap, VecDeque};
use std::f64::consts::{PI, TAU};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use gpui::{ObjectFit, ParentElement, Styled, StyledImage, div, img, px};
use openbot_contracts::avatar::{
    AvatarHue, AvatarMood, BloubColorId, BloubExpression, BloubParametricState, BloubShape,
    EngineShapeId, PlushState, PlushTransition, bloub_avatar_profile, plush_mood_presentation,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::bot_engine::{BloubBotEngine, blink_lid_at, dot_pulse};
use super::eyefit::{
    ComposedEyeTransform, DecalageOffset, ExpressionDef, EyeSpec, GazeAngles, blink_scale,
    decalage_for, expression_def, fit_eyes_zero_clipping, project_eyes_3d,
    verify_eyes_zero_clipping,
};
use super::silhouettes::{
    DEMI_VIEWBOX, NOTIF_BLUE, RAYON, Silhouette, VIEW_BOX, engine_shape_radii, radius_at_angle,
    silhouette_svg_path,
};

pub const PLUSH_BODY_SCALE: f64 = 1.15;
pub const NOTIFY_COLOR: &str = NOTIF_BLUE;
pub const CLOSED_EYE_COLOR: &str = "#1c1a1d";
pub const CLOSED_EYE_BEND: f64 = 0.42;
pub const CLOSED_EYE_STROKE: f64 = 0.20;
pub const FACE_CHOICE_EYE_SCALE: f64 = 1.25;
pub const SMALL_AVATAR_EYE_SCALE_MAX: f64 = 1.30;
pub const BALL_FILL: f64 = 0.8;
pub const BLINK_CLOSED: f64 = 0.06;
pub const LID_FRINGE: f64 = 30.0 / 205.0;
pub const LID_SOFT: f64 = 0.1;
pub const LID_BANDS: usize = 4;
pub const PAD_BLEND_MODE: &str = "plus-lighter";
pub const EXPECTED_BUNDLED_WEBP_COUNT: usize = 73;

const GREY_REF: f64 = 0.55;
const TINT_GAMMA: f64 = 0.6;
const TINT_SHEEN: f64 = 0.08;
const PAD_BAR_RATIO: f64 = 0.5;
const SHAPE_MORPH_SEC: f64 = 0.45;
const POP_SHRINK_SEC: f64 = 0.225;
const POP_GROW_SEC: f64 = 0.45;
const EMERGE_SEC: f64 = 0.30;
const SHAKE_SEC: f64 = 0.80;
const SHAKE_YAW_DEG: f64 = 12.0;
const DISSOLVE_EDGE: f64 = 0.12;
const PIXEL_STEP: u32 = 32;
const MAX_TINT_CACHE_ENTRIES: usize = 64;
const MAX_FRAME_CACHE_ENTRIES: usize = 48;

/// Canonical 73 bundled `.webp` plush/felt relative paths under `assets/openbot_plush/`.
pub const BUNDLED_RELATIVE_WEBP_PATHS: [&str; EXPECTED_BUNDLED_WEBP_COUNT] = [
    "bloub/boule.webp",
    "bloub/cloe-corps.webp",
    "bloub/cloe-oeil0-x.webp",
    "bloub/cloe-oeil0-y.webp",
    "bloub/cloe-oeil1-x.webp",
    "bloub/cloe-oeil1-y.webp",
    "bloub/forme-capsule.webp",
    "bloub/forme-carre.webp",
    "bloub/forme-cercle.webp",
    "bloub/forme-fantome.webp",
    "bloub/forme-galet.webp",
    "bloub/forme-goutte.webp",
    "bloub/forme-hexagone.webp",
    "bloub/forme-losange.webp",
    "bloub/forme-meduse.webp",
    "bloub/forme-nuage.webp",
    "bloub/forme-pentagone.webp",
    "bloub/forme-poulpe.webp",
    "bloub/forme-squircle.webp",
    "bloub/forme-triangle.webp",
    "bloub/fred-corps.webp",
    "bloub/fred-oeil0-x.webp",
    "bloub/fred-oeil0-y.webp",
    "bloub/fred-oeil1-x.webp",
    "bloub/fred-oeil1-y.webp",
    "bloub/malu-corps.webp",
    "bloub/malu-oeil0-x.webp",
    "bloub/malu-oeil0-y.webp",
    "bloub/malu-oeil1-x.webp",
    "bloub/malu-oeil1-y.webp",
    "bloub/nexo-corps.webp",
    "bloub/nexo-oeil0-x.webp",
    "bloub/nexo-oeil0-y.webp",
    "bloub/nexo-oeil1-x.webp",
    "bloub/nexo-oeil1-y.webp",
    "bloub/vita-corps.webp",
    "bloub/vita-oeil0-x.webp",
    "bloub/vita-oeil0-y.webp",
    "bloub/vita-oeil1-x.webp",
    "bloub/vita-oeil1-y.webp",
    "bloub/yoshi-corps.webp",
    "bloub/yoshi-oeil0-x.webp",
    "bloub/yoshi-oeil0-y.webp",
    "bloub/yoshi-oeil1-x.webp",
    "bloub/yoshi-oeil1-y.webp",
    "ui/ball-lilac.webp",
    "ui/ball-mint.webp",
    "ui/ball-pink.webp",
    "ui/body-lilac.webp",
    "ui/eye-left.webp",
    "ui/eye-right.webp",
    "ui/felt-pill-amber.webp",
    "ui/felt-pill-lilac.webp",
    "ui/felt-x-ball.webp",
    "ui/fishing-still.webp",
    "ui/server/ambre.webp",
    "ui/server/bleu.webp",
    "ui/server/orange.webp",
    "ui/server/rose.webp",
    "ui/server/rouge.webp",
    "ui/server/turquoise.webp",
    "ui/server/vert.webp",
    "ui/server/violet.webp",
    "ui/server/shape/capsule.webp",
    "ui/server/shape/carre.webp",
    "ui/server/shape/cercle.webp",
    "ui/server/shape/galet.webp",
    "ui/server/shape/hexagone.webp",
    "ui/server/shape/losange.webp",
    "ui/server/shape/nuage.webp",
    "ui/server/shape/pentagone.webp",
    "ui/server/shape/squircle.webp",
    "ui/server/shape/triangle.webp",
];

struct BoundedCache<V> {
    order: VecDeque<String>,
    entries: HashMap<String, V>,
    limit: usize,
}

impl<V: Clone> BoundedCache<V> {
    fn new(limit: usize) -> Self {
        Self {
            order: VecDeque::with_capacity(limit),
            entries: HashMap::with_capacity(limit),
            limit: limit.max(1),
        }
    }

    fn get(&mut self, key: &str) -> Option<V> {
        let value = self.entries.get(key)?.clone();
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_owned());
        Some(value)
    }

    fn insert(&mut self, key: String, value: V) {
        if let Some(pos) = self.order.iter().position(|k| k == &key) {
            self.order.remove(pos);
        }
        self.entries.insert(key.clone(), value);
        self.order.push_back(key);
        while self.entries.len() > self.limit {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }
}

/// Resolves and caches the 73 bundled `.webp` plush/felt assets (`+ fishing.webm`).
pub struct BundledPlushAssets {
    root_dir: PathBuf,
    decoded_rgba: Mutex<HashMap<String, Arc<image::RgbaImage>>>,
    untinted_render: Mutex<HashMap<String, Arc<gpui::RenderImage>>>,
    tinted_rgba_cache: Mutex<BoundedCache<Arc<image::RgbaImage>>>,
    tinted_render_cache: Mutex<BoundedCache<Arc<gpui::RenderImage>>>,
}

impl Default for BundledPlushAssets {
    fn default() -> Self {
        Self::new()
    }
}

impl BundledPlushAssets {
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<BundledPlushAssets> = OnceLock::new();
        GLOBAL.get_or_init(Self::new)
    }

    pub fn new() -> Self {
        Self::with_root(Self::discover_root_dir())
    }

    pub fn with_root(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
            decoded_rgba: Mutex::new(HashMap::new()),
            untinted_render: Mutex::new(HashMap::new()),
            tinted_rgba_cache: Mutex::new(BoundedCache::new(MAX_TINT_CACHE_ENTRIES)),
            tinted_render_cache: Mutex::new(BoundedCache::new(MAX_TINT_CACHE_ENTRIES)),
        }
    }

    fn discover_root_dir() -> PathBuf {
        let manifest_candidate =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/openbot_plush");
        if manifest_candidate.is_dir() {
            return manifest_candidate;
        }
        if let Ok(cwd) = std::env::current_dir() {
            for rel in [
                "crates/gpui_telegram_desktop/assets/openbot_plush",
                "assets/openbot_plush",
            ] {
                let candidate = cwd.join(rel);
                if candidate.is_dir() {
                    return candidate;
                }
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let bundle = parent.join("../Resources/assets/openbot_plush");
                if bundle.is_dir() {
                    return bundle;
                }
            }
        }
        manifest_candidate
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub const fn bundled_relative_webp_paths() -> &'static [&'static str] {
        &BUNDLED_RELATIVE_WEBP_PATHS
    }

    pub fn bundled_webp_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::with_capacity(EXPECTED_BUNDLED_WEBP_COUNT);
        Self::collect_webp_recursive(&self.root_dir, &mut out);
        out.sort();
        out
    }

    fn collect_webp_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                Self::collect_webp_recursive(&path, out);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("webp"))
            {
                out.push(path);
            }
        }
    }

    fn normalize_key(rel_path: &str) -> Option<String> {
        let trimmed = rel_path.trim().trim_start_matches('/');
        let stripped = trimmed
            .strip_prefix("assets/openbot_plush/")
            .or_else(|| trimmed.strip_prefix("openbot_plush/"))
            .unwrap_or(trimmed);
        if stripped.is_empty() || stripped.contains("..") || stripped.contains('\\') {
            return None;
        }
        Some(stripped.to_owned())
    }

    pub fn resolve_asset_path(&self, rel_path: &str) -> Option<PathBuf> {
        let key = Self::normalize_key(rel_path)?;
        let direct = self.root_dir.join(&key);
        if direct.is_file() {
            return Some(direct);
        }
        if !key.contains('/') {
            for prefix in ["bloub", "ui", "ui/server", "ui/server/shape"] {
                let candidate = self.root_dir.join(prefix).join(&key);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    pub fn read_bytes(&self, rel_path: &str) -> Result<Vec<u8>, String> {
        let path = self
            .resolve_asset_path(rel_path)
            .ok_or_else(|| format!("plush asset not found: {rel_path}"))?;
        std::fs::read(&path).map_err(|err| format!("failed reading {}: {err}", path.display()))
    }

    pub fn decode_rgba(&self, rel_path: &str) -> Result<Arc<image::RgbaImage>, String> {
        let path = self
            .resolve_asset_path(rel_path)
            .ok_or_else(|| format!("plush asset not found: {rel_path}"))?;
        let canonical_key = path.to_string_lossy().into_owned();
        if let Some(cached) = self.decoded_rgba.lock().get(&canonical_key).cloned() {
            return Ok(cached);
        }
        let bytes = std::fs::read(&path)
            .map_err(|err| format!("failed reading {}: {err}", path.display()))?;
        let decoded = image::load_from_memory(&bytes)
            .map_err(|err| format!("failed decoding {}: {err}", path.display()))?
            .into_rgba8();
        let arc = Arc::new(decoded);
        self.decoded_rgba.lock().insert(canonical_key, arc.clone());
        Ok(arc)
    }

    pub fn render_image(&self, rel_path: &str) -> Result<Arc<gpui::RenderImage>, String> {
        let path = self
            .resolve_asset_path(rel_path)
            .ok_or_else(|| format!("plush asset not found: {rel_path}"))?;
        let canonical_key = path.to_string_lossy().into_owned();
        if let Some(cached) = self.untinted_render.lock().get(&canonical_key).cloned() {
            return Ok(cached);
        }
        let rgba = self.decode_rgba(rel_path)?;
        let render_img = rgba_to_gpui_render_image((*rgba).clone());
        self.untinted_render
            .lock()
            .insert(canonical_key, render_img.clone());
        Ok(render_img)
    }

    pub fn tinted_rgba(
        &self,
        rel_path: &str,
        hex: &str,
        target_px: Option<u32>,
    ) -> Result<Arc<image::RgbaImage>, String> {
        let resolved_hex = resolve_ink_hex(hex);
        let px_tag = target_px.unwrap_or(0);
        let cache_key = format!("{rel_path}|{resolved_hex}|{px_tag}");
        if let Some(cached) = self.tinted_rgba_cache.lock().get(&cache_key) {
            return Ok(cached);
        }
        let base = self.decode_rgba(rel_path)?;
        let working = match target_px {
            Some(sz) if sz > 0 && (sz != base.width() || sz != base.height()) => {
                image::imageops::resize(&*base, sz, sz, image::imageops::FilterType::Triangle)
            }
            _ => (*base).clone(),
        };
        let tinted = Arc::new(tint_rgba_image(&working, &resolved_hex));
        self.tinted_rgba_cache.lock().insert(cache_key, tinted.clone());
        Ok(tinted)
    }

    pub fn tinted_render_image(
        &self,
        rel_path: &str,
        hex: &str,
        target_px: Option<u32>,
    ) -> Result<Arc<gpui::RenderImage>, String> {
        let resolved_hex = resolve_ink_hex(hex);
        let px_tag = target_px.unwrap_or(0);
        let cache_key = format!("{rel_path}|{resolved_hex}|{px_tag}");
        if let Some(cached) = self.tinted_render_cache.lock().get(&cache_key) {
            return Ok(cached);
        }
        let tinted = self.tinted_rgba(rel_path, &resolved_hex, target_px)?;
        let render_img = rgba_to_gpui_render_image((*tinted).clone());
        self.tinted_render_cache
            .lock()
            .insert(cache_key, render_img.clone());
        Ok(render_img)
    }

    pub const fn shape_body_asset(shape: EngineShapeId) -> &'static str {
        match shape {
            EngineShapeId::Cercle => "bloub/forme-cercle.webp",
            EngineShapeId::Galet => "bloub/forme-galet.webp",
            EngineShapeId::Squircle => "bloub/forme-squircle.webp",
            EngineShapeId::Capsule => "bloub/forme-capsule.webp",
            EngineShapeId::Triangle => "bloub/forme-triangle.webp",
            EngineShapeId::Hexagone => "bloub/forme-hexagone.webp",
            EngineShapeId::Nuage => "bloub/forme-nuage.webp",
            EngineShapeId::Goutte => "bloub/forme-goutte.webp",
            EngineShapeId::Carre => "bloub/forme-carre.webp",
            EngineShapeId::Losange => "bloub/forme-losange.webp",
            EngineShapeId::Pentagone => "bloub/forme-pentagone.webp",
            EngineShapeId::Fantome => "bloub/forme-fantome.webp",
            EngineShapeId::Meduse => "bloub/forme-meduse.webp",
            EngineShapeId::Poulpe => "bloub/forme-poulpe.webp",
        }
    }
}

#[inline]
pub fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub fn linear_to_srgb(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

pub fn resolve_ink_hex(raw: &str) -> String {
    let trimmed = raw.trim();
    for named in BloubColorId::ALL {
        let named_hex = named.hex();
        let named_id = match named {
            BloubColorId::Encre => "encre",
            BloubColorId::Brun => "brun",
            BloubColorId::Rouge => "rouge",
            BloubColorId::Orange => "orange",
            BloubColorId::Ambre => "ambre",
            BloubColorId::Vert => "vert",
            BloubColorId::Turquoise => "turquoise",
            BloubColorId::Bleu => "bleu",
            BloubColorId::Violet => "violet",
            BloubColorId::Rose => "rose",
            BloubColorId::Gris => "gris",
            BloubColorId::Creme => "creme",
        };
        if trimmed.eq_ignore_ascii_case(named_id) {
            return named_hex.to_owned();
        }
    }
    let hex_digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex_digits.len() >= 6 && hex_digits.as_bytes()[..6].iter().all(u8::is_ascii_hexdigit) {
        return format!("#{}", hex_digits[..6].to_ascii_lowercase());
    }
    BloubColorId::Encre.hex().to_owned()
}

pub fn parse_hex_rgb(hex: &str) -> [u8; 3] {
    let resolved = resolve_ink_hex(hex);
    let digits = &resolved[1..7];
    let r = u8::from_str_radix(&digits[0..2], 16).unwrap_or(10);
    let g = u8::from_str_radix(&digits[2..4], 16).unwrap_or(10);
    let b = u8::from_str_radix(&digits[4..6], 16).unwrap_or(12);
    [r, g, b]
}

/// Exact 256×4 sRGB<->Linear felt tint ramp (`plush.js:L923-L945`, `plush-felt.ts:L393-L413`).
pub fn tint_ramp(hex: &str) -> [[u8; 4]; 256] {
    let [r8, g8, b8] = parse_hex_rgb(hex);
    let ink = [
        srgb_to_linear(f64::from(r8) / 255.0),
        srgb_to_linear(f64::from(g8) / 255.0),
        srgb_to_linear(f64::from(b8) / 255.0),
    ];
    let median = srgb_to_linear(GREY_REF);
    let sheen_span = 1.0_f64 - GREY_REF;
    let mut ramp = [[0_u8; 4]; 256];
    for (grey, out) in ramp.iter_mut().enumerate() {
        let gf = grey as f64 / 255.0;
        let light = (srgb_to_linear(gf) / median).powf(TINT_GAMMA);
        let sheen_val = TINT_SHEEN * ((gf - GREY_REF) / sheen_span).max(0.0).powi(2);
        for ch in 0..3 {
            let tinted = (ink[ch] * light).min(1.0);
            let srgb = linear_to_srgb(tinted + (1.0 - tinted) * sheen_val);
            out[ch] = (srgb * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
        }
        out[3] = 255;
    }
    ramp
}

/// Applies `tint_ramp(hex)` to an unpremultiplied RGBA8 greyscale felt texture while preserving alpha.
pub fn tint_rgba_image(src: &image::RgbaImage, hex: &str) -> image::RgbaImage {
    let ramp = tint_ramp(hex);
    let mut out = src.clone();
    for px in out.chunks_exact_mut(4) {
        let entry = ramp[px[0] as usize];
        px[0] = entry[0];
        px[1] = entry[1];
        px[2] = entry[2];
    }
    out
}

/// Rec.709 desaturation color matrix (`plush-draw.ts:L219-L243`).
pub fn saturation_matrix(s: f64) -> [f64; 20] {
    let sat = if s.is_finite() { s.clamp(0.0, 2.0) } else { 1.0 };
    [
        0.213 + 0.787 * sat, 0.715 - 0.715 * sat, 0.072 - 0.072 * sat, 0.0, 0.0,
        0.213 - 0.213 * sat, 0.715 + 0.285 * sat, 0.072 - 0.072 * sat, 0.0, 0.0,
        0.213 - 0.213 * sat, 0.715 - 0.715 * sat, 0.072 + 0.928 * sat, 0.0, 0.0,
        0.0,                 0.0,                 0.0,                 1.0, 0.0,
    ]
}

pub fn desaturate_rgba_image(src: &image::RgbaImage, saturation: f64) -> image::RgbaImage {
    let m = saturation_matrix(saturation);
    let mut out = src.clone();
    for px in out.chunks_exact_mut(4) {
        let r = f64::from(px[0]);
        let g = f64::from(px[1]);
        let b = f64::from(px[2]);
        px[0] = (m[0] * r + m[1] * g + m[2] * b + 0.5).floor().clamp(0.0, 255.0) as u8;
        px[1] = (m[5] * r + m[6] * g + m[7] * b + 0.5).floor().clamp(0.0, 255.0) as u8;
        px[2] = (m[10] * r + m[11] * g + m[12] * b + 0.5).floor().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Converts RGBA8 into GPUI's BGRA8 [`gpui::RenderImage`] (`pixel.swap(0, 2)`).
pub fn rgba_to_gpui_render_image(mut rgba: image::RgbaImage) -> Arc<gpui::RenderImage> {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let frame = image::Frame::new(rgba);
    Arc::new(gpui::RenderImage::new(vec![frame]))
}

pub fn plush_pixels(size_px: f32, scale_factor: f32) -> u32 {
    let scaled = (f64::from(size_px.max(1.0)) * f64::from(scale_factor.max(1.0))).ceil() as u32;
    let stepped = scaled.div_ceil(PIXEL_STEP) * PIXEL_STEP;
    stepped.max(PIXEL_STEP)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlushCharacterId {
    Nexo,
    Vita,
    Cloe,
    Malu,
    Yoshi,
    Fred,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PlushPadSpec {
    pub file_x: &'static str,
    pub file_y: &'static str,
    pub short: f64,
    pub kw: f64,
    pub kh: f64,
    pub dx: f64,
    pub dy: f64,
}

pub const GENERIC_PAD: PlushPadSpec = PlushPadSpec {
    file_x: "bloub/fred-oeil0-x.webp",
    file_y: "bloub/fred-oeil0-y.webp",
    short: 0.46,
    kw: 3.823213092207733,
    kh: 3.5378116570986435,
    dx: 0.0,
    dy: 0.0,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlushCharacterSpec {
    pub id: PlushCharacterId,
    pub shape: BloubShape,
    pub engine_shape: EngineShapeId,
    pub ink: &'static str,
    pub body_asset: &'static str,
    pub split: f64,
    pub gaze: GazeAngles,
    pub eyes: [EyeSpec; 2],
    pub left_pad: PlushPadSpec,
    pub right_pad: PlushPadSpec,
}

impl PlushCharacterId {
    pub const ALL: [Self; 6] = [
        Self::Nexo,
        Self::Vita,
        Self::Cloe,
        Self::Malu,
        Self::Yoshi,
        Self::Fred,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nexo => "nexo",
            Self::Vita => "vita",
            Self::Cloe => "cloe",
            Self::Malu => "malu",
            Self::Yoshi => "yoshi",
            Self::Fred => "fred",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "nexo" => Some(Self::Nexo),
            "vita" => Some(Self::Vita),
            "cloe" => Some(Self::Cloe),
            "malu" => Some(Self::Malu),
            "yoshi" => Some(Self::Yoshi),
            "fred" => Some(Self::Fred),
            _ => None,
        }
    }

    pub const fn spec(self) -> PlushCharacterSpec {
        match self {
            Self::Nexo => PlushCharacterSpec {
                id: Self::Nexo,
                shape: BloubShape::Squircle,
                engine_shape: EngineShapeId::Squircle,
                ink: "#e45206",
                body_asset: "bloub/nexo-corps.webp",
                split: 16.0,
                gaze: GazeAngles { yaw: 1.0, pitch: -7.0, roll: -1.0 },
                eyes: [
                    EyeSpec { w: 0.205, h: 0.43, tilt: -28.0, open: 1.0 },
                    EyeSpec { w: 0.205, h: 0.43, tilt: 35.0, open: 1.0 },
                ],
                left_pad: PlushPadSpec {
                    file_x: "bloub/nexo-oeil0-x.webp",
                    file_y: "bloub/nexo-oeil0-y.webp",
                    short: 0.205,
                    kw: 4.1283081570996973,
                    kh: 4.4794212382739209,
                    dx: 0.035,
                    dy: -0.065,
                },
                right_pad: PlushPadSpec {
                    file_x: "bloub/nexo-oeil1-x.webp",
                    file_y: "bloub/nexo-oeil1-y.webp",
                    short: 0.205,
                    kw: 4.1283081570996973,
                    kh: 4.4794212382739209,
                    dx: -0.015,
                    dy: -0.058,
                },
            },
            Self::Vita => PlushCharacterSpec {
                id: Self::Vita,
                shape: BloubShape::Hexagone,
                engine_shape: EngineShapeId::Hexagone,
                ink: "#22c377",
                body_asset: "bloub/vita-corps.webp",
                split: 17.0,
                gaze: GazeAngles { yaw: -4.0, pitch: 5.0, roll: 8.0 },
                eyes: [
                    EyeSpec { w: 0.27, h: 0.51, tilt: 3.0, open: 1.0 },
                    EyeSpec { w: 0.49, h: 0.13, tilt: -4.0, open: 1.0 },
                ],
                left_pad: PlushPadSpec {
                    file_x: "bloub/vita-oeil0-x.webp",
                    file_y: "bloub/vita-oeil0-y.webp",
                    short: 0.27,
                    kw: 4.0982228327723949,
                    kh: 4.1909039175755689,
                    dx: 0.034055756216271146,
                    dy: -0.05281578999623958,
                },
                right_pad: PlushPadSpec {
                    file_x: "bloub/vita-oeil1-x.webp",
                    file_y: "bloub/vita-oeil1-y.webp",
                    short: 0.13,
                    kw: 4.830250971433216,
                    kh: 5.4218004654231384,
                    dx: -0.026029860323685244,
                    dy: -0.05510519721699535,
                },
            },
            Self::Cloe => PlushCharacterSpec {
                id: Self::Cloe,
                shape: BloubShape::Triangle,
                engine_shape: EngineShapeId::Triangle,
                ink: "#df2986",
                body_asset: "bloub/cloe-corps.webp",
                split: 20.0,
                gaze: GazeAngles { yaw: 3.0, pitch: 16.0, roll: 0.0 },
                eyes: [
                    EyeSpec { w: 0.33, h: 0.12, tilt: 28.0, open: 1.0 },
                    EyeSpec { w: 0.34, h: 0.11, tilt: -26.0, open: 1.0 },
                ],
                left_pad: PlushPadSpec {
                    file_x: "bloub/cloe-oeil0-x.webp",
                    file_y: "bloub/cloe-oeil0-y.webp",
                    short: 0.12,
                    kw: 4.3591111005767456,
                    kh: 4.7558467554000261,
                    dx: 0.024552162164021694,
                    dy: -0.038398427223289566,
                },
                right_pad: PlushPadSpec {
                    file_x: "bloub/cloe-oeil1-x.webp",
                    file_y: "bloub/cloe-oeil1-y.webp",
                    short: 0.11,
                    kw: 4.8426765322109722,
                    kh: 4.9374116644835961,
                    dx: -0.041541687753233185,
                    dy: -0.03950575553898529,
                },
            },
            Self::Malu => PlushCharacterSpec {
                id: Self::Malu,
                shape: BloubShape::Cercle,
                engine_shape: EngineShapeId::Cercle,
                ink: "#9a55ef",
                body_asset: "bloub/malu-corps.webp",
                split: 15.0,
                gaze: GazeAngles { yaw: 9.0, pitch: 4.0, roll: -8.0 },
                eyes: [
                    EyeSpec { w: 0.23, h: 0.42, tilt: -3.0, open: 1.0 },
                    EyeSpec { w: 0.19, h: 0.18, tilt: 0.0, open: 1.0 },
                ],
                left_pad: PlushPadSpec {
                    file_x: "bloub/malu-oeil0-x.webp",
                    file_y: "bloub/malu-oeil0-y.webp",
                    short: 0.23,
                    kw: 4.6028828298850337,
                    kh: 4.3986407471813775,
                    dx: 0.009231984564124829,
                    dy: -0.0044476764477897265,
                },
                right_pad: PlushPadSpec {
                    file_x: "bloub/malu-oeil1-x.webp",
                    file_y: "bloub/malu-oeil1-y.webp",
                    short: 0.18,
                    kw: 4.5367971612046469,
                    kh: 4.78994535636234,
                    dx: -0.026116871314858032,
                    dy: 0.0003016649469345866,
                },
            },
            Self::Yoshi => PlushCharacterSpec {
                id: Self::Yoshi,
                shape: BloubShape::Capsule,
                engine_shape: EngineShapeId::Capsule,
                ink: "#e94741",
                body_asset: "bloub/yoshi-corps.webp",
                split: 18.0,
                gaze: GazeAngles { yaw: 11.0, pitch: 20.0, roll: 2.0 },
                eyes: [
                    EyeSpec { w: 0.30, h: 0.16, tilt: 17.0, open: 1.0 },
                    EyeSpec { w: 0.31, h: 0.15, tilt: -18.0, open: 1.0 },
                ],
                left_pad: PlushPadSpec {
                    file_x: "bloub/yoshi-oeil0-x.webp",
                    file_y: "bloub/yoshi-oeil0-y.webp",
                    short: 0.16,
                    kw: 4.4544964102618136,
                    kh: 4.8481994100583039,
                    dx: 0.03916727618687797,
                    dy: 0.007869041986432974,
                },
                right_pad: PlushPadSpec {
                    file_x: "bloub/yoshi-oeil1-x.webp",
                    file_y: "bloub/yoshi-oeil1-y.webp",
                    short: 0.15,
                    kw: 4.7981785125489926,
                    kh: 5.2193861457491044,
                    dx: -0.028381687537700435,
                    dy: 0.012892774756514975,
                },
            },
            Self::Fred => PlushCharacterSpec {
                id: Self::Fred,
                shape: BloubShape::Hexagone,
                engine_shape: EngineShapeId::Hexagone,
                ink: "#efb428",
                body_asset: "bloub/fred-corps.webp",
                split: 21.0,
                gaze: GazeAngles { yaw: 7.0, pitch: -3.0, roll: 0.0 },
                eyes: [
                    EyeSpec { w: 0.46, h: 0.50, tilt: 0.0, open: 1.0 },
                    EyeSpec { w: 0.48, h: 0.50, tilt: 0.0, open: 1.0 },
                ],
                left_pad: PlushPadSpec {
                    file_x: "bloub/fred-oeil0-x.webp",
                    file_y: "bloub/fred-oeil0-y.webp",
                    short: 0.46,
                    kw: 3.823213092207733,
                    kh: 3.5378116570986435,
                    dx: 0.039353114938410254,
                    dy: -0.035566855454058126,
                },
                right_pad: PlushPadSpec {
                    file_x: "bloub/fred-oeil1-x.webp",
                    file_y: "bloub/fred-oeil1-y.webp",
                    short: 0.48,
                    kw: 3.8659674576907657,
                    kh: 3.3799458008857739,
                    dx: -0.055407781997306026,
                    dy: -0.03778285688705488,
                },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadAxis {
    X,
    Y,
}

impl PadAxis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlushPiece {
    pub src: PadAxis,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub sx: f64,
    pub sy: f64,
    pub sw: f64,
    pub sh: f64,
}

impl PlushPiece {
    pub fn view_box_str(&self) -> String {
        format!("{} {} {} {}", self.sx, self.sy, self.sw, self.sh)
    }
}

/// Rest capsule unprojection (`lt`, `plush.js:L605-L615`, `plush-felt.ts:L218-L226`).
pub fn rest_capsule(matrix: &[f64; 6], open: f64, rest: f64, w: f64, h: f64) -> (f64, f64) {
    if rest >= 1.0 || open <= 1e-9 {
        return (w, h);
    }
    let [a, b, c, d, _, _] = *matrix;
    let b_open = b / open;
    let d_open = d / open;
    let denom_w = a.hypot(b_open).max(1e-9);
    let denom_h = c.hypot(d_open).max(1e-9);
    (
        w * a.hypot(b_open * rest) / denom_w,
        h * c.hypot(d_open * rest) / denom_h,
    )
}

/// 9-slice felt eye capsule generator (`Ae`, `BAR = 0.5`, `plush.js:L450-L475`).
pub fn pad_pieces(pad: &PlushPadSpec, rw: f64, rh: f64) -> Vec<PlushPiece> {
    let short = rw.min(rh);
    if short <= 1e-9 {
        return Vec::new();
    }
    let lying = rw >= rh;
    let extra = (rw.max(rh) - short) * 0.5;
    let (round, across) = if lying {
        (pad.kw * short, pad.kh * short)
    } else {
        (pad.kh * short, pad.kw * short)
    };
    let bar = PAD_BAR_RATIO * short;
    let length = (round + 2.0 * bar).max(1e-9);
    let make_piece = |from: f64, to: f64, start: f64, end: f64| -> PlushPiece {
        if lying {
            PlushPiece {
                src: PadAxis::X,
                x: from,
                y: -across * 0.5,
                w: to - from,
                h: across,
                sx: start / length,
                sy: 0.0,
                sw: (end - start) / length,
                sh: 1.0,
            }
        } else {
            PlushPiece {
                src: PadAxis::Y,
                x: -across * 0.5,
                y: from,
                w: across,
                h: to - from,
                sx: 0.0,
                sy: start / length,
                sw: 1.0,
                sh: (end - start) / length,
            }
        }
    };
    let mut pieces = Vec::new();
    let mut push_pair = |from: f64, to: f64, start: f64, end: f64| {
        pieces.push(make_piece(from, to, start, end));
        pieces.push(make_piece(-to, -from, length - end, length - start));
    };
    let step = bar.max(extra / 3.0);
    let same_step = (step - bar).abs() <= 1e-12;
    if same_step {
        push_pair(
            -extra - round * 0.5,
            (-extra + bar).min(0.0),
            0.0,
            round * 0.5 + extra.min(bar),
        );
    } else {
        push_pair(-extra - round * 0.5, -extra, 0.0, round * 0.5);
    }
    let mut index = usize::from(same_step);
    while (index as f64) * step < extra - 1e-9 * short {
        let from = -extra + (index as f64) * step;
        let to = (from + step).min(0.0);
        let start = round * 0.5 + ((index % 2) as f64) * bar;
        push_pair(from, to, start, start + ((to - from) / step) * bar);
        index += 1;
    }
    pieces
}

pub fn pad_extent(pieces: &[PlushPiece]) -> (f64, f64) {
    let mut ex = 0.0_f64;
    let mut ey = 0.0_f64;
    for p in pieces {
        ex = ex.max(p.x.abs()).max((p.x + p.w).abs());
        ey = ey.max(p.y.abs()).max((p.y + p.h).abs());
    }
    (ex, ey)
}

/// Unsquash inverse Jacobian (`ct`, `plush.js:L600-L604`, `plush-felt.ts:L203-L216`).
pub fn unsquash_matrix(matrix: &[f64; 6], open: f64) -> Option<[f64; 6]> {
    if open >= 1.0 || open <= 1e-9 {
        return None;
    }
    let [a, b, c, d, _, _] = *matrix;
    let b_open = b / open;
    let d_open = d / open;
    let det = a * d - b * c;
    if det.abs() <= 1e-12 {
        return None;
    }
    Some([
        (d * a - c * b_open) / det,
        (a * b_open - b * a) / det,
        (d * c - c * d_open) / det,
        (a * d_open - b * c) / det,
        0.0,
        0.0,
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LidBand {
    pub w: f64,
    pub h: f64,
    pub r: f64,
    pub alpha: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlushLidMask {
    pub cx: f64,
    pub cy: f64,
    pub s: f64,
    pub bands: [LidBand; LID_BANDS],
}

#[inline]
fn clamp01(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
fn ease_out_cubic(t: f64) -> f64 {
    let u = 1.0 - clamp01(t);
    1.0 - u * u * u
}

#[inline]
fn ease_in_out_cubic(t: f64) -> f64 {
    let u = clamp01(t);
    if u < 0.5 {
        4.0 * u * u * u
    } else {
        1.0 - (-2.0 * u + 2.0).powi(3) * 0.5
    }
}

#[inline]
fn ease_out_quint(t: f64) -> f64 {
    1.0 - (1.0 - clamp01(t)).powi(5)
}

/// 4-band soft eyelid mask (`ut`, `plush.js:L616-L634`, `plush-felt.ts:L241-L261`).
pub fn lid_mask(
    matrix: &[f64; 6],
    open: f64,
    rest: f64,
    capsule_half: (f64, f64),
    image_extent: (f64, f64),
) -> Option<PlushLidMask> {
    if open >= rest || (rest - BLINK_CLOSED) <= 1e-9 {
        return None;
    }
    let s = clamp01((open - BLINK_CLOSED) / (rest - BLINK_CLOSED));
    let short = capsule_half.0.min(capsule_half.1);
    let fringe = LID_FRINGE * 2.0 * short;
    let full = fringe.max(
        (image_extent.0 - capsule_half.0 + short)
            .hypot(image_extent.1 - capsule_half.1 + short)
            - short,
    );
    let margin = fringe + s.powi(8) * (full - fringe);
    let soft = LID_SOFT * 2.0 * short;
    let make_band = |band: usize| {
        let grow = margin + soft * (band as f64) / (LID_BANDS as f64);
        LidBand {
            w: capsule_half.0 + grow,
            h: capsule_half.1 + grow,
            r: short + grow,
            alpha: 1.0 / ((band + 1) as f64),
        }
    };
    Some(PlushLidMask {
        cx: matrix[4],
        cy: matrix[5],
        s,
        bands: [make_band(3), make_band(2), make_band(1), make_band(0)],
    })
}

/// Adaptive optical eye scale for small avatar sizes (`1.30 -> 1.0` across `18px..=36px`).
pub fn small_size_eye_scale(size_px: f32) -> f64 {
    if !size_px.is_finite() || size_px >= 36.0 {
        return 1.0;
    }
    if size_px <= 18.0 {
        return SMALL_AVATAR_EYE_SCALE_MAX;
    }
    let t = f64::from((size_px - 18.0) / (36.0 - 18.0)).clamp(0.0, 1.0);
    lerp(SMALL_AVATAR_EYE_SCALE_MAX, 1.0, t)
}

/// Optical-rim anchoring offsets when enlarging plush eyes (`rt`, `plush.js:L578-L598`).
pub fn eye_scale_offsets(eyes: &[([f64; 6], f64, f64)], eye_scale: f64) -> Vec<(f64, f64)> {
    let mut zeros = vec![(0.0, 0.0); eyes.len()];
    if !eye_scale.is_finite() || (eye_scale - 1.0).abs() <= 1e-9 || eyes.is_empty() {
        return zeros;
    }
    let n = eyes.len() as f64;
    let mut mx = 0.0;
    let mut my = 0.0;
    for (m, _, _) in eyes {
        mx += m[4] / n;
        my += m[5] / n;
    }
    let dist = mx.hypot(my);
    if dist < 1e-6 {
        return zeros;
    }
    let ux = mx / dist;
    let uy = my / dist;
    let mut max_proj = f64::NEG_INFINITY;
    for &(m, w, h) in eyes {
        let ext = ((w * 0.5) * (m[0] * ux + m[1] * uy)).hypot((h * 0.5) * (m[2] * ux + m[3] * uy));
        max_proj = max_proj.max(m[4] * ux + m[5] * uy + ext);
    }
    let ax = mx + (max_proj - dist) * ux;
    let ay = my + (max_proj - dist) * uy;
    for (out, &(m, _, _)) in zeros.iter_mut().zip(eyes.iter()) {
        *out = ((eye_scale - 1.0) * (m[4] - ax), (eye_scale - 1.0) * (m[5] - ay));
    }
    zeros
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlushLook {
    pub yaw: f64,
    pub pitch: f64,
    pub mix: f64,
    pub spin: f64,
    pub wander: f64,
    pub blink: Option<f64>,
}

pub const REST_PLUSH_LOOK: PlushLook = PlushLook {
    yaw: 0.0,
    pitch: 0.0,
    mix: 0.0,
    spin: 0.0,
    wander: 0.4,
    blink: None,
};

pub const THINK_PLUSH_LOOK: PlushLook = PlushLook {
    yaw: 18.0,
    pitch: 22.0,
    mix: 1.0,
    spin: 0.0,
    wander: 0.4,
    blink: None,
};

pub const ASLEEP_PLUSH_LOOK: PlushLook = PlushLook {
    yaw: 0.0,
    pitch: 0.0,
    mix: 0.0,
    spin: 0.0,
    wander: 0.15,
    blink: Some(0.0),
};

pub const fn engine_state_of(state: PlushState) -> BloubParametricState {
    match state {
        PlushState::Idle | PlushState::Thinking | PlushState::Asleep | PlushState::Failed => {
            BloubParametricState::Idle
        }
        PlushState::Wink => BloubParametricState::Wink,
        PlushState::Wide => BloubParametricState::Wide,
        PlushState::Notify => BloubParametricState::Notify,
    }
}

pub const fn morph_duration_of(state: PlushState) -> f64 {
    match state {
        PlushState::Idle | PlushState::Failed => 0.45,
        PlushState::Thinking => 0.40,
        PlushState::Wink => 0.30,
        PlushState::Wide => 0.55,
        PlushState::Notify | PlushState::Asleep => 0.50,
    }
}

pub const fn look_of(state: PlushState) -> PlushLook {
    match state {
        PlushState::Thinking => THINK_PLUSH_LOOK,
        PlushState::Asleep => ASLEEP_PLUSH_LOOK,
        _ => REST_PLUSH_LOOK,
    }
}

pub fn shake_deg_at(elapsed_sec: f64) -> f64 {
    if elapsed_sec <= 0.0 || elapsed_sec >= SHAKE_SEC {
        0.0
    } else {
        SHAKE_YAW_DEG
            * (2.0 * TAU * elapsed_sec / SHAKE_SEC).sin()
            * (PI * elapsed_sec / SHAKE_SEC).sin()
    }
}

pub fn enter_look(from: PlushLook, state: PlushState, elapsed_sec: f64) -> PlushLook {
    let to = look_of(state);
    let t = ease_out_quint(clamp01(elapsed_sec / morph_duration_of(state)));
    let blink = if from.blink.is_some() || to.blink.is_some() {
        Some(lerp(from.blink.unwrap_or(1.0), to.blink.unwrap_or(1.0), t))
    } else {
        None
    };
    let mut spin = lerp(from.spin, to.spin, t);
    if state == PlushState::Failed {
        spin += shake_deg_at(elapsed_sec);
    }
    PlushLook {
        yaw: lerp(from.yaw, to.yaw, t),
        pitch: lerp(from.pitch, to.pitch, t),
        mix: lerp(from.mix, to.mix, t),
        spin,
        wander: lerp(from.wander, to.wander, t),
        blink,
    }
}

pub fn asleep_face() -> ExpressionDef {
    let sleepy = expression_def(BloubExpression::Somnolent);
    let eye = EyeSpec {
        w: 0.30,
        h: 0.089,
        tilt: 0.0,
        open: 1.0,
    };
    ExpressionDef {
        id: BloubExpression::Somnolent,
        gaze: sleepy.gaze,
        split: sleepy.split,
        eyes: [eye, eye],
    }
}

pub fn failed_face(base: &ExpressionDef) -> ExpressionDef {
    let sad = expression_def(BloubExpression::Triste);
    let pick = usize::from(base.eyes[1].w * base.eyes[1].h > base.eyes[0].w * base.eyes[0].h);
    let chosen = base.eyes[pick];
    let ratio = chosen.w / chosen.h.max(1e-9);
    let h = if ratio > 0.6 && ratio < 1.7 {
        chosen.w / 1.8
    } else {
        chosen.h
    };
    ExpressionDef {
        id: BloubExpression::Triste,
        gaze: sad.gaze,
        split: base.split,
        eyes: [
            EyeSpec { w: chosen.w, h, tilt: sad.eyes[0].tilt, open: 1.0 },
            EyeSpec { w: chosen.w, h, tilt: sad.eyes[1].tilt, open: 1.0 },
        ],
    }
}

pub fn failed_symmetric_pads(pads: [PlushPadSpec; 2], base: &ExpressionDef) -> [PlushPadSpec; 2] {
    let pick = usize::from(base.eyes[1].w * base.eyes[1].h > base.eyes[0].w * base.eyes[0].h);
    let chosen = pads[pick];
    [
        PlushPadSpec { dx: pads[0].dx, dy: pads[0].dy, ..chosen },
        PlushPadSpec { dx: pads[1].dx, dy: pads[1].dy, ..chosen },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DecorTrack {
    pub phase: f64,
    pub at: f64,
    pub from: f64,
    pub on: bool,
    pub exit: f64,
}

impl DecorTrack {
    pub const OFF: Self = Self { phase: 0.0, at: f64::NEG_INFINITY, from: 0.0, on: false, exit: 1.0 };
    pub const SHOWN: Self = Self { phase: 0.0, at: f64::NEG_INFINITY, from: 1.0, on: true, exit: 1.0 };
}

pub fn decor_level(track: DecorTrack, now_sec: f64) -> f64 {
    let since = now_sec - track.at;
    if track.on {
        track.from + (1.0 - track.from) * ease_out_cubic(clamp01(since / EMERGE_SEC))
    } else {
        track.from * (1.0 - ease_out_quint(clamp01(since / track.exit.max(1e-6))))
    }
}

pub fn toggle_decor(track: DecorTrack, on: bool, now_sec: f64, next_state: PlushState) -> DecorTrack {
    if on == track.on {
        return track;
    }
    let level = decor_level(track, now_sec);
    DecorTrack {
        phase: if on && level < 0.001 { now_sec } else { track.phase },
        at: now_sec,
        from: level,
        on,
        exit: morph_duration_of(next_state),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlushBall {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub opacity: f64,
}

pub fn ball_box(x: f64, y: f64, r: f64, opacity: f64, unit: f64) -> PlushBall {
    let half = r / BALL_FILL;
    PlushBall {
        x: (x - half) * unit,
        y: (y - half) * unit,
        size: (2.0 * half) * unit,
        opacity,
    }
}

pub fn thinking_dots(track: DecorTrack, now_sec: f64) -> Vec<PlushBall> {
    const THINK_X: f64 = 0.62;
    const THINK_Y: f64 = -1.30;
    const THINK_SCALE: f64 = 0.60;
    const DOT_OFFSETS: [f64; 3] = [-0.557, -0.013, 0.532];
    const DOT_RADIUS: f64 = 0.165;
    const DOT_PULSE_PEAK: f64 = 1.25;

    let level = decor_level(track, now_sec);
    if level < 0.001 {
        return Vec::new();
    }
    DOT_OFFSETS
        .iter()
        .enumerate()
        .map(|(idx, &offset)| {
            let pulse = dot_pulse(now_sec - track.phase, idx);
            let r = DOT_RADIUS
                * THINK_SCALE
                * (1.0 + (DOT_PULSE_PEAK - 1.0) * pulse)
                * (0.5 + 0.5 * level);
            ball_box(
                THINK_X + offset * THINK_SCALE,
                THINK_Y,
                r,
                level * (0.55 + 0.45 * pulse),
                RAYON,
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlushZ {
    pub x: f64,
    pub y: f64,
    pub half: f64,
    pub width: f64,
    pub opacity: f64,
}

pub fn sleep_zs(track: DecorTrack, now_sec: f64) -> Vec<PlushZ> {
    const Z_LIFE: f64 = 3.0;
    const Z_COUNT: usize = 3;
    const Z_FROM_X: f64 = 0.55;
    const Z_FROM_Y: f64 = -0.74;
    const Z_TO_X: f64 = 1.25;
    const Z_TO_Y: f64 = -1.38;
    const Z_SIZE_MIN: f64 = 0.10;
    const Z_SIZE_MAX: f64 = 0.20;
    const Z_STROKE_RATIO: f64 = 0.24;

    let level = decor_level(track, now_sec);
    if level < 0.001 {
        return Vec::new();
    }
    let mut zs = Vec::with_capacity(Z_COUNT);
    for idx in 0..Z_COUNT {
        let raw = (now_sec - track.phase) / Z_LIFE - (idx as f64) / (Z_COUNT as f64);
        let life = ((raw % 1.0) + 1.0) % 1.0;
        let opacity = clamp01(life / 0.15) * clamp01((1.0 - life) / 0.45) * level;
        if opacity < 0.001 {
            continue;
        }
        let size = lerp(Z_SIZE_MIN, Z_SIZE_MAX, life) * RAYON;
        zs.push(PlushZ {
            x: (lerp(Z_FROM_X, Z_TO_X, life) + (life * TAU).sin() * 0.04) * RAYON,
            y: lerp(Z_FROM_Y, Z_TO_Y, life) * RAYON,
            half: size * 0.5,
            width: size * Z_STROKE_RATIO,
            opacity,
        });
    }
    zs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransitionBodyStage {
    Avant,
    Apres,
    Deux,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlushTransitionSample {
    pub corps: TransitionBodyStage,
    pub melange: f64,
    pub sx: f64,
    pub sy: f64,
    pub bascule: bool,
    pub fini: bool,
}

pub const fn transition_duration(transition: PlushTransition) -> f64 {
    match transition {
        PlushTransition::Cut | PlushTransition::Warp | PlushTransition::Dissolve => SHAPE_MORPH_SEC,
        PlushTransition::Flip => SHAPE_MORPH_SEC * 2.0,
        PlushTransition::Pop => POP_SHRINK_SEC + POP_GROW_SEC,
    }
}

/// Evaluates all 5 body transitions (`pe`, `plush.js:L222-L275`).
pub fn sample_transition(transition: PlushTransition, elapsed_sec: f64) -> PlushTransitionSample {
    if elapsed_sec < 0.0 {
        return PlushTransitionSample {
            corps: TransitionBodyStage::Avant,
            melange: 0.0,
            sx: 1.0,
            sy: 1.0,
            bascule: false,
            fini: false,
        };
    }
    let dur = transition_duration(transition);
    if elapsed_sec >= dur {
        return PlushTransitionSample {
            corps: TransitionBodyStage::Apres,
            melange: 1.0,
            sx: 1.0,
            sy: 1.0,
            bascule: true,
            fini: true,
        };
    }
    let r = elapsed_sec / dur;
    match transition {
        PlushTransition::Cut => PlushTransitionSample {
            corps: TransitionBodyStage::Apres,
            melange: 1.0,
            sx: 1.0,
            sy: 1.0,
            bascule: true,
            fini: false,
        },
        PlushTransition::Warp | PlushTransition::Dissolve => PlushTransitionSample {
            corps: TransitionBodyStage::Deux,
            melange: ease_out_quint(r),
            sx: 1.0,
            sy: 1.0,
            bascule: true,
            fini: false,
        },
        PlushTransition::Flip => {
            let e = ease_in_out_cubic(r);
            let swapped = e >= 0.5;
            PlushTransitionSample {
                corps: if swapped {
                    TransitionBodyStage::Apres
                } else {
                    TransitionBodyStage::Avant
                },
                melange: if swapped { 1.0 } else { 0.0 },
                sx: (PI * e).cos().abs(),
                sy: 1.0,
                bascule: swapped,
                fini: false,
            }
        }
        PlushTransition::Pop => {
            if elapsed_sec < POP_SHRINK_SEC {
                let s = 1.0 - ease_in_out_cubic(elapsed_sec / POP_SHRINK_SEC);
                PlushTransitionSample {
                    corps: TransitionBodyStage::Avant,
                    melange: 0.0,
                    sx: s,
                    sy: s,
                    bascule: false,
                    fini: false,
                }
            } else {
                let e = elapsed_sec - POP_SHRINK_SEC;
                let u = clamp01(e / POP_GROW_SEC);
                let overshoot = if e < POP_GROW_SEC {
                    1.0 + 0.14 * (PI * u).sin() * (1.0 - 0.35 * u)
                } else {
                    1.0
                };
                let s = ease_out_quint(u) * overshoot;
                PlushTransitionSample {
                    corps: TransitionBodyStage::Apres,
                    melange: 1.0,
                    sx: s,
                    sy: s,
                    bascule: true,
                    fini: false,
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PlushClosedArc {
    pub w: f64,
    pub stroke_width: f64,
    pub bend_y: f64,
    pub opacity: f64,
    pub color: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlushEyeLayer {
    pub transform: ComposedEyeTransform,
    pub pad: PlushPadSpec,
    pub fade_from_pad: Option<PlushPadSpec>,
    pub fade_from_pieces: Vec<PlushPiece>,
    pub pad_fade: f64,
    pub inner_unsquash: Option<[f64; 6]>,
    pub pieces: Vec<PlushPiece>,
    pub pad_opacity: f64,
    pub translate_x: f64,
    pub translate_y: f64,
    pub lid: Option<PlushLidMask>,
    pub closed_arc: Option<PlushClosedArc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlushFrameSample {
    pub character: Option<PlushCharacterId>,
    pub shape: EngineShapeId,
    pub state: PlushState,
    pub expression: BloubExpression,
    pub ink_hex: String,
    pub time_sec: f64,
    pub body_alpha: f64,
    pub breath_sy: f64,
    pub body_path: String,
    pub silhouette: Silhouette,
    pub gaze: GazeAngles,
    pub split: f64,
    pub zero_clipping: bool,
    pub eye_scale: f64,
    pub pad_blend: &'static str,
    pub eyes: [PlushEyeLayer; 2],
    pub busy_dots: Vec<PlushBall>,
    pub notify_color: Option<&'static str>,
    pub notif_ball: Option<PlushBall>,
    pub sleep_zs: Vec<PlushZ>,
    pub shake_deg: f64,
    pub transition: PlushTransition,
    pub transition_elapsed_sec: f64,
    pub transition_sample: PlushTransitionSample,
    pub avant_body_asset: String,
    pub avant_ink_hex: Option<String>,
    pub avant_radii: [f64; 64],
    pub apres_body_asset: String,
    pub apres_ink_hex: Option<String>,
    pub apres_radii: [f64; 64],
}

#[derive(Debug, Clone, PartialEq)]
struct BodyTarget {
    key: String,
    character: Option<PlushCharacterId>,
    shape: EngineShapeId,
    body_asset: String,
    tint_hex: Option<String>,
    ink_hex: String,
    pads: [PlushPadSpec; 2],
    radii: [f64; 64],
}

impl BodyTarget {
    fn from_config(
        character: Option<PlushCharacterId>,
        shape: EngineShapeId,
        color_hex: &str,
    ) -> Self {
        if let Some(ch) = character {
            let spec = ch.spec();
            let radii = engine_shape_radii(spec.engine_shape);
            return Self {
                key: format!("char:{}", ch.as_str()),
                character: Some(ch),
                shape: spec.engine_shape,
                body_asset: spec.body_asset.to_owned(),
                tint_hex: None,
                ink_hex: spec.ink.to_owned(),
                pads: [spec.left_pad, spec.right_pad],
                radii,
            };
        }
        let ink_hex = resolve_ink_hex(color_hex);
        let body_asset = BundledPlushAssets::shape_body_asset(shape).to_owned();
        let radii = engine_shape_radii(shape);
        Self {
            key: format!("{ink_hex}|{body_asset}"),
            character: None,
            shape,
            body_asset,
            tint_hex: Some(ink_hex.clone()),
            ink_hex,
            pads: [GENERIC_PAD, GENERIC_PAD],
            radii,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PadFadeState {
    from_pads: Option<[PlushPadSpec; 2]>,
    at: f64,
    start: f64,
}

impl PadFadeState {
    const NONE: Self = Self {
        from_pads: None,
        at: f64::NEG_INFINITY,
        start: 1.0,
    };

    fn progress(self, now_sec: f64) -> f64 {
        if self.from_pads.is_some() {
            self.start + (1.0 - self.start) * ease_out_quint(clamp01((now_sec - self.at) / SHAPE_MORPH_SEC))
        } else {
            1.0
        }
    }
}

/// Stateful 3D tactile felt `PlushBot` renderer (`plush.js:L976-L1208`).
pub struct PlushFeltRenderer {
    character: Option<PlushCharacterId>,
    shape: EngineShapeId,
    ink_hex: String,
    expression: BloubExpression,
    expression_override: Option<BloubExpression>,
    state: PlushState,
    transition: PlushTransition,
    eye_scale: f64,
    pointer_gaze: Option<(f64, f64)>,
    avant: BodyTarget,
    apres: BodyTarget,
    queued_target: Option<BodyTarget>,
    transition_start_sec: f64,
    in_transition: bool,
    state_change_sec: f64,
    look_origin: PlushLook,
    last_look: PlushLook,
    dots_track: DecorTrack,
    zs_track: DecorTrack,
    pad_fade: PadFadeState,
    engine: BloubBotEngine,
    last_clock_sec: f64,
}

pub const fn bloub_shape_to_engine(shape: BloubShape) -> EngineShapeId {
    match shape {
        BloubShape::Cercle => EngineShapeId::Cercle,
        BloubShape::Galet => EngineShapeId::Galet,
        BloubShape::Squircle => EngineShapeId::Squircle,
        BloubShape::Capsule => EngineShapeId::Capsule,
        BloubShape::Triangle => EngineShapeId::Triangle,
        BloubShape::Hexagone => EngineShapeId::Hexagone,
        BloubShape::Nuage => EngineShapeId::Nuage,
        BloubShape::Carre => EngineShapeId::Carre,
        BloubShape::Losange => EngineShapeId::Losange,
        BloubShape::Pentagone => EngineShapeId::Pentagone,
        BloubShape::Fantome => EngineShapeId::Fantome,
        BloubShape::Meduse => EngineShapeId::Meduse,
        BloubShape::Poulpe => EngineShapeId::Poulpe,
    }
}

#[inline]
fn harmonic_wave(t: f64, period: f64, phase: f64) -> f64 {
    let r = TAU * t / period;
    0.55 * (r + phase).sin()
        + 0.30 * (2.0 * r + 1.7 * phase + 1.1).sin()
        + 0.15 * (3.0 * r + 2.3 * phase + 2.4).sin()
}

impl PlushFeltRenderer {
    pub fn new(
        shape: EngineShapeId,
        color_hex: &str,
        expression: BloubExpression,
        state: PlushState,
    ) -> Self {
        let target = BodyTarget::from_config(None, shape, color_hex);
        let initial_look = look_of(state);
        let dots_track = if state == PlushState::Thinking {
            DecorTrack::SHOWN
        } else {
            DecorTrack::OFF
        };
        let zs_track = if state == PlushState::Asleep {
            DecorTrack::SHOWN
        } else {
            DecorTrack::OFF
        };
        let engine = BloubBotEngine::new(shape, expression, engine_state_of(state));
        Self {
            character: None,
            shape,
            ink_hex: target.ink_hex.clone(),
            expression,
            expression_override: None,
            state,
            transition: PlushTransition::Flip,
            eye_scale: 1.0,
            pointer_gaze: None,
            avant: target.clone(),
            apres: target,
            queued_target: None,
            transition_start_sec: f64::NEG_INFINITY,
            in_transition: false,
            state_change_sec: f64::NEG_INFINITY,
            look_origin: initial_look,
            last_look: initial_look,
            dots_track,
            zs_track,
            pad_fade: PadFadeState::NONE,
            engine,
            last_clock_sec: 0.0,
        }
    }

    pub fn for_character(character: PlushCharacterId, state: PlushState) -> Self {
        let spec = character.spec();
        let mut renderer = Self::new(
            spec.engine_shape,
            spec.ink,
            BloubExpression::Neutre,
            state,
        );
        renderer.character = Some(character);
        let target = BodyTarget::from_config(Some(character), spec.engine_shape, spec.ink);
        renderer.ink_hex = target.ink_hex.clone();
        renderer.avant = target.clone();
        renderer.apres = target;
        renderer
    }

    pub fn for_seed(
        seed: &str,
        hue: Option<AvatarHue>,
        color: Option<&str>,
        mood: AvatarMood,
    ) -> Self {
        let profile = bloub_avatar_profile(seed, hue, color);
        let mood_pres = plush_mood_presentation(mood);
        let expr = mood_pres.expression.unwrap_or(profile.expression);
        let mut renderer = Self::new(
            bloub_shape_to_engine(profile.shape),
            &profile.color,
            expr,
            mood_pres.state,
        );
        renderer.transition = PlushTransition::Pop;
        renderer
    }

    fn push_body_target(&mut self, next: BodyTarget, now_sec: f64) {
        if self.apres.key == next.key {
            self.queued_target = None;
            if self.avant.key == next.key {
                self.avant = next.clone();
            }
            self.apres = next;
            return;
        }
        if self.in_transition && self.transition != PlushTransition::Cut {
            self.queued_target = Some(next);
            return;
        }
        self.queued_target = None;
        if self.transition == PlushTransition::Cut || !now_sec.is_finite() {
            self.avant = next.clone();
            self.apres = next;
            self.transition_start_sec = f64::NEG_INFINITY;
            self.in_transition = false;
            return;
        }
        self.in_transition = true;
        self.avant = self.apres.clone();
        self.apres = next;
        self.transition_start_sec = now_sec;
    }

    pub fn set_character(&mut self, character: Option<PlushCharacterId>, now_sec: f64) {
        self.character = character;
        if let Some(ch) = character {
            let spec = ch.spec();
            self.shape = spec.engine_shape;
            self.ink_hex = spec.ink.to_owned();
        }
        let next = BodyTarget::from_config(self.character, self.shape, &self.ink_hex);
        self.engine = BloubBotEngine::new(next.shape, self.expression, engine_state_of(self.state));
        self.push_body_target(next, now_sec);
    }

    pub fn set_shape(&mut self, shape: EngineShapeId, now_sec: f64) {
        self.character = None;
        self.shape = shape;
        let next = BodyTarget::from_config(None, self.shape, &self.ink_hex);
        self.engine = BloubBotEngine::new(self.shape, self.expression, engine_state_of(self.state));
        self.push_body_target(next, now_sec);
    }

    pub fn set_color(&mut self, color_hex: &str, now_sec: f64) {
        if self.character.is_some() {
            return;
        }
        self.ink_hex = resolve_ink_hex(color_hex);
        let next = BodyTarget::from_config(None, self.shape, &self.ink_hex);
        self.push_body_target(next, now_sec);
    }

    pub fn set_expression(&mut self, expression: BloubExpression) {
        self.expression = expression;
        self.expression_override = Some(expression);
        self.engine = BloubBotEngine::new(self.shape, self.expression, engine_state_of(self.state));
    }

    pub fn clear_expression_override(&mut self) {
        self.expression_override = None;
    }

    pub fn set_transition(&mut self, transition: PlushTransition) {
        self.transition = transition;
    }

    pub fn set_eye_scale(&mut self, eye_scale: f64) {
        self.eye_scale = if eye_scale.is_finite() && eye_scale > 0.0 {
            eye_scale
        } else {
            1.0
        };
    }

    pub fn set_pointer_gaze(&mut self, normalized_xy: Option<(f64, f64)>) {
        self.pointer_gaze = normalized_xy.map(|(x, y)| (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)));
    }

    pub const fn character(&self) -> Option<PlushCharacterId> {
        self.character
    }

    pub const fn shape(&self) -> EngineShapeId {
        self.shape
    }

    pub fn ink_hex(&self) -> &str {
        &self.ink_hex
    }

    pub const fn state(&self) -> PlushState {
        self.state
    }

    pub const fn transition(&self) -> PlushTransition {
        self.transition
    }

    pub const fn eye_scale(&self) -> f64 {
        self.eye_scale
    }

    pub const fn last_clock_sec(&self) -> f64 {
        self.last_clock_sec
    }

    fn current_pads(&self, state: PlushState) -> [PlushPadSpec; 2] {
        let resting = self.resting_face_for(&self.apres);
        if state == PlushState::Failed {
            failed_symmetric_pads(self.apres.pads, &resting)
        } else {
            self.apres.pads
        }
    }

    fn resting_face_for(&self, target: &BodyTarget) -> ExpressionDef {
        if let Some(ch) = target.character {
            if self.expression_override.is_none() {
                let spec = ch.spec();
                return ExpressionDef {
                    id: self.expression,
                    gaze: spec.gaze,
                    split: spec.split,
                    eyes: spec.eyes,
                };
            }
        }
        expression_def(self.expression)
    }

    pub fn set_state(&mut self, next_state: PlushState, now_sec: f64) {
        if next_state == self.state {
            return;
        }
        let prev_state = self.state;
        if (next_state == PlushState::Failed) != (prev_state == PlushState::Failed)
            && now_sec.is_finite()
        {
            let prev_pads = self.current_pads(prev_state);
            let cur_progress = self.pad_fade.progress(now_sec);
            self.pad_fade = PadFadeState {
                from_pads: Some(prev_pads),
                at: now_sec,
                start: 1.0 - cur_progress,
            };
        }
        self.look_origin = self.last_look;
        self.state = next_state;
        self.state_change_sec = now_sec;
        self.dots_track = toggle_decor(
            self.dots_track,
            next_state == PlushState::Thinking,
            now_sec,
            next_state,
        );
        self.zs_track = toggle_decor(
            self.zs_track,
            next_state == PlushState::Asleep,
            now_sec,
            next_state,
        );
        self.engine =
            BloubBotEngine::new(self.shape, self.expression, engine_state_of(self.state));
    }

    pub fn sample_plush_frame(&mut self, now_sec: f64, reduced_motion: bool) -> PlushFrameSample {
        let sample_t = if reduced_motion {
            self.state.frozen_pose_time()
        } else if now_sec.is_finite() {
            now_sec.max(0.0)
        } else {
            0.0
        };
        self.last_clock_sec = sample_t;

        let mut trans_elapsed = if reduced_motion || !self.in_transition {
            transition_duration(self.transition)
        } else {
            (sample_t - self.transition_start_sec).max(0.0)
        };
        let mut trans_sample = if reduced_motion || !self.in_transition {
            PlushTransitionSample {
                corps: TransitionBodyStage::Apres,
                melange: 1.0,
                sx: 1.0,
                sy: 1.0,
                bascule: true,
                fini: true,
            }
        } else {
            sample_transition(self.transition, trans_elapsed)
        };
        if self.in_transition && trans_sample.fini {
            self.in_transition = false;
            self.avant = self.apres.clone();
            if let Some(queued) = self.queued_target.take() {
                self.push_body_target(queued, sample_t);
                trans_elapsed = 0.0;
                trans_sample = sample_transition(self.transition, 0.0);
            }
        }

        let shown = if trans_sample.bascule {
            self.apres.clone()
        } else {
            self.avant.clone()
        };
        let state_elapsed = if !reduced_motion && self.state_change_sec.is_finite() {
            (sample_t - self.state_change_sec).max(0.0)
        } else {
            10.0
        };
        let look = if reduced_motion {
            PlushLook {
                wander: 0.0,
                spin: 0.0,
                ..look_of(self.state)
            }
        } else if self.state_change_sec.is_finite() {
            enter_look(self.look_origin, self.state, state_elapsed)
        } else {
            look_of(self.state)
        };
        self.last_look = look;
        let shake_deg = if !reduced_motion && self.state == PlushState::Failed {
            shake_deg_at(state_elapsed)
        } else {
            0.0
        };

        let engine_sample = self.engine.sample(sample_t, reduced_motion);
        let resting_face = self.resting_face_for(&shown);
        let active_face = match self.state {
            PlushState::Asleep => asleep_face(),
            PlushState::Failed => failed_face(&resting_face),
            PlushState::Wink => ExpressionDef {
                id: self.expression,
                gaze: GazeAngles { yaw: -5.37, pitch: 4.55, roll: 6.7 },
                split: 16.25,
                eyes: [
                    EyeSpec { w: 0.236, h: 0.464, tilt: 0.0, open: 1.0 },
                    EyeSpec { w: 0.447, h: 0.089, tilt: 0.0, open: 1.0 },
                ],
            },
            PlushState::Wide => ExpressionDef {
                id: self.expression,
                gaze: GazeAngles { yaw: 6.92, pitch: -21.96, roll: 11.6 },
                split: 18.43,
                eyes: [EyeSpec { w: 0.356, h: 0.875, tilt: 0.0, open: 1.0 }; 2],
            },
            PlushState::Notify => ExpressionDef {
                id: self.expression,
                gaze: GazeAngles { yaw: -21.94, pitch: -5.82, roll: -12.2 },
                split: 18.89,
                eyes: [EyeSpec { w: 0.505, h: 0.498, tilt: 0.0, open: 1.0 }; 2],
            },
            PlushState::Idle | PlushState::Thinking => resting_face,
        };
        let active_pads = if self.state == PlushState::Failed {
            failed_symmetric_pads(shown.pads, &resting_face)
        } else {
            shown.pads
        };

        self.build_frame_sample(
            sample_t,
            reduced_motion,
            state_elapsed,
            &shown,
            &active_face,
            active_pads,
            look,
            shake_deg,
            &engine_sample,
            trans_elapsed,
            trans_sample,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_frame_sample(
        &self,
        sample_t: f64,
        reduced_motion: bool,
        state_elapsed: f64,
        shown: &BodyTarget,
        active_face: &ExpressionDef,
        active_pads: [PlushPadSpec; 2],
        look: PlushLook,
        shake_deg: f64,
        engine_sample: &super::bot_engine::BloubFrameSample,
        trans_elapsed: f64,
        trans_sample: PlushTransitionSample,
    ) -> PlushFrameSample {
        let (d_yaw, d_pitch, d_roll, drift_x, drift_y, lid, breath_sy) = if reduced_motion {
            (0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0)
        } else {
            let w_amt = look.wander;
            let dy = (harmonic_wave(sample_t, 11.3, 0.4) * 5.5
                + harmonic_wave(sample_t, 3.7, 2.1) * 1.6)
                * w_amt;
            let dp = (harmonic_wave(sample_t, 9.1, 1.3) * 4.2
                + harmonic_wave(sample_t, 4.3, 0.7) * 1.3)
                * w_amt;
            let dr = harmonic_wave(sample_t, 13.7, 3.2) * 2.2 * w_amt;
            let dx = harmonic_wave(sample_t, 7.9, 1.9) * 0.006;
            let dfy = harmonic_wave(sample_t, 5.3, 0.3) * 0.007;
            let raw_lid = blink_lid_at(sample_t);
            let blink_lid = match look.blink {
                None => raw_lid,
                Some(b) => 1.0 - (1.0 - raw_lid) * clamp01(b),
            };
            let breath = 1.0 + (sample_t / 3.4 * TAU).sin() * 0.005;
            (dy, dp, dr, dx, dfy, blink_lid, breath)
        };

        let mut target_yaw = lerp(active_face.gaze.yaw, look.yaw, look.mix);
        let mut target_pitch = lerp(active_face.gaze.pitch, look.pitch, look.mix);
        if !reduced_motion
            && matches!(
                self.state,
                PlushState::Idle | PlushState::Wink | PlushState::Wide | PlushState::Notify
            )
        {
            if let Some((nx, ny)) = self.pointer_gaze {
                target_yaw = -26.0 + nx * 16.0;
                target_pitch = 10.0 - ny * 13.0;
            }
        }
        let gaze = GazeAngles {
            yaw: target_yaw + d_yaw - look.spin,
            pitch: target_pitch + d_pitch,
            roll: active_face.gaze.roll + d_roll,
        };

        let sil = Silhouette {
            radii: shown.radii,
            rot: 0.0,
            cx: 0.0,
            cy: 0.0,
            sx: 1.0,
            sy: breath_sy,
        };
        let decalage: DecalageOffset = if shown.character.is_some() {
            DecalageOffset::default()
        } else {
            let expr_key = match self.state {
                PlushState::Idle | PlushState::Thinking => Some(active_face.id),
                _ => None,
            };
            decalage_for(shown.shape, engine_state_of(self.state), expr_key)
        };

        let basis = project_eyes_3d(gaze, RAYON, active_face.split);
        let raw_eyes: [ComposedEyeTransform; 2] = std::array::from_fn(|idx| {
            let r = basis[idx];
            let spec = active_face.eyes[idx];
            let culled = r.depth <= 0.02;
            let alpha = if culled {
                0.0
            } else {
                engine_sample.eye_alpha * clamp01(r.depth / 0.12)
            };
            let rest_open = blink_scale(spec.open);
            let open_squash = blink_scale(lid.min(spec.open));
            let s = radius_at_angle(&sil.radii, r.y.atan2(r.x) - sil.rot);
            let tilt_rad = spec.tilt.to_radians();
            let (lc, ls) = (tilt_rad.cos(), tilt_rad.sin());
            let m_a = r.a * lc + r.c * ls;
            let m_b = (r.b * lc + r.d * ls) * open_squash;
            let m_c = -r.a * ls + r.c * lc;
            let m_d = (-r.b * ls + r.d * lc) * open_squash;
            let tx = r.x * s + (drift_x + decalage.x) * RAYON;
            let ty = r.y * s + (drift_y + decalage.y) * RAYON;
            ComposedEyeTransform {
                w: spec.w * RAYON,
                h: spec.h * RAYON,
                open: spec.open,
                rest_open,
                open_squash,
                alpha,
                culled,
                depth: r.depth,
                matrix: [m_a, m_b, m_c, m_d, tx, ty],
            }
        });
        let mut raw_eyes = raw_eyes;
        let zero_clipping = if shown.character.is_none() {
            fit_eyes_zero_clipping(&sil, &mut raw_eyes)
        } else {
            verify_eyes_zero_clipping(&sil, &raw_eyes)
        };

        let scale_inputs: [([f64; 6], f64, f64); 2] = std::array::from_fn(|idx| {
            let mut m = raw_eyes[idx].matrix;
            m[4] += active_pads[idx].dx * RAYON;
            m[5] += active_pads[idx].dy * RAYON;
            (m, raw_eyes[idx].w, raw_eyes[idx].h)
        });
        let rim_offsets = eye_scale_offsets(&scale_inputs, self.eye_scale);

        let z_level = if reduced_motion {
            if self.state == PlushState::Asleep { 1.0 } else { 0.0 }
        } else {
            decor_level(self.zs_track, sample_t)
        };
        let pad_fade = if reduced_motion {
            1.0
        } else {
            self.pad_fade.progress(sample_t)
        };

        let eyes: [PlushEyeLayer; 2] = std::array::from_fn(|idx| {
            let tr = raw_eyes[idx];
            let pad = active_pads[idx];
            let (cap_w, cap_h) =
                rest_capsule(&tr.matrix, tr.open_squash, tr.rest_open, tr.w, tr.h);
            let pieces = pad_pieces(&pad, cap_w, cap_h);
            let (fade_from_pad, fade_from_pieces) = match self.pad_fade.from_pads {
                Some(prev) if pad_fade < 1.0 && prev[idx].file_x != pad.file_x => {
                    (Some(prev[idx]), pad_pieces(&prev[idx], cap_w, cap_h))
                }
                _ => (None, Vec::new()),
            };
            let inner_unsquash = unsquash_matrix(&tr.matrix, tr.open_squash);
            let lid = if z_level < 1.0 {
                lid_mask(
                    &tr.matrix,
                    tr.open_squash,
                    tr.rest_open,
                    (cap_w * 0.5, cap_h * 0.5),
                    pad_extent(&pieces),
                )
            } else {
                None
            };
            let closed_arc = if z_level > 0.0 {
                Some(PlushClosedArc {
                    w: tr.w,
                    stroke_width: tr.w * CLOSED_EYE_STROKE,
                    bend_y: tr.w * CLOSED_EYE_BEND,
                    opacity: z_level,
                    color: CLOSED_EYE_COLOR,
                })
            } else {
                None
            };
            PlushEyeLayer {
                transform: tr,
                pad,
                fade_from_pad,
                fade_from_pieces,
                pad_fade,
                inner_unsquash,
                pieces,
                pad_opacity: (1.0 - z_level).clamp(0.0, 1.0),
                translate_x: pad.dx * RAYON + rim_offsets[idx].0,
                translate_y: pad.dy * RAYON + rim_offsets[idx].1,
                lid,
                closed_arc,
            }
        });

        let busy_dots = if reduced_motion {
            if self.state == PlushState::Thinking {
                thinking_dots(DecorTrack::SHOWN, sample_t)
            } else {
                Vec::new()
            }
        } else {
            thinking_dots(self.dots_track, sample_t)
        };
        let sleep_zs_out = if reduced_motion {
            if self.state == PlushState::Asleep {
                sleep_zs(DecorTrack::SHOWN, 1.2)
            } else {
                Vec::new()
            }
        } else {
            sleep_zs(self.zs_track, sample_t)
        };
        let notif_ball = if self.state == PlushState::Notify || engine_sample.notif.is_some() {
            let t = clamp01(state_elapsed / SHAPE_MORPH_SEC);
            let over = if t < 1.0 {
                1.0 + 0.14 * (t * PI).sin() * (1.0 - 0.35 * t)
            } else {
                1.0
            };
            let r = 0.15 * over;
            let ang = (-42.0_f64).to_radians();
            Some(ball_box(ang.cos() * 1.003, ang.sin() * 1.003, r, 1.0, RAYON))
        } else {
            None
        };
        let body_path = silhouette_svg_path(&sil, RAYON);

        PlushFrameSample {
            character: shown.character,
            shape: shown.shape,
            state: self.state,
            expression: active_face.id,
            ink_hex: shown.ink_hex.clone(),
            time_sec: sample_t,
            body_alpha: engine_sample.body_alpha,
            breath_sy,
            body_path,
            silhouette: sil,
            gaze,
            split: active_face.split,
            zero_clipping,
            eye_scale: self.eye_scale,
            pad_blend: PAD_BLEND_MODE,
            eyes,
            busy_dots,
            notify_color: notif_ball.map(|_| NOTIFY_COLOR),
            notif_ball,
            sleep_zs: sleep_zs_out,
            shake_deg,
            transition: self.transition,
            transition_elapsed_sec: trans_elapsed,
            transition_sample: trans_sample,
            avant_body_asset: self.avant.body_asset.clone(),
            avant_ink_hex: self.avant.tint_hex.clone(),
            avant_radii: self.avant.radii,
            apres_body_asset: self.apres.body_asset.clone(),
            apres_ink_hex: self.apres.tint_hex.clone(),
            apres_radii: self.apres.radii,
        }
    }
}

#[inline]
fn sample_rgba_premul_bilinear(img: &image::RgbaImage, u: f64, v: f64) -> [f64; 4] {
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 {
        return [0.0; 4];
    }
    let fx = (u.clamp(0.0, 1.0) * f64::from(w) - 0.5).clamp(0.0, f64::from(w - 1));
    let fy = (v.clamp(0.0, 1.0) * f64::from(h) - 0.5).clamp(0.0, f64::from(h - 1));
    let x0 = fx.floor() as u32;
    let y0 = fy.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = fx - f64::from(x0);
    let ty = fy - f64::from(y0);
    let fetch = |x: u32, y: u32| -> [f64; 4] {
        let p = img.get_pixel(x, y).0;
        let a = f64::from(p[3]) / 255.0;
        [
            (f64::from(p[0]) / 255.0) * a,
            (f64::from(p[1]) / 255.0) * a,
            (f64::from(p[2]) / 255.0) * a,
            a,
        ]
    };
    let c00 = fetch(x0, y0);
    let c10 = fetch(x1, y0);
    let c01 = fetch(x0, y1);
    let c11 = fetch(x1, y1);
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = c00[i] + (c10[i] - c00[i]) * tx;
        let bot = c01[i] + (c11[i] - c01[i]) * tx;
        out[i] = top + (bot - top) * ty;
    }
    out
}

#[inline]
fn premul_to_rgba8(p: [f64; 4]) -> [u8; 4] {
    let a = p[3].clamp(0.0, 1.0);
    if a <= 1e-6 {
        return [0, 0, 0, 0];
    }
    [
        ((p[0] / a).clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8,
        ((p[1] / a).clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8,
        ((p[2] / a).clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8,
        (a * 255.0 + 0.5).floor() as u8,
    ]
}

/// 64-ray radial `Warp` body interpolator (`plush.js:L276-L333`).
pub fn warp_blend_bodies(
    avant: &image::RgbaImage,
    apres: &image::RgbaImage,
    ra: &[f64; 64],
    rb: &[f64; 64],
    elapsed_sec: f64,
    out_size: u32,
) -> image::RgbaImage {
    let sz = out_size.max(1);
    let melange = ease_out_quint(clamp01(elapsed_sec / SHAPE_MORPH_SEC));
    let rc: [f64; 64] = std::array::from_fn(|i| lerp(ra[i], rb[i], melange));
    let mut out = image::RgbaImage::new(sz, sz);
    let inv = 1.0 / f64::from(sz);
    for py in 0..sz {
        let qy = (f64::from(py) + 0.5) * inv * 2.0 - 1.0;
        for px in 0..sz {
            let qx = (f64::from(px) + 0.5) * inv * 2.0 - 1.0;
            let ang = qy.atan2(qx);
            let r_cur = radius_at_angle(&rc, ang).max(1e-6);
            let scale_a = radius_at_angle(ra, ang) / r_cur;
            let scale_b = radius_at_angle(rb, ang) / r_cur;
            let pa = sample_rgba_premul_bilinear(
                avant,
                (qx * scale_a + 1.0) * 0.5,
                (qy * scale_a + 1.0) * 0.5,
            );
            let pb = sample_rgba_premul_bilinear(
                apres,
                (qx * scale_b + 1.0) * 0.5,
                (qy * scale_b + 1.0) * 0.5,
            );
            let mixed = [
                lerp(pa[0], pb[0], melange),
                lerp(pa[1], pb[1], melange),
                lerp(pa[2], pb[2], melange),
                lerp(pa[3], pb[3], melange),
            ];
            out.put_pixel(px, py, image::Rgba(premul_to_rgba8(mixed)));
        }
    }
    out
}

fn grad_noise_2d(x: f64, y: f64, seed: u32) -> f64 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - f64::from(xi);
    let yf = y - f64::from(yi);
    let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let grad = |ix: i32, iy: i32| -> (f64, f64) {
        let mut h = ((ix as u32)
            .wrapping_mul(0x27d4_eb2d)
            .wrapping_add((iy as u32).wrapping_mul(0x1656_67b1)))
            ^ seed.wrapping_mul(0x9e37_79b9);
        h = (h ^ (h >> 15)).wrapping_mul(0x85eb_ca6b);
        let ang = (f64::from(h & 0xffff) / 65536.0) * TAU;
        (ang.cos(), ang.sin())
    };
    let dot_corner = |cx: i32, cy: i32| {
        let (gx, gy) = grad(cx, cy);
        gx * (x - f64::from(cx)) + gy * (y - f64::from(cy))
    };
    let u = fade(xf);
    let v = fade(yf);
    let nx0 = lerp(dot_corner(xi, yi), dot_corner(xi + 1, yi), u);
    let nx1 = lerp(dot_corner(xi, yi + 1), dot_corner(xi + 1, yi + 1), u);
    lerp(nx0, nx1, v)
}

fn fractal_noise_4oct_seed7(vb_x: f64, vb_y: f64) -> f64 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 0.05;
    for oct in 0..4_u32 {
        sum += amp * grad_noise_2d(vb_x * freq, vb_y * freq, 7_u32.wrapping_add(oct * 31));
        freq *= 2.0;
        amp *= 0.5;
    }
    (sum * 0.28 + 0.505).clamp(0.31, 0.70)
}

/// Deterministic 4-octave fractal-noise `Dissolve` wipe (`De = 0.12`, `plush.js:L412-L447`).
pub fn dissolve_blend_bodies(
    avant: &image::RgbaImage,
    apres: &image::RgbaImage,
    melange: f64,
    out_size: u32,
) -> image::RgbaImage {
    let sz = out_size.max(1);
    let slope = 1.0 / (0.4 * DISSOLVE_EDGE); // 20.833333333333332
    let intercept = (clamp01(melange) * 1.12 + 0.75) / DISSOLVE_EDGE;
    let mut out = image::RgbaImage::new(sz, sz);
    let inv = 1.0 / f64::from(sz);
    for py in 0..sz {
        let v = (f64::from(py) + 0.5) * inv;
        let vb_y = v * VIEW_BOX - DEMI_VIEWBOX;
        for px in 0..sz {
            let u = (f64::from(px) + 0.5) * inv;
            let vb_x = u * VIEW_BOX - DEMI_VIEWBOX;
            let n = fractal_noise_4oct_seed7(vb_x, vb_y);
            let wa = clamp01(slope * n + (1.0 - intercept));
            let wb = clamp01(-slope * n + intercept);
            let pa = sample_rgba_premul_bilinear(avant, u, v);
            let pb = sample_rgba_premul_bilinear(apres, u, v);
            let mut acc = [pa[0] * wa, pa[1] * wa, pa[2] * wa, pa[3] * wa];
            let sb = [pb[0] * wb, pb[1] * wb, pb[2] * wb, pb[3] * wb];
            let inv_sb = 1.0 - sb[3];
            for c in 0..4 {
                acc[c] = sb[c] + acc[c] * inv_sb;
            }
            out.put_pixel(px, py, image::Rgba(premul_to_rgba8(acc)));
        }
    }
    out
}

#[inline]
fn mul_affine(m1: &[f64; 6], m2: &[f64; 6]) -> [f64; 6] {
    [
        m1[0] * m2[0] + m1[2] * m2[1],
        m1[1] * m2[0] + m1[3] * m2[1],
        m1[0] * m2[2] + m1[2] * m2[3],
        m1[1] * m2[2] + m1[3] * m2[3],
        m1[0] * m2[4] + m1[2] * m2[5] + m1[4],
        m1[1] * m2[4] + m1[3] * m2[5] + m1[5],
    ]
}

#[inline]
fn invert_affine(m: &[f64; 6]) -> Option<[f64; 6]> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() <= 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let ia = m[3] * inv;
    let ib = -m[1] * inv;
    let ic = -m[2] * inv;
    let id = m[0] * inv;
    Some([
        ia,
        ib,
        ic,
        id,
        -(ia * m[4] + ic * m[5]),
        -(ib * m[4] + id * m[5]),
    ])
}

#[inline]
fn apply_affine(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

#[inline]
fn blend_over_premul(dst: &mut [f64; 4], src: [f64; 4]) {
    let inv_a = (1.0 - src[3]).clamp(0.0, 1.0);
    for i in 0..4 {
        dst[i] = (src[i] + dst[i] * inv_a).clamp(0.0, 1.0);
    }
}

#[inline]
fn rrect_inside(lx: f64, ly: f64, half_w: f64, half_h: f64, r: f64) -> f64 {
    let rr = r.min(half_w).min(half_h).max(0.0);
    let qx = lx.abs() - (half_w - rr);
    let qy = ly.abs() - (half_h - rr);
    let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rr;
    clamp01(0.5 - dist)
}

fn sample_pieces_isolated_plus_lighter(
    lx: f64,
    ly: f64,
    pieces: &[PlushPiece],
    tex_x: &image::RgbaImage,
    tex_y: &image::RgbaImage,
) -> [f64; 4] {
    let additive = pieces.len() > 1;
    let mut acc = [0.0_f64; 4];
    for piece in pieces {
        if piece.w <= 1e-9 || piece.h <= 1e-9 {
            continue;
        }
        if lx < piece.x || lx > piece.x + piece.w || ly < piece.y || ly > piece.y + piece.h {
            continue;
        }
        let u = piece.sx + ((lx - piece.x) / piece.w) * piece.sw;
        let v = piece.sy + ((ly - piece.y) / piece.h) * piece.sh;
        let tex = match piece.src {
            PadAxis::X => tex_x,
            PadAxis::Y => tex_y,
        };
        let s = sample_rgba_premul_bilinear(tex, u, v);
        if additive {
            for c in 0..4 {
                acc[c] = (acc[c] + s[c]).min(1.0);
            }
        } else {
            blend_over_premul(&mut acc, s);
        }
    }
    acc
}

fn rasterize_round_polyline(
    buf: &mut [[f64; 4]],
    sz: u32,
    pts_px: &[(f64, f64)],
    stroke_px: f64,
    rgb8: [u8; 3],
    opacity: f64,
) {
    if pts_px.len() < 2 || opacity <= 0.001 || stroke_px <= 0.01 {
        return;
    }
    let radius = stroke_px * 0.5;
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for &(x, y) in pts_px {
        min_x = min_x.min(x - radius - 1.5);
        max_x = max_x.max(x + radius + 1.5);
        min_y = min_y.min(y - radius - 1.5);
        max_y = max_y.max(y + radius + 1.5);
    }
    let x0 = min_x.floor().max(0.0) as u32;
    let x1 = max_x.ceil().min(f64::from(sz)) as u32;
    let y0 = min_y.floor().max(0.0) as u32;
    let y1 = max_y.ceil().min(f64::from(sz)) as u32;
    let cr = f64::from(rgb8[0]) / 255.0;
    let cg = f64::from(rgb8[1]) / 255.0;
    let cb = f64::from(rgb8[2]) / 255.0;
    for py in y0..y1 {
        let cy = f64::from(py) + 0.5;
        for px in x0..x1 {
            let cx = f64::from(px) + 0.5;
            let mut min_dist = f64::INFINITY;
            for seg in pts_px.windows(2) {
                let (ax, ay) = seg[0];
                let (bx, by) = seg[1];
                let dx = bx - ax;
                let dy = by - ay;
                let len_sq = dx * dx + dy * dy;
                let t = if len_sq <= 1e-9 {
                    0.0
                } else {
                    ((cx - ax) * dx + (cy - ay) * dy) / len_sq
                }
                .clamp(0.0, 1.0);
                let proj_x = ax + t * dx;
                let proj_y = ay + t * dy;
                min_dist = min_dist.min((cx - proj_x).hypot(cy - proj_y));
            }
            let cov = clamp01(radius + 0.5 - min_dist) * clamp01(opacity);
            if cov > 1e-4 {
                let idx = (py * sz + px) as usize;
                blend_over_premul(&mut buf[idx], [cr * cov, cg * cov, cb * cov, cov]);
            }
        }
    }
}

fn load_body_rgba(
    assets: &BundledPlushAssets,
    rel_path: &str,
    tint_hex: Option<&str>,
) -> Option<Arc<image::RgbaImage>> {
    match tint_hex {
        Some(hex) => assets.tinted_rgba(rel_path, hex, None).ok(),
        None => assets.decode_rgba(rel_path).ok(),
    }
}

fn blit_viewbox_sprite(
    buf: &mut [[f64; 4]],
    sz: u32,
    px_per_vb: f64,
    sprite: &image::RgbaImage,
    vb_x: f64,
    vb_y: f64,
    vb_w: f64,
    vb_h: f64,
    opacity: f64,
) {
    if vb_w <= 1e-6 || vb_h <= 1e-6 || opacity <= 0.001 {
        return;
    }
    let half_px = f64::from(sz) * 0.5;
    let x0_f = half_px + vb_x * px_per_vb;
    let y0_f = half_px + vb_y * px_per_vb;
    let w_f = vb_w * px_per_vb;
    let h_f = vb_h * px_per_vb;
    let x0 = x0_f.floor().max(0.0) as u32;
    let y0 = y0_f.floor().max(0.0) as u32;
    let x1 = (x0_f + w_f).ceil().min(f64::from(sz)) as u32;
    let y1 = (y0_f + h_f).ceil().min(f64::from(sz)) as u32;
    let op = clamp01(opacity);
    for py in y0..y1 {
        let v = (f64::from(py) + 0.5 - y0_f) / w_f.max(1e-6);
        let vy = (f64::from(py) + 0.5 - y0_f) / h_f.max(1e-6);
        let _ = v;
        if !(0.0..=1.0).contains(&vy) {
            continue;
        }
        for px in x0..x1 {
            let ux = (f64::from(px) + 0.5 - x0_f) / w_f.max(1e-6);
            if !(0.0..=1.0).contains(&ux) {
                continue;
            }
            let mut s = sample_rgba_premul_bilinear(sprite, ux, vy);
            for ch in &mut s {
                *ch *= op;
            }
            blend_over_premul(&mut buf[(py * sz + px) as usize], s);
        }
    }
}

/// CPU RGBA8 compositor for `PlushFrameSample` with isolated `plus-lighter` 9-slice pads and 4-band `lid_mask`.
pub fn composite_plush_rgba_with_assets(
    sample: &PlushFrameSample,
    pixel_size: u32,
    saturation: f64,
    assets: &BundledPlushAssets,
) -> image::RgbaImage {
    let sz = pixel_size.max(8);
    let px_per_vb = f64::from(sz) / VIEW_BOX;
    let half_px = f64::from(sz) * 0.5;
    let mut buf = vec![[0.0_f64; 4]; (sz * sz) as usize];

    let body_sx = sample.transition_sample.sx;
    let body_sy = sample.transition_sample.sy * sample.breath_sy;
    let body_alpha = clamp01(sample.body_alpha);

    if body_alpha > 0.001 && body_sx.abs() > 1e-4 && body_sy.abs() > 1e-4 {
        let avant_img = load_body_rgba(
            assets,
            &sample.avant_body_asset,
            sample.avant_ink_hex.as_deref(),
        );
        let apres_img = load_body_rgba(
            assets,
            &sample.apres_body_asset,
            sample.apres_ink_hex.as_deref(),
        );
        let blended_body: Option<Arc<image::RgbaImage>> = match sample.transition_sample.corps {
            TransitionBodyStage::Avant => avant_img.or(apres_img),
            TransitionBodyStage::Apres => apres_img.or(avant_img),
            TransitionBodyStage::Deux => match (avant_img, apres_img) {
                (Some(av), Some(ap)) if sample.transition == PlushTransition::Warp => Some(
                    Arc::new(warp_blend_bodies(
                        &av,
                        &ap,
                        &sample.avant_radii,
                        &sample.apres_radii,
                        sample.transition_elapsed_sec,
                        sz,
                    )),
                ),
                (Some(av), Some(ap)) if sample.transition == PlushTransition::Dissolve => Some(
                    Arc::new(dissolve_blend_bodies(
                        &av,
                        &ap,
                        sample.transition_sample.melange,
                        sz,
                    )),
                ),
                (_, ap) => ap,
            },
        };
        if let Some(body_tex) = blended_body {
            for py in 0..sz {
                let vb_y = ((f64::from(py) + 0.5 - half_px) / px_per_vb) / body_sy;
                let v = (vb_y + DEMI_VIEWBOX) / VIEW_BOX;
                if !(0.0..=1.0).contains(&v) {
                    continue;
                }
                for px in 0..sz {
                    let vb_x = ((f64::from(px) + 0.5 - half_px) / px_per_vb) / body_sx;
                    let u = (vb_x + DEMI_VIEWBOX) / VIEW_BOX;
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let mut p = sample_rgba_premul_bilinear(&body_tex, u, v);
                    for ch in &mut p {
                        *ch *= body_alpha;
                    }
                    buf[(py * sz + px) as usize] = p;
                }
            }
        }

        let vp_body = [
            px_per_vb * body_sx,
            0.0,
            0.0,
            px_per_vb * body_sy,
            half_px,
            half_px,
        ];
        for eye in &sample.eyes {
            paint_plush_eye_layer(&mut buf, sz, &vp_body, eye, sample.eye_scale, body_alpha, assets);
        }
    }

    if !sample.busy_dots.is_empty() {
        if let Ok(dot_tex) = assets.tinted_rgba("bloub/boule.webp", &sample.ink_hex, None) {
            for dot in &sample.busy_dots {
                blit_viewbox_sprite(&mut buf, sz, px_per_vb, &dot_tex, dot.x, dot.y, dot.size, dot.size, dot.opacity);
            }
        }
    }
    if let Some(notif) = sample.notif_ball {
        if let Ok(notif_tex) = assets.tinted_rgba("bloub/boule.webp", NOTIFY_COLOR, None) {
            blit_viewbox_sprite(&mut buf, sz, px_per_vb, &notif_tex, notif.x, notif.y, notif.size, notif.size, notif.opacity);
        }
    }
    if !sample.sleep_zs.is_empty() {
        let ink_rgb = parse_hex_rgb(&sample.ink_hex);
        for z in &sample.sleep_zs {
            let pts = [
                (half_px + (z.x - z.half) * px_per_vb, half_px + (z.y - z.half) * px_per_vb),
                (half_px + (z.x + z.half) * px_per_vb, half_px + (z.y - z.half) * px_per_vb),
                (half_px + (z.x - z.half) * px_per_vb, half_px + (z.y + z.half) * px_per_vb),
                (half_px + (z.x + z.half) * px_per_vb, half_px + (z.y + z.half) * px_per_vb),
            ];
            rasterize_round_polyline(&mut buf, sz, &pts, z.width * px_per_vb, ink_rgb, z.opacity);
        }
    }

    let mut out = image::RgbaImage::new(sz, sz);
    for (idx, &p) in buf.iter().enumerate() {
        let px = (idx as u32) % sz;
        let py = (idx as u32) / sz;
        out.put_pixel(px, py, image::Rgba(premul_to_rgba8(p)));
    }
    if (saturation - 1.0).abs() > 1e-4 {
        desaturate_rgba_image(&out, saturation)
    } else {
        out
    }
}

fn paint_plush_eye_layer(
    buf: &mut [[f64; 4]],
    sz: u32,
    vp_body: &[f64; 6],
    eye: &PlushEyeLayer,
    eye_scale: f64,
    body_alpha: f64,
    assets: &BundledPlushAssets,
) {
    if eye.transform.culled || eye.transform.alpha <= 0.001 {
        return;
    }
    let trans_mat = [1.0, 0.0, 0.0, 1.0, eye.translate_x, eye.translate_y];
    let scale_mat = [eye_scale, 0.0, 0.0, eye_scale, 0.0, 0.0];
    let c_mat = mul_affine(&mul_affine(&trans_mat, &eye.transform.matrix), &scale_mat);
    let m_pad_vb = match eye.inner_unsquash {
        Some(inner) => mul_affine(&c_mat, &inner),
        None => c_mat,
    };
    let t_pad = mul_affine(vp_body, &m_pad_vb);

    let pad_alpha = clamp01(eye.transform.alpha * eye.pad_opacity * body_alpha);
    if pad_alpha > 0.001 && !eye.pieces.is_empty() {
        if let (Ok(tex_x), Ok(tex_y), Some(inv_pad)) = (
            assets.decode_rgba(eye.pad.file_x),
            assets.decode_rgba(eye.pad.file_y),
            invert_affine(&t_pad),
        ) {
            let fade_tex = eye.fade_from_pad.and_then(|fp| {
                Some((
                    assets.decode_rgba(fp.file_x).ok()?,
                    assets.decode_rgba(fp.file_y).ok()?,
                ))
            });
            let inv_lid = eye.lid.as_ref().and_then(|lid| {
                let cx = eye.translate_x + lid.cx;
                let cy = eye.translate_y + lid.cy;
                let lid_squash = [1.0, 0.0, 0.0, lid.s, 0.0, cy * (1.0 - lid.s)];
                let _ = cx;
                let m_lid_vb = mul_affine(&lid_squash, &m_pad_vb);
                invert_affine(&mul_affine(vp_body, &m_lid_vb))
            });

            let (ext_x, ext_y) = pad_extent(&eye.pieces);
            let corners = [
                apply_affine(&t_pad, -ext_x, -ext_y),
                apply_affine(&t_pad, ext_x, -ext_y),
                apply_affine(&t_pad, -ext_x, ext_y),
                apply_affine(&t_pad, ext_x, ext_y),
            ];
            let mut min_x = f64::INFINITY;
            let mut max_x = f64::NEG_INFINITY;
            let mut min_y = f64::INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for (cx, cy) in corners {
                min_x = min_x.min(cx - 1.0);
                max_x = max_x.max(cx + 1.0);
                min_y = min_y.min(cy - 1.0);
                max_y = max_y.max(cy + 1.0);
            }
            let x0 = min_x.floor().max(0.0) as u32;
            let x1 = max_x.ceil().min(f64::from(sz)) as u32;
            let y0 = min_y.floor().max(0.0) as u32;
            let y1 = max_y.ceil().min(f64::from(sz)) as u32;

            for py in y0..y1 {
                let sy = f64::from(py) + 0.5;
                for px in x0..x1 {
                    let sx = f64::from(px) + 0.5;
                    let (lx, ly) = apply_affine(&inv_pad, sx, sy);
                    let mut isolated =
                        sample_pieces_isolated_plus_lighter(lx, ly, &eye.pieces, &tex_x, &tex_y);
                    if let Some((ref fx, ref fy)) = fade_tex {
                        let prev = sample_pieces_isolated_plus_lighter(
                            lx,
                            ly,
                            &eye.fade_from_pieces,
                            fx,
                            fy,
                        );
                        let pf = clamp01(eye.pad_fade);
                        for ch in 0..4 {
                            isolated[ch] = lerp(prev[ch], isolated[ch], pf);
                        }
                    }
                    if isolated[3] <= 1e-5 {
                        continue;
                    }
                    if let (Some(lid), Some(inv_l)) = (&eye.lid, &inv_lid) {
                        let (mx, my) = apply_affine(inv_l, sx, sy);
                        let mut mask_a = 0.0_f64;
                        for band in &lid.bands {
                            let cov = rrect_inside(mx, my, band.w, band.h, band.r) * band.alpha;
                            mask_a += cov * (1.0 - mask_a);
                        }
                        for ch in &mut isolated {
                            *ch *= clamp01(mask_a);
                        }
                    }
                    for ch in &mut isolated {
                        *ch *= pad_alpha;
                    }
                    if isolated[3] > 1e-5 {
                        blend_over_premul(&mut buf[(py * sz + px) as usize], isolated);
                    }
                }
            }
        }
    }

    if let Some(closed) = eye.closed_arc {
        let arc_alpha = clamp01(eye.transform.alpha * closed.opacity * body_alpha);
        if arc_alpha > 0.001 {
            let t_arc = mul_affine(vp_body, &c_mat);
            let steps = 16_usize;
            let half_w = closed.w * 0.5;
            let mut pts = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let t = (i as f64) / (steps as f64);
                let inv_t = 1.0 - t;
                let lx = inv_t * inv_t * (-half_w) + t * t * half_w;
                let ly = 2.0 * inv_t * t * closed.bend_y;
                pts.push(apply_affine(&t_arc, lx, ly));
            }
            let scale_est = t_arc[0].hypot(t_arc[1]).max(t_arc[2].hypot(t_arc[3]));
            rasterize_round_polyline(
                buf,
                sz,
                &pts,
                closed.stroke_width * scale_est,
                parse_hex_rgb(closed.color),
                arc_alpha,
            );
        }
    }
}

pub fn composite_plush_rgba(sample: &PlushFrameSample, pixel_size: u32) -> image::RgbaImage {
    composite_plush_rgba_with_assets(sample, pixel_size, 1.0, BundledPlushAssets::global())
}

fn cached_plush_render_image(sample: &PlushFrameSample, pixel_size: u32) -> Arc<gpui::RenderImage> {
    static FRAME_CACHE: OnceLock<Mutex<BoundedCache<Arc<gpui::RenderImage>>>> = OnceLock::new();
    let cache = FRAME_CACHE.get_or_init(|| Mutex::new(BoundedCache::new(MAX_FRAME_CACHE_ENTRIES)));
    let t_bucket = (sample.time_sec * 30.0).round() as i64;
    let tr_bucket = (sample.transition_elapsed_sec * 60.0).round() as i64;
    let key = format!(
        "{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{:.2}|{}",
        sample.character,
        sample.shape,
        sample.state,
        sample.expression,
        sample.ink_hex,
        t_bucket,
        tr_bucket,
        sample.eye_scale,
        pixel_size
    );
    if let Some(hit) = cache.lock().get(&key) {
        return hit;
    }
    let rgba = composite_plush_rgba(sample, pixel_size);
    let rendered = rgba_to_gpui_render_image(rgba);
    cache.lock().insert(key, rendered.clone());
    rendered
}

/// Renders a `PlushFrameSample` into a GPUI `Div` with exact `128% × 1.15` optical framing.
pub fn render_plush_avatar_element(sample: &PlushFrameSample, size_px: f32) -> gpui::Div {
    let safe_size = if size_px.is_finite() && size_px > 0.0 {
        size_px
    } else {
        32.0
    };
    let inner_px = safe_size * 1.28 * (PLUSH_BODY_SCALE as f32);
    let offset_px = (safe_size - inner_px) * 0.5;
    let target_pixels = plush_pixels(inner_px, 2.0);
    let render_img = cached_plush_render_image(sample, target_pixels);

    div()
        .relative()
        .w(px(safe_size))
        .h(px(safe_size))
        .flex_shrink_0()
        .child(
            img(render_img)
                .absolute()
                .left(px(offset_px))
                .top(px(offset_px))
                .w(px(inner_px))
                .h(px(inner_px))
                .object_fit(ObjectFit::Contain),
        )
}

pub fn render_plush_seed_avatar(
    seed: &str,
    hue: Option<AvatarHue>,
    color: Option<&str>,
    mood: AvatarMood,
    now_sec: f64,
    reduced_motion: bool,
    size_px: f32,
) -> gpui::Div {
    let mut renderer = PlushFeltRenderer::for_seed(seed, hue, color, mood);
    renderer.set_eye_scale(small_size_eye_scale(size_px));
    let sample = renderer.sample_plush_frame(now_sec, reduced_motion);
    render_plush_avatar_element(&sample, size_px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_plush_assets_has_all_73_webp_and_fishing_webm() {
        let assets = BundledPlushAssets::global();
        let found = assets.bundled_webp_paths();
        assert_eq!(found.len(), EXPECTED_BUNDLED_WEBP_COUNT);
        for rel in BundledPlushAssets::bundled_relative_webp_paths() {
            assert!(assets.resolve_asset_path(rel).is_some(), "missing {rel}");
        }
        assert!(assets.resolve_asset_path("ui/fishing.webm").is_some());
    }

    #[test]
    fn tint_ramp_matches_upstream_golden_vectors_bit_for_bit() {
        let red = tint_ramp("#e8483f");
        assert_eq!(red[0], [0, 0, 0, 255]);
        assert_eq!(red[64], [149, 43, 37, 255]);
        assert_eq!(red[128], [220, 68, 59, 255]);
        assert_eq!(red[140], [232, 72, 63, 255]);
        assert_eq!(red[200], [255, 98, 89, 255]);
        assert_eq!(red[255], [255, 127, 118, 255]);

        let orange = tint_ramp("#f08a24");
        assert_eq!(orange[0], [0, 0, 0, 255]);
        assert_eq!(orange[64], [155, 87, 19, 255]);
        assert_eq!(orange[128], [228, 131, 34, 255]);
        assert_eq!(orange[140], [240, 138, 36, 255]);
        assert_eq!(orange[200], [255, 173, 63, 255]);
        assert_eq!(orange[255], [255, 204, 96, 255]);

        let notif = tint_ramp("#2496e8");
        assert_eq!(notif[0], [0, 0, 0, 255]);
        assert_eq!(notif[64], [19, 95, 149, 255]);
        assert_eq!(notif[128], [34, 142, 220, 255]);
        assert_eq!(notif[140], [36, 150, 232, 255]);
        assert_eq!(notif[200], [63, 187, 255, 255]);
        assert_eq!(notif[255], [96, 219, 255, 255]);
    }

    #[test]
    fn characters_pads_lid_and_eye_scale_behave_deterministically() {
        assert_eq!(PlushCharacterId::ALL.len(), 6);
        for ch in PlushCharacterId::ALL {
            assert_eq!(PlushCharacterId::parse(ch.as_str()), Some(ch));
            let spec = ch.spec();
            let pieces = pad_pieces(&spec.left_pad, spec.eyes[0].w * 100.0, spec.eyes[0].h * 100.0);
            assert!(pieces.len() >= 2 && pieces.len() % 2 == 0);
        }
        assert!((GENERIC_PAD.kw - 3.823213092207733).abs() < 1e-12);
        assert!((GENERIC_PAD.kh - 3.5378116570986435).abs() < 1e-12);

        let m = [1.0, 0.0, 0.0, 0.4, -20.0, -5.0];
        assert!(unsquash_matrix(&m, 0.4).is_some());
        let lid = lid_mask(&m, 0.4, 1.0, (10.0, 20.0), (40.0, 50.0)).expect("lid active");
        assert_eq!(lid.bands.len(), 4);
        assert!((lid.bands[0].alpha - 0.25).abs() < 1e-9);
        assert!((lid.bands[3].alpha - 1.0).abs() < 1e-9);

        assert!((small_size_eye_scale(18.0) - 1.30).abs() < 1e-9);
        assert!((small_size_eye_scale(36.0) - 1.0).abs() < 1e-9);
        let offs = eye_scale_offsets(
            &[([1.0, 0.0, 0.0, 1.0, 10.0, 0.0], 18.0, 40.0), ([1.0, 0.0, 0.0, 1.0, 36.0, 0.0], 18.0, 40.0)],
            FACE_CHOICE_EYE_SCALE,
        );
        assert!(offs[0].0 < 0.0 && offs[1].0 < 0.0);
    }

    #[test]
    fn plush_states_transitions_and_rgba_compositor_cover_all_variants() {
        let mut renderer = PlushFeltRenderer::for_character(PlushCharacterId::Nexo, PlushState::Idle);
        let idle = renderer.sample_plush_frame(1.0, false);
        assert_eq!(idle.pad_blend, "plus-lighter");
        assert!(idle.busy_dots.is_empty());

        renderer.set_state(PlushState::Thinking, 1.0);
        let thinking = renderer.sample_plush_frame(1.4, false);
        assert_eq!(thinking.busy_dots.len(), 3);

        renderer.set_state(PlushState::Notify, 2.0);
        let notify = renderer.sample_plush_frame(2.5, false);
        assert_eq!(notify.notify_color, Some("#2496e8"));
        assert!(notify.notif_ball.is_some());

        renderer.set_state(PlushState::Asleep, 3.0);
        let asleep = renderer.sample_plush_frame(4.2, false);
        assert!(!asleep.sleep_zs.is_empty());
        assert!(asleep.eyes[0].closed_arc.is_some());

        renderer.set_state(PlushState::Failed, 5.0);
        let failed = renderer.sample_plush_frame(5.15, false);
        assert!(failed.shake_deg.abs() > 0.1);

        for tr in [
            PlushTransition::Flip,
            PlushTransition::Pop,
            PlushTransition::Warp,
            PlushTransition::Dissolve,
            PlushTransition::Cut,
        ] {
            let mid = sample_transition(tr, transition_duration(tr) * 0.5);
            assert!(!mid.fini);
            let done = sample_transition(tr, transition_duration(tr) + 0.01);
            assert!(done.fini && done.bascule);
        }

        let img = composite_plush_rgba(&thinking, 64);
        assert_eq!(img.dimensions(), (64, 64));
        assert!(img.pixels().any(|p| p.0[3] > 0));
    }
}
