//! OpenBot Plush Mascots, `ServerPlushLogo`, `PlushMascot` & `PlushLoader`,
//! `GlobalSearchPlushState`, `MarketplaceNoMatchPlushState`, `recovery_plush_svg`,
//! and Interactive `AppLogo` (`Production` / `Dev` / `Preview` + 5-Click `"party"` Easter Egg).
//!
//! 1:1 port of `packages/ui/src/{server-plush-logo,plush-loader}.tsx`,
//! `packages/ui/src/components/GlobalSearchPlush.tsx`,
//! `packages/ui/src/features/marketplace/MarketplaceNoMatchPlush.tsx`,
//! `src/main/start-recovery-plush.ts`, and `packages/brand/src/{AppLogo.tsx,app-logo-shape.ts,logo.css}`.

use std::sync::Arc;

use base64::Engine as _;
use gpui::{
    Bounds, ObjectFit, ParentElement, PathBuilder, Pixels, Point, Styled, StyledImage, Window,
    canvas, div, img, point, px, rgb,
};
use openbot_contracts::avatar::{ServerPlushColor, server_plush_color, server_plush_hex};
use serde::{Deserialize, Serialize};

use super::bot_engine::PointerKind;
use super::plush_felt::{BundledPlushAssets, rgba_to_gpui_render_image};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerAppearanceShape {
    Squircle,
    Cercle,
    Galet,
    Capsule,
    Carre,
    Losange,
    Triangle,
    Pentagone,
    Hexagone,
    Nuage,
}

impl ServerAppearanceShape {
    pub const ALL: [Self; 10] = [
        Self::Squircle,
        Self::Cercle,
        Self::Galet,
        Self::Capsule,
        Self::Carre,
        Self::Losange,
        Self::Triangle,
        Self::Pentagone,
        Self::Hexagone,
        Self::Nuage,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Squircle => "squircle",
            Self::Cercle => "cercle",
            Self::Galet => "galet",
            Self::Capsule => "capsule",
            Self::Carre => "carre",
            Self::Losange => "losange",
            Self::Triangle => "triangle",
            Self::Pentagone => "pentagone",
            Self::Hexagone => "hexagone",
            Self::Nuage => "nuage",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|s| s.as_str() == lower)
    }

    pub const fn grey_asset_path(self) -> &'static str {
        match self {
            Self::Squircle => "ui/server/shape/squircle.webp",
            Self::Cercle => "ui/server/shape/cercle.webp",
            Self::Galet => "ui/server/shape/galet.webp",
            Self::Capsule => "ui/server/shape/capsule.webp",
            Self::Carre => "ui/server/shape/carre.webp",
            Self::Losange => "ui/server/shape/losange.webp",
            Self::Triangle => "ui/server/shape/triangle.webp",
            Self::Pentagone => "ui/server/shape/pentagone.webp",
            Self::Hexagone => "ui/server/shape/hexagone.webp",
            Self::Nuage => "ui/server/shape/nuage.webp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerAppearance {
    pub shape: ServerAppearanceShape,
    pub color: Option<String>,
}

impl ServerAppearance {
    pub fn is_valid_color(hex: &str) -> bool {
        hex.len() == 7
            && hex.starts_with('#')
            && hex.as_bytes()[1..].iter().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }

    pub fn is_automatic(&self) -> bool {
        self.shape == ServerAppearanceShape::Squircle && self.color.is_none()
    }
}

pub const fn server_color_asset_path(color: ServerPlushColor) -> &'static str {
    match color {
        ServerPlushColor::Rouge => "ui/server/rouge.webp",
        ServerPlushColor::Orange => "ui/server/orange.webp",
        ServerPlushColor::Ambre => "ui/server/ambre.webp",
        ServerPlushColor::Vert => "ui/server/vert.webp",
        ServerPlushColor::Turquoise => "ui/server/turquoise.webp",
        ServerPlushColor::Bleu => "ui/server/bleu.webp",
        ServerPlushColor::Violet => "ui/server/violet.webp",
        ServerPlushColor::Rose => "ui/server/rose.webp",
    }
}

/// `ServerPlushLogo` (`packages/ui/src/server-plush-logo.tsx:L26-L128`).
#[derive(Debug, Clone, PartialEq)]
pub struct ServerPlushLogo {
    pub seed: String,
    pub appearance: Option<ServerAppearance>,
    pub size_px: f32,
}

impl ServerPlushLogo {
    pub fn new(seed: impl Into<String>) -> Self {
        Self {
            seed: seed.into(),
            appearance: None,
            size_px: 36.0,
        }
    }

    pub fn with_appearance(mut self, appearance: Option<ServerAppearance>) -> Self {
        self.appearance = appearance;
        self
    }

    pub fn with_size(mut self, size_px: f32) -> Self {
        self.size_px = if size_px.is_finite() && size_px > 0.0 { size_px } else { 36.0 };
        self
    }

    pub fn is_automatic(&self) -> bool {
        match &self.appearance {
            None => true,
            Some(app) => app.is_automatic(),
        }
    }

    pub fn automatic_asset_path(&self) -> &'static str {
        server_color_asset_path(server_plush_color(&self.seed))
    }

    pub fn resolved_hex(&self) -> String {
        let custom = self.appearance.as_ref().and_then(|a| a.color.as_deref());
        server_plush_hex(&self.seed, custom)
    }

    pub fn resolve_rgba(&self, assets: &BundledPlushAssets) -> Result<Arc<image::RgbaImage>, String> {
        let auto_path = self.automatic_asset_path();
        let Some(ref app) = self.appearance else {
            return assets.decode_rgba(auto_path);
        };
        if app.is_automatic() {
            return assets.decode_rgba(auto_path);
        }
        let hex = self.resolved_hex();
        assets
            .tinted_rgba(app.shape.grey_asset_path(), &hex, None)
            .or_else(|_| assets.decode_rgba(auto_path))
    }

    pub fn resolve_render_image(
        &self,
        assets: &BundledPlushAssets,
    ) -> Result<Arc<gpui::RenderImage>, String> {
        let auto_path = self.automatic_asset_path();
        let Some(ref app) = self.appearance else {
            return assets.render_image(auto_path);
        };
        if app.is_automatic() {
            return assets.render_image(auto_path);
        }
        let hex = self.resolved_hex();
        assets
            .tinted_render_image(app.shape.grey_asset_path(), &hex, None)
            .or_else(|_| assets.render_image(auto_path))
    }

    pub fn render_element(&self) -> gpui::Div {
        render_server_plush_logo(self)
    }
}

pub fn render_server_plush_logo(logo: &ServerPlushLogo) -> gpui::Div {
    let sz = logo.size_px.max(8.0);
    let container = div().relative().w(px(sz)).h(px(sz)).flex_shrink_0();
    match logo.resolve_render_image(BundledPlushAssets::global()) {
        Ok(image) => container.child(img(image).w(px(sz)).h(px(sz)).object_fit(ObjectFit::Contain)),
        Err(_) => container,
    }
}

pub const PLUSH_MASCOT_BODY_ASSET: &str = "ui/body-lilac.webp";
pub const PLUSH_MASCOT_EYE_LEFT_ASSET: &str = "ui/eye-left.webp";
pub const PLUSH_MASCOT_EYE_RIGHT_ASSET: &str = "ui/eye-right.webp";
pub const PLUSH_MASCOT_LEFT_EYE_ORIGIN: (f32, f32) = (0.356, 0.481);
pub const PLUSH_MASCOT_RIGHT_EYE_ORIGIN: (f32, f32) = (0.644, 0.481);
pub const PLUSH_LOADER_BALL_ASSETS: [&str; 3] = [
    "ui/ball-lilac.webp",
    "ui/ball-mint.webp",
    "ui/ball-pink.webp",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlushMascotFace {
    Idle,
    Wide,
    Wink,
    Happy,
}

impl PlushMascotFace {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Wide => "wide",
            Self::Wink => "wink",
            Self::Happy => "happy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlushMascotPose {
    pub body_scale: (f32, f32),
    pub eyes_translate_px: (f32, f32),
    pub left_eye_scale: (f32, f32),
    pub right_eye_scale: (f32, f32),
    pub eye_translate_y_frac: f32,
}

/// `PlushMascot` (`packages/ui/src/plush-loader.tsx:L14-L26`, `primitives.css:L2079-L2266`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlushMascot {
    pub face: PlushMascotFace,
    pub size_px: f32,
    pub extra_eye_offset_px: (f32, f32),
    pub body_rotate_deg: f32,
    pub body_translate_px: (f32, f32),
}

impl Default for PlushMascot {
    fn default() -> Self {
        Self::new(32.0)
    }
}

impl PlushMascot {
    pub fn new(size_px: f32) -> Self {
        Self {
            face: PlushMascotFace::Idle,
            size_px: if size_px.is_finite() && size_px > 0.0 { size_px } else { 32.0 },
            extra_eye_offset_px: (0.0, 0.0),
            body_rotate_deg: 0.0,
            body_translate_px: (0.0, 0.0),
        }
    }

    pub fn with_face(mut self, face: PlushMascotFace) -> Self {
        self.face = face;
        self
    }

    pub fn sample_pose(
        &self,
        elapsed_ms: f64,
        in_loader: bool,
        reduced_motion: bool,
    ) -> PlushMascotPose {
        let base_scale = if self.face == PlushMascotFace::Wide { 1.18_f32 } else { 1.0_f32 };
        if reduced_motion {
            return PlushMascotPose {
                body_scale: (1.0, 1.0),
                eyes_translate_px: self.extra_eye_offset_px,
                left_eye_scale: (base_scale, base_scale),
                right_eye_scale: (base_scale, base_scale),
                eye_translate_y_frac: 0.0,
            };
        }

        let breathe = if elapsed_ms >= 600.0 {
            let p = (((elapsed_ms - 600.0) % 3_200.0) / 3_200.0) as f32;
            let w = 0.5 - 0.5 * (p * std::f32::consts::TAU).cos();
            (1.0 + 0.012 * w, 1.0 - 0.022 * w)
        } else {
            (1.0, 1.0)
        };

        let mut blink_sy = 1.0_f32;
        if elapsed_ms >= 1_800.0 {
            let p = (((elapsed_ms - 1_800.0) % 5_200.0) / 5_200.0) as f32;
            if (0.93..0.96).contains(&p) {
                let u = (p - 0.93) / 0.03;
                blink_sy = 1.0 - 0.9 * u;
            } else if (0.96..=1.0).contains(&p) {
                let u = (p - 0.96) / 0.04;
                blink_sy = 0.1 + 0.9 * u;
            }
        }

        let look_unit = self.size_px * 0.035;
        let mut look_xy = (0.0_f32, 0.0_f32);
        if in_loader && elapsed_ms >= 700.0 {
            let p = (((elapsed_ms - 700.0) % 1_200.0) / 1_200.0) as f32;
            let x_mult = if p <= 0.18 {
                -0.9
            } else if p <= 0.32 {
                -0.9 + ((p - 0.18) / 0.14) * 0.9
            } else if p <= 0.46 {
                ((p - 0.32) / 0.14) * 0.9
            } else if p <= 0.70 {
                0.9
            } else {
                0.9 - ((p - 0.70) / 0.30) * 1.8
            };
            look_xy = (look_unit * x_mult, look_unit * 0.6);
        } else if !in_loader && elapsed_ms >= 2_200.0 {
            let p = (((elapsed_ms - 2_200.0) % 6_000.0) / 6_000.0) as f32;
            if (0.34..=0.50).contains(&p) {
                look_xy = (-look_unit, look_unit * 0.2);
            } else if (0.58..=0.78).contains(&p) {
                look_xy = (look_unit, -look_unit * 0.3);
            }
        }

        let mut left_sy = base_scale * blink_sy;
        let mut right_sy = base_scale * blink_sy;
        let mut eye_ty = 0.0_f32;
        match self.face {
            PlushMascotFace::Wink if elapsed_ms <= 600.0 => {
                let p = (elapsed_ms / 600.0).clamp(0.0, 1.0) as f32;
                let w = if p < 0.30 {
                    1.0 - 0.9 * (p / 0.30)
                } else if p <= 0.60 {
                    0.1
                } else {
                    0.1 + 0.9 * ((p - 0.60) / 0.40)
                };
                left_sy = base_scale * w;
            }
            PlushMascotFace::Happy => {
                let u = (elapsed_ms / 220.0).clamp(0.0, 1.0) as f32;
                let sy = 1.0 - 0.65 * u;
                left_sy = base_scale * sy;
                right_sy = base_scale * sy;
                eye_ty = -0.015 * u;
            }
            _ => {}
        }

        PlushMascotPose {
            body_scale: breathe,
            eyes_translate_px: (
                look_xy.0 + self.extra_eye_offset_px.0,
                look_xy.1 + self.extra_eye_offset_px.1,
            ),
            left_eye_scale: (base_scale, left_sy),
            right_eye_scale: (base_scale, right_sy),
            eye_translate_y_frac: eye_ty,
        }
    }

    pub fn composite_rgba(
        &self,
        assets: &BundledPlushAssets,
        elapsed_ms: f64,
        in_loader: bool,
        reduced_motion: bool,
    ) -> Result<image::RgbaImage, String> {
        let body = assets.decode_rgba(PLUSH_MASCOT_BODY_ASSET)?;
        let eye_l = assets.decode_rgba(PLUSH_MASCOT_EYE_LEFT_ASSET)?;
        let eye_r = assets.decode_rgba(PLUSH_MASCOT_EYE_RIGHT_ASSET)?;
        let out_px = (self.size_px.max(16.0).ceil() as u32).clamp(16, 512);
        let pose = self.sample_pose(elapsed_ms, in_loader, reduced_motion);

        let mut canvas_img = image::RgbaImage::new(out_px, out_px);
        let sz_f = out_px as f32;
        let rot_rad = self.body_rotate_deg.to_radians();
        let (cos_r, sin_r) = (rot_rad.cos(), rot_rad.sin());
        let pivot = (sz_f * 0.5, sz_f);

        let sample_layer = |img: &image::RgbaImage, u: f32, v: f32| -> [f32; 4] {
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                return [0.0; 4];
            }
            let x = ((u * (img.width() as f32 - 1.0)).round() as u32).min(img.width() - 1);
            let y = ((v * (img.height() as f32 - 1.0)).round() as u32).min(img.height() - 1);
            let p = img.get_pixel(x, y).0;
            let a = f32::from(p[3]) / 255.0;
            [
                (f32::from(p[0]) / 255.0) * a,
                (f32::from(p[1]) / 255.0) * a,
                (f32::from(p[2]) / 255.0) * a,
                a,
            ]
        };
        let over = |dst: &mut [f32; 4], src: [f32; 4]| {
            let inv = (1.0 - src[3]).clamp(0.0, 1.0);
            for i in 0..4 {
                dst[i] = (src[i] + dst[i] * inv).clamp(0.0, 1.0);
            }
        };

        let scale_ratio = sz_f / self.size_px.max(1.0);
        let btx = self.body_translate_px.0 * scale_ratio;
        let bty = self.body_translate_px.1 * scale_ratio;
        let etx = (pose.eyes_translate_px.0 * scale_ratio) / sz_f;
        let ety = (pose.eyes_translate_px.1 * scale_ratio) / sz_f + pose.eye_translate_y_frac;

        for py in 0..out_px {
            for px_i in 0..out_px {
                let sx = (px_i as f32 + 0.5) - btx - pivot.0;
                let sy = (py as f32 + 0.5) - bty - pivot.1;
                let rx = (sx * cos_r + sy * sin_r) / pose.body_scale.0.max(0.05) + pivot.0;
                let ry = (-sx * sin_r + sy * cos_r) / pose.body_scale.1.max(0.05) + pivot.1;
                let u = rx / sz_f;
                let v = ry / sz_f;
                let mut acc = sample_layer(&body, u, v);

                let lu = (u - etx - PLUSH_MASCOT_LEFT_EYE_ORIGIN.0)
                    / pose.left_eye_scale.0.max(0.04)
                    + PLUSH_MASCOT_LEFT_EYE_ORIGIN.0;
                let lv = (v - ety - PLUSH_MASCOT_LEFT_EYE_ORIGIN.1)
                    / pose.left_eye_scale.1.max(0.04)
                    + PLUSH_MASCOT_LEFT_EYE_ORIGIN.1;
                over(&mut acc, sample_layer(&eye_l, lu, lv));

                let ru = (u - etx - PLUSH_MASCOT_RIGHT_EYE_ORIGIN.0)
                    / pose.right_eye_scale.0.max(0.04)
                    + PLUSH_MASCOT_RIGHT_EYE_ORIGIN.0;
                let rv = (v - ety - PLUSH_MASCOT_RIGHT_EYE_ORIGIN.1)
                    / pose.right_eye_scale.1.max(0.04)
                    + PLUSH_MASCOT_RIGHT_EYE_ORIGIN.1;
                over(&mut acc, sample_layer(&eye_r, ru, rv));

                if acc[3] > 1e-4 {
                    let a = acc[3].clamp(0.0, 1.0);
                    canvas_img.put_pixel(
                        px_i,
                        py,
                        image::Rgba([
                            ((acc[0] / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                            ((acc[1] / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                            ((acc[2] / a).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                            (a * 255.0 + 0.5) as u8,
                        ]),
                    );
                }
            }
        }
        Ok(canvas_img)
    }

    pub fn render_element(&self, elapsed_ms: f64, reduced_motion: bool) -> gpui::Div {
        let sz = self.size_px.max(8.0);
        let root = div().relative().w(px(sz)).h(px(sz)).flex_shrink_0();
        match self.composite_rgba(BundledPlushAssets::global(), elapsed_ms, false, reduced_motion) {
            Ok(rgba) => root.child(
                img(rgba_to_gpui_render_image(rgba))
                    .w(px(sz))
                    .h(px(sz))
                    .object_fit(ObjectFit::Contain),
            ),
            Err(_) => root,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlushLoaderLayout {
    Row,
    Stack,
}

impl PlushLoaderLayout {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Row => "row",
            Self::Stack => "stack",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlushLoaderBallPose {
    pub asset_path: &'static str,
    pub size_px: f32,
    pub translate_y_px: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub opacity: f32,
}

/// `PlushLoader` (`packages/ui/src/plush-loader.tsx:L28-L51`, `primitives.css:L2066-L2283`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlushLoader {
    pub size_px: f32,
    pub layout: PlushLoaderLayout,
    pub label: Option<String>,
}

impl Default for PlushLoader {
    fn default() -> Self {
        Self::new(32.0, PlushLoaderLayout::Row)
    }
}

impl PlushLoader {
    pub fn new(size_px: f32, layout: PlushLoaderLayout) -> Self {
        Self {
            size_px: if size_px.is_finite() && size_px > 0.0 { size_px } else { 32.0 },
            layout,
            label: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn ball_size_px(&self) -> f32 {
        match self.layout {
            PlushLoaderLayout::Row => self.size_px * 0.24,
            PlushLoaderLayout::Stack => self.size_px * 0.20,
        }
    }

    pub fn sample_balls(&self, elapsed_ms: f64, reduced_motion: bool) -> [PlushLoaderBallPose; 3] {
        const POP_DELAYS_MS: [f64; 3] = [300.0, 380.0, 460.0];
        const BOUNCE_DELAYS_MS: [f64; 3] = [700.0, 850.0, 1_000.0];
        let ball_px = self.ball_size_px();

        std::array::from_fn(|idx| {
            if reduced_motion {
                return PlushLoaderBallPose {
                    asset_path: PLUSH_LOADER_BALL_ASSETS[idx],
                    size_px: ball_px,
                    translate_y_px: 0.0,
                    scale_x: 1.0,
                    scale_y: 1.0,
                    opacity: 1.0,
                };
            }
            let pop_u = ((elapsed_ms - POP_DELAYS_MS[idx]) / 360.0).clamp(0.0, 1.0) as f32;
            let opacity = pop_u;
            let pop_scale = 0.6 + 0.4 * (1.0 - (1.0 - pop_u).powi(3));

            let (mut ty, mut sx, mut sy) = (0.0_f32, 1.0_f32, 1.0_f32);
            if elapsed_ms >= BOUNCE_DELAYS_MS[idx] {
                let p = (((elapsed_ms - BOUNCE_DELAYS_MS[idx]) % 1_200.0) / 1_200.0) as f32;
                if p < 0.22 {
                    let u = p / 0.22;
                    ty = -1.10 * ball_px * u;
                    sx = 1.0 - 0.05 * u;
                    sy = 1.0 + 0.06 * u;
                } else if p < 0.42 {
                    let u = (p - 0.22) / 0.20;
                    ty = -1.10 * ball_px * (1.0 - u);
                    sx = 0.95 + 0.19 * u;
                    sy = 1.06 - 0.22 * u;
                } else if p < 0.50 {
                    let u = (p - 0.42) / 0.08;
                    sx = 1.14 - 0.14 * u;
                    sy = 0.84 + 0.16 * u;
                }
            }
            PlushLoaderBallPose {
                asset_path: PLUSH_LOADER_BALL_ASSETS[idx],
                size_px: ball_px,
                translate_y_px: ty,
                scale_x: sx * pop_scale,
                scale_y: sy * pop_scale,
                opacity,
            }
        })
    }

    pub fn render_element(&self, elapsed_ms: f64, reduced_motion: bool) -> gpui::Div {
        let sz = self.size_px.max(8.0);
        let mascot = PlushMascot::new(sz);
        let assets = BundledPlushAssets::global();
        let mascot_el = match mascot.composite_rgba(assets, elapsed_ms, true, reduced_motion) {
            Ok(rgba) => div().w(px(sz)).h(px(sz)).flex_shrink_0().child(
                img(rgba_to_gpui_render_image(rgba))
                    .w(px(sz))
                    .h(px(sz))
                    .object_fit(ObjectFit::Contain),
            ),
            Err(_) => div().w(px(sz)).h(px(sz)).flex_shrink_0(),
        };

        let ball_poses = self.sample_balls(elapsed_ms, reduced_motion);
        let mut balls_row = div().flex().items_end().gap(px(sz * 0.08));
        if self.layout == PlushLoaderLayout::Row {
            balls_row = balls_row.pb(px(sz * 0.04));
        }
        for pose in ball_poses {
            let w = (pose.size_px * pose.scale_x).max(1.0);
            let h = (pose.size_px * pose.scale_y).max(1.0);
            let mut slot = div()
                .relative()
                .w(px(pose.size_px))
                .h(px(pose.size_px * 2.2))
                .flex()
                .items_end()
                .justify_center();
            if pose.opacity > 0.01 {
                if let Ok(ball_img) = assets.render_image(pose.asset_path) {
                    slot = slot.child(
                        img(ball_img)
                            .w(px(w))
                            .h(px(h))
                            .mb(px((-pose.translate_y_px).max(0.0)))
                            .opacity(pose.opacity)
                            .object_fit(ObjectFit::Contain),
                    );
                }
            }
            balls_row = balls_row.child(slot);
        }

        match self.layout {
            PlushLoaderLayout::Row => div()
                .flex()
                .items_end()
                .gap(px(sz * 0.18))
                .child(mascot_el)
                .child(balls_row),
            PlushLoaderLayout::Stack => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(sz * 0.30))
                .child(mascot_el)
                .child(balls_row),
        }
    }
}

pub const GLOBAL_SEARCH_IDLE_MS: u64 = 500;
pub const GLOBAL_SEARCH_GAZE_REACH: f64 = 160.0;
pub const GLOBAL_SEARCH_LOOK_PX: f64 = 4.0;
pub const GLOBAL_SEARCH_DART_MS: f64 = 760.0;
pub const GLOBAL_SEARCH_TILT_MS: f64 = 900.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GlobalSearchPlushSample {
    pub gaze: Option<(f64, f64)>,
    pub eye_dart_x_px: f64,
    pub body_rotate_deg: f64,
    pub body_translate_y_px: f64,
    pub looking_for_result: bool,
}

fn ease_in_out_sine(t: f64) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * t.clamp(0.0, 1.0)).cos()
}

fn cubic_bezier_y(p1x: f64, p1y: f64, p2x: f64, p2y: f64, x: f64) -> f64 {
    let target = x.clamp(0.0, 1.0);
    let sample_x = |t: f64| {
        let inv = 1.0 - t;
        3.0 * inv * inv * t * p1x + 3.0 * inv * t * t * p2x + t * t * t
    };
    let sample_y = |t: f64| {
        let inv = 1.0 - t;
        3.0 * inv * inv * t * p1y + 3.0 * inv * t * t * p2y + t * t * t
    };
    let mut lo = 0.0_f64;
    let mut hi = 1.0_f64;
    for _ in 0..16 {
        let mid = 0.5 * (lo + hi);
        if sample_x(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    sample_y(0.5 * (lo + hi))
}

/// `GlobalSearchPlushState` (`packages/ui/src/components/GlobalSearchPlush.tsx:L5-L128`).
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalSearchPlushState {
    pub reduced_motion: bool,
    pub searching: bool,
    pub gaze: Option<(f64, f64)>,
    idle_deadline_ms: Option<f64>,
    waiting_for_search_end: bool,
    look_started_ms: Option<f64>,
}

impl GlobalSearchPlushState {
    pub const IDLE_MS: u64 = GLOBAL_SEARCH_IDLE_MS;
    pub const GAZE_REACH: f64 = GLOBAL_SEARCH_GAZE_REACH;
    pub const LOOK_PX: f64 = GLOBAL_SEARCH_LOOK_PX;

    pub fn new(reduced_motion: bool) -> Self {
        Self {
            reduced_motion,
            searching: false,
            gaze: None,
            idle_deadline_ms: None,
            waiting_for_search_end: false,
            look_started_ms: None,
        }
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        if reduced_motion {
            self.gaze = None;
            self.idle_deadline_ms = None;
            self.waiting_for_search_end = false;
            self.look_started_ms = None;
        }
    }

    pub fn update_caret(
        &mut self,
        caret_x: f64,
        caret_y: f64,
        center_x: f64,
        center_y: f64,
        now_ms: f64,
    ) {
        if self.reduced_motion
            || !caret_x.is_finite()
            || !caret_y.is_finite()
            || !center_x.is_finite()
            || !center_y.is_finite()
            || !now_ms.is_finite()
        {
            if self.reduced_motion {
                self.gaze = None;
            }
            return;
        }
        self.look_started_ms = None;
        self.waiting_for_search_end = false;
        let gx = ((caret_x - center_x) / Self::GAZE_REACH).clamp(-1.0, 1.0);
        let gy = ((caret_y - center_y) / Self::GAZE_REACH).clamp(-1.0, 1.0);
        self.gaze = Some((gx, gy));
        self.idle_deadline_ms = Some(now_ms + (Self::IDLE_MS as f64));
    }

    pub fn set_searching(&mut self, searching: bool, now_ms: f64) {
        self.searching = searching;
        if !searching && self.waiting_for_search_end && !self.reduced_motion && now_ms.is_finite() {
            self.start_look_for_result(now_ms);
        }
    }

    fn start_look_for_result(&mut self, start_ms: f64) {
        self.waiting_for_search_end = false;
        self.gaze = None;
        self.look_started_ms = Some(start_ms);
    }

    pub fn tick(&mut self, now_ms: f64) -> GlobalSearchPlushSample {
        if self.reduced_motion || !now_ms.is_finite() {
            return GlobalSearchPlushSample::default();
        }
        if let Some(deadline) = self.idle_deadline_ms {
            if now_ms >= deadline {
                self.idle_deadline_ms = None;
                self.waiting_for_search_end = true;
                if !self.searching {
                    self.start_look_for_result(deadline);
                }
            }
        }
        let Some(start_ms) = self.look_started_ms else {
            return GlobalSearchPlushSample {
                gaze: self.gaze,
                ..GlobalSearchPlushSample::default()
            };
        };
        let elapsed = (now_ms - start_ms).max(0.0);
        let total_ms = GLOBAL_SEARCH_DART_MS + GLOBAL_SEARCH_TILT_MS;
        if elapsed >= total_ms {
            self.look_started_ms = None;
            return GlobalSearchPlushSample::default();
        }

        let mut eye_dart_x_px = 0.0_f64;
        if elapsed < GLOBAL_SEARCH_DART_MS {
            let p = ease_in_out_sine(elapsed / GLOBAL_SEARCH_DART_MS);
            eye_dart_x_px = if p < 0.20 {
                -Self::LOOK_PX * (p / 0.20)
            } else if p < 0.50 {
                -Self::LOOK_PX + 2.0 * Self::LOOK_PX * ((p - 0.20) / 0.30)
            } else if p < 0.75 {
                Self::LOOK_PX - 2.0 * Self::LOOK_PX * ((p - 0.50) / 0.25)
            } else {
                -Self::LOOK_PX * (1.0 - (p - 0.75) / 0.25)
            };
        }

        let mut body_rotate_deg = 0.0_f64;
        let mut body_translate_y_px = 0.0_f64;
        if elapsed >= GLOBAL_SEARCH_DART_MS {
            let u = (elapsed - GLOBAL_SEARCH_DART_MS) / GLOBAL_SEARCH_TILT_MS;
            let eased = cubic_bezier_y(0.22, 1.0, 0.36, 1.0, u);
            let amt = if eased < 0.35 {
                eased / 0.35
            } else if eased <= 0.70 {
                1.0
            } else {
                (1.0 - eased) / 0.30
            }
            .clamp(0.0, 1.0);
            body_rotate_deg = -10.0 * amt;
            body_translate_y_px = -4.0 * amt;
        }

        GlobalSearchPlushSample {
            gaze: None,
            eye_dart_x_px,
            body_rotate_deg,
            body_translate_y_px,
            looking_for_result: true,
        }
    }

    pub fn render_element(&mut self, now_ms: f64, size_px: f32) -> gpui::Div {
        let sample = self.tick(now_ms);
        let sz = size_px.max(8.0);
        let mut mascot = PlushMascot::new(sz);
        if let Some((gx, gy)) = sample.gaze {
            mascot.extra_eye_offset_px = (
                (gx.clamp(-1.0, 1.0) * 0.07 * f64::from(sz)) as f32,
                (gy.clamp(-1.0, 1.0) * 0.05 * f64::from(sz)) as f32,
            );
        } else {
            mascot.extra_eye_offset_px = (sample.eye_dart_x_px as f32, 0.0);
        }
        mascot.body_rotate_deg = sample.body_rotate_deg as f32;
        mascot.body_translate_px = (0.0, sample.body_translate_y_px as f32);
        mascot.render_element(now_ms, self.reduced_motion)
    }
}

/// `MarketplaceNoMatchPlushState` (`MarketplaceNoMatchPlush.tsx:L1-L54`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketplaceNoMatchPlushState {
    pub reduced_motion: bool,
    pub top_left_alpha: Option<u8>,
    pub video_error: bool,
}

impl MarketplaceNoMatchPlushState {
    pub const VIDEO_ASSET_NAME: &'static str = "fishing.webm";
    pub const STILL_ASSET_NAME: &'static str = "fishing-still.webp";
    pub const VIDEO_REL_PATH: &'static str = "ui/fishing.webm";
    pub const STILL_REL_PATH: &'static str = "ui/fishing-still.webp";
    pub const ALPHA_THRESHOLD: u8 = 128;

    pub const fn new(reduced_motion: bool) -> Self {
        Self {
            reduced_motion,
            top_left_alpha: None,
            video_error: false,
        }
    }

    pub const fn keeps_alpha(top_left_rgba: [u8; 4]) -> bool {
        top_left_rgba[3] < Self::ALPHA_THRESHOLD
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    pub fn on_loaded_top_left_alpha(&mut self, alpha: u8) {
        self.top_left_alpha = Some(alpha);
    }

    pub fn on_video_error(&mut self) {
        self.video_error = true;
    }

    pub fn uses_video_loop(&self) -> bool {
        !self.reduced_motion
            && !self.video_error
            && self.top_left_alpha.is_none_or(|a| a < Self::ALPHA_THRESHOLD)
    }

    pub fn active_asset_name(&self) -> &'static str {
        if self.uses_video_loop() {
            Self::VIDEO_ASSET_NAME
        } else {
            Self::STILL_ASSET_NAME
        }
    }

    pub fn active_asset_rel_path(&self) -> &'static str {
        if self.uses_video_loop() {
            Self::VIDEO_REL_PATH
        } else {
            Self::STILL_REL_PATH
        }
    }

    pub const fn poster_asset_rel_path(&self) -> &'static str {
        Self::STILL_REL_PATH
    }

    pub fn render_element(&self, size_px: f32) -> gpui::Div {
        let sz = if size_px.is_finite() && size_px > 0.0 { size_px } else { 160.0 };
        let root = div().relative().w(px(sz)).h(px(sz)).flex_shrink_0();
        match BundledPlushAssets::global().render_image(Self::STILL_REL_PATH) {
            Ok(still) => root.child(img(still).w(px(sz)).h(px(sz)).object_fit(ObjectFit::Contain)),
            Err(_) => root,
        }
    }
}

pub fn recovery_plush_svg_from_assets(assets: &BundledPlushAssets) -> Option<String> {
    let body_bytes = assets.read_bytes("bloub/cloe-corps.webp").ok()?;
    let eye_bytes = assets.read_bytes("bloub/cloe-oeil0-x.webp").ok()?;
    let body_url = format!(
        "data:image/webp;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&body_bytes)
    );
    let eye_url = format!(
        "data:image/webp;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&eye_bytes)
    );
    let slice = |x: &str, w: &str, vb: &str| -> String {
        format!(
            "<svg preserveAspectRatio=\"none\" x=\"{x}\" y=\"-28.54\" width=\"{w}\" height=\"57.07\" viewBox=\"{vb}\" style=\"mix-blend-mode:plus-lighter\"><use href=\"#plush-eye-texture\"/></svg>"
        )
    };
    let eye_slices = [
        slice("-36.65", "32.15", "0 0 0.5 1"),
        slice("4.5", "32.15", "0.5 0 0.5 1"),
        slice("-4.5", "4.5", "0.5 0 0.07 1"),
        slice("0", "4.5", "0.43 0 0.07 1"),
    ]
    .concat();
    Some(format!(
        "<svg class=\"plush\" viewBox=\"-110 -120 220 200\" aria-hidden=\"true\" focusable=\"false\">\n<defs>\n<image id=\"plush-eye-texture\" width=\"1\" height=\"1\" preserveAspectRatio=\"none\" href=\"{eye_url}\"/>\n<g id=\"plush-eye\" style=\"isolation:isolate\">{eye_slices}</g>\n</defs>\n<image x=\"-158\" y=\"-158\" width=\"316\" height=\"316\" href=\"{body_url}\"/>\n<g class=\"plush-eyes\">\n<use href=\"#plush-eye\" transform=\"translate(2.46,-3.84) matrix(0.85,-0.4,0.45,0.89,-31.31,22.88)\"/>\n<use href=\"#plush-eye\" transform=\"translate(-4.15,-3.95) matrix(0.81,0.39,-0.44,0.9,44.26,22.41)\"/>\n</g>\n</svg>"
    ))
}

/// Static `cloe` 4-slice `plus-lighter` recovery SVG (`src/main/start-recovery-plush.ts:L9-L45`).
pub fn recovery_plush_svg() -> String {
    recovery_plush_svg_from_assets(BundledPlushAssets::global()).unwrap_or_default()
}

pub const APP_LOGO_SIZE: f32 = 240.0;
pub const APP_LOGO_CORNER: f32 = 50.0;
pub const APP_LOGO_STROKE_WIDTH: f32 = 9.5;
pub const APP_LOGO_LEFT_ORIGIN: (f32, f32) = (0.287, 0.487);
pub const APP_LOGO_RIGHT_ORIGIN: (f32, f32) = (0.712, 0.488);
pub const APP_LOGO_FIRST_BLINK_DELAY_MS: f64 = 2_112.0;
pub const APP_LOGO_BLINK_PERIOD_MS: f64 = 4_800.0;
pub const APP_LOGO_BLINK_DURATION_MS: f64 = 192.0;
pub const APP_LOGO_WINK_DURATION_MS: f64 = 360.0;
pub const APP_LOGO_RAPID_CLICK_WINDOW_MS: f64 = 900.0;
pub const APP_LOGO_RAPID_CLICK_COUNT: usize = 5;
pub const APP_LOGO_PARTY_DURATION_MS: f64 = 1_200.0;

pub const APP_LOGO_EYE_LEFT_POINTS_STR: &str = "43.55 93.61 64.69 81.41 36.48 108.04 79.67 83.11 35.93 122.88 91.58 90.74 38.9 132.69 97.66 98.76 42.44 138.88 100.43 105.4 46.9 143.83 101.97 112.04 55.08 149.51 101.83 122.52 73.01 152.43 94.14 140.23";
pub const APP_LOGO_EYE_RIGHT_POINTS_STR: &str = "145.65 93.61 166.79 81.41 140.83 101.52 175.58 81.46 138.3 109.53 183.18 83.63 137.55 117.43 189.67 87.33 139.67 129.39 197.88 95.78 142.92 136.32 201.52 102.48 149.03 143.86 204.07 112.08 159.14 150.37 203.51 124.75 169.28 152.61 199.82 134.98";

pub const APP_LOGO_LEFT_POLYLINE: [(f32, f32); 16] = [
    (43.55, 93.61), (64.69, 81.41), (36.48, 108.04), (79.67, 83.11),
    (35.93, 122.88), (91.58, 90.74), (38.9, 132.69), (97.66, 98.76),
    (42.44, 138.88), (100.43, 105.4), (46.9, 143.83), (101.97, 112.04),
    (55.08, 149.51), (101.83, 122.52), (73.01, 152.43), (94.14, 140.23),
];

pub const APP_LOGO_RIGHT_POLYLINE: [(f32, f32); 18] = [
    (145.65, 93.61), (166.79, 81.41), (140.83, 101.52), (175.58, 81.46),
    (138.3, 109.53), (183.18, 83.63), (137.55, 117.43), (189.67, 87.33),
    (139.67, 129.39), (197.88, 95.78), (142.92, 136.32), (201.52, 102.48),
    (149.03, 143.86), (204.07, 112.08), (159.14, 150.37), (203.51, 124.75),
    (169.28, 152.61), (199.82, 134.98),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppLogoVariant {
    Production,
    Dev,
    Preview,
}

impl AppLogoVariant {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Dev => "dev",
            Self::Preview => "preview",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "production" => Some(Self::Production),
            "dev" => Some(Self::Dev),
            "preview" => Some(Self::Preview),
            _ => None,
        }
    }

    pub const fn background_hex(self) -> &'static str {
        match self {
            Self::Production => "#d6adf2",
            Self::Dev => "#e3b866",
            Self::Preview => "#61c985",
        }
    }

    pub const fn background_rgb_u32(self) -> u32 {
        match self {
            Self::Production => 0xd6adf2,
            Self::Dev => 0xe3b866,
            Self::Preview => 0x61c985,
        }
    }

    pub const fn eye_hex(self) -> &'static str {
        "#040007"
    }

    pub const fn eye_rgb_u32(self) -> u32 {
        0x040007
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppLogoAnimation {
    None,
    Blink,
    LookAround,
    Surprised,
}

impl AppLogoAnimation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Blink => "blink",
            Self::LookAround => "look-around",
            Self::Surprised => "surprised",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppLogoReaction {
    None,
    Wink,
    Party,
}

impl AppLogoReaction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Wink => "wink",
            Self::Party => "party",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppLogoEyePose {
    pub translate_pct: (f32, f32),
    pub scale: (f32, f32),
    pub rotate_deg: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppLogoFrameSample {
    pub background_rgb_u32: u32,
    pub eye_rgb_u32: u32,
    pub blinking: bool,
    pub click_reaction: Option<&'static str>,
    pub easter_egg: Option<&'static str>,
    pub left_eye: AppLogoEyePose,
    pub right_eye: AppLogoEyePose,
}

/// Interactive `AppLogoState` (`packages/brand/src/AppLogo.tsx:L15-L188`).
#[derive(Debug, Clone, PartialEq)]
pub struct AppLogoState {
    pub variant: AppLogoVariant,
    pub animation: AppLogoAnimation,
    pub interactive: bool,
    pub has_pointer_area: bool,
    pub reduced_motion: bool,
    gaze_norm: (f32, f32),
    rapid_clicks_ms: Vec<f64>,
    wink_start_ms: Option<f64>,
    party_start_ms: Option<f64>,
}

impl Default for AppLogoState {
    fn default() -> Self {
        Self::new(AppLogoVariant::Production)
    }
}

fn lerp_rgb_u32(a: u32, b: u32, t: f32) -> u32 {
    let u = t.clamp(0.0, 1.0);
    let ch = |shift: u32| -> u32 {
        let ca = ((a >> shift) & 0xff) as f32;
        let cb = ((b >> shift) & 0xff) as f32;
        ((ca + (cb - ca) * u).round().clamp(0.0, 255.0)) as u32
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

impl AppLogoState {
    pub const fn new(variant: AppLogoVariant) -> Self {
        Self {
            variant,
            animation: AppLogoAnimation::None,
            interactive: true,
            has_pointer_area: false,
            reduced_motion: false,
            gaze_norm: (0.0, 0.0),
            rapid_clicks_ms: Vec::new(),
            wink_start_ms: None,
            party_start_ms: None,
        }
    }

    pub const fn with_animation(mut self, animation: AppLogoAnimation) -> Self {
        self.animation = animation;
        self
    }

    pub const fn with_interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    pub const fn with_pointer_area(mut self, has_pointer_area: bool) -> Self {
        self.has_pointer_area = has_pointer_area;
        self
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        if reduced_motion {
            self.gaze_norm = (0.0, 0.0);
            self.rapid_clicks_ms.clear();
            self.wink_start_ms = None;
            self.party_start_ms = None;
        }
    }

    pub fn reset_eye_position(&mut self) {
        self.gaze_norm = (0.0, 0.0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn look_at(
        &mut self,
        pointer_kind: PointerKind,
        client_x: f64,
        client_y: f64,
        logo_center_x: f64,
        logo_center_y: f64,
        area_left: f64,
        area_top: f64,
        area_right: f64,
        area_bottom: f64,
    ) {
        if !self.interactive || self.reduced_motion || pointer_kind != PointerKind::Mouse {
            return;
        }
        let reach_x = (logo_center_x - area_left)
            .max(area_right - logo_center_x)
            .max(1.0);
        let reach_y = (logo_center_y - area_top)
            .max(area_bottom - logo_center_y)
            .max(1.0);
        let nx = ((client_x - logo_center_x) / reach_x).clamp(-1.0, 1.0) as f32;
        let ny = ((client_y - logo_center_y) / reach_y).clamp(-1.0, 1.0) as f32;
        self.gaze_norm = (nx, ny);
    }

    pub fn eye_position_pct(&self) -> (f32, f32) {
        let (rx, ry) = if self.has_pointer_area {
            (9.0_f32, 7.0_f32)
        } else {
            (2.4_f32, 1.8_f32)
        };
        (self.gaze_norm.0 * rx, self.gaze_norm.1 * ry)
    }

    pub fn handle_key_down(&mut self, key: &str, now_ms: f64) -> bool {
        if key != "Enter" && key != " " {
            return false;
        }
        if self.interactive && !self.reduced_motion {
            self.wink_start_ms = Some(now_ms);
        }
        true
    }

    pub fn register_click(&mut self, now_ms: f64) -> AppLogoReaction {
        if !self.interactive || self.reduced_motion {
            return AppLogoReaction::None;
        }
        self.wink_start_ms = Some(now_ms);
        self.rapid_clicks_ms
            .retain(|&time| now_ms - time < APP_LOGO_RAPID_CLICK_WINDOW_MS);
        self.rapid_clicks_ms.push(now_ms);
        if self.rapid_clicks_ms.len() >= APP_LOGO_RAPID_CLICK_COUNT {
            self.rapid_clicks_ms.clear();
            self.party_start_ms = Some(now_ms);
            return AppLogoReaction::Party;
        }
        AppLogoReaction::Wink
    }

    pub fn sample(&self, now_ms: f64) -> AppLogoFrameSample {
        let base_bg = self.variant.background_rgb_u32();
        let eye_rgb = self.variant.eye_rgb_u32();
        let rest_eye = AppLogoEyePose {
            translate_pct: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotate_deg: 0.0,
        };
        if self.reduced_motion {
            return AppLogoFrameSample {
                background_rgb_u32: base_bg,
                eye_rgb_u32: eye_rgb,
                blinking: false,
                click_reaction: None,
                easter_egg: None,
                left_eye: rest_eye,
                right_eye: rest_eye,
            };
        }

        let gaze_pct = self.eye_position_pct();
        let mut left = AppLogoEyePose {
            translate_pct: gaze_pct,
            scale: (1.0, 1.0),
            rotate_deg: 0.0,
        };
        let mut right = left;
        let mut blinking = false;

        match self.animation {
            AppLogoAnimation::None => {}
            AppLogoAnimation::Blink => {
                if now_ms >= APP_LOGO_FIRST_BLINK_DELAY_MS {
                    let cycle = (now_ms - APP_LOGO_FIRST_BLINK_DELAY_MS) % APP_LOGO_BLINK_PERIOD_MS;
                    if cycle < APP_LOGO_BLINK_DURATION_MS {
                        blinking = true;
                        let p = (cycle / APP_LOGO_BLINK_DURATION_MS) as f32;
                        let dist = (p - 0.5).abs() * 2.0;
                        let sy = 0.08 + 0.92 * dist;
                        left.scale.1 *= sy;
                        right.scale.1 *= sy;
                    }
                }
            }
            AppLogoAnimation::LookAround => {
                let p = ((now_ms % 6_000.0) / 6_000.0) as f32;
                let dx = if (0.35..=0.42).contains(&p) {
                    -3.5
                } else if (0.52..=0.59).contains(&p) {
                    3.5
                } else {
                    0.0
                };
                left.translate_pct.0 += dx;
                right.translate_pct.0 += dx;
            }
            AppLogoAnimation::Surprised => {
                let p = ((now_ms % 5_400.0) / 5_400.0) as f32;
                if (0.44..=0.51).contains(&p) {
                    left.translate_pct.1 -= 1.5;
                    right.translate_pct.1 -= 1.5;
                    left.scale = (left.scale.0 * 0.94, left.scale.1 * 1.12);
                    right.scale = (right.scale.0 * 0.94, right.scale.1 * 1.12);
                }
            }
        }

        self.apply_reactions_to_sample(now_ms, base_bg, eye_rgb, blinking, left, right)
    }

    fn apply_reactions_to_sample(
        &self,
        now_ms: f64,
        base_bg: u32,
        eye_rgb: u32,
        blinking: bool,
        mut left: AppLogoEyePose,
        mut right: AppLogoEyePose,
    ) -> AppLogoFrameSample {
        let mut click_reaction = None;
        if let Some(ws) = self.wink_start_ms {
            let elapsed = now_ms - ws;
            if (0.0..APP_LOGO_WINK_DURATION_MS).contains(&elapsed) {
                click_reaction = Some("wink");
                let p = (elapsed / APP_LOGO_WINK_DURATION_MS) as f32;
                let left_sy = if p < 0.38 {
                    1.0 - 0.92 * (p / 0.38)
                } else if p <= 0.58 {
                    0.08
                } else {
                    0.08 + 0.92 * ((p - 0.58) / 0.42)
                };
                left.scale.1 *= left_sy;
                let right_amt = 1.0 - ((p - 0.48).abs() / 0.52).clamp(0.0, 1.0);
                right.translate_pct.1 -= 1.0 * right_amt;
                right.scale.0 *= 1.0 - 0.04 * right_amt;
                right.scale.1 *= 1.0 + 0.08 * right_amt;
            }
        }

        let mut bg = base_bg;
        let mut easter_egg = None;
        if let Some(ps) = self.party_start_ms {
            let elapsed = now_ms - ps;
            if (0.0..APP_LOGO_PARTY_DURATION_MS).contains(&elapsed) {
                easter_egg = Some("party");
                let p = (elapsed / APP_LOGO_PARTY_DURATION_MS) as f32;
                let dev_c = AppLogoVariant::Dev.background_rgb_u32();
                let prev_c = AppLogoVariant::Preview.background_rgb_u32();
                let prod_c = AppLogoVariant::Production.background_rgb_u32();
                bg = if p < 0.22 {
                    lerp_rgb_u32(base_bg, dev_c, p / 0.22)
                } else if p < 0.48 {
                    lerp_rgb_u32(dev_c, prev_c, (p - 0.22) / 0.26)
                } else if p < 0.74 {
                    lerp_rgb_u32(prev_c, prod_c, (p - 0.48) / 0.26)
                } else {
                    lerp_rgb_u32(prod_c, base_bg, (p - 0.74) / 0.26)
                };

                let dance = |u: f32, sign: f32| -> (f32, f32, f32) {
                    if u < 0.25 {
                        let k = u / 0.25;
                        (0.0, -6.0 * sign * k, -8.0 * sign * k)
                    } else if u < 0.50 {
                        let k = (u - 0.25) / 0.25;
                        (
                            4.0 * sign * k,
                            (-6.0 + 8.0 * k) * sign,
                            (-8.0 + 16.0 * k) * sign,
                        )
                    } else if u < 0.75 {
                        let k = (u - 0.50) / 0.25;
                        (
                            (4.0 - 8.0 * k) * sign,
                            (2.0 - 4.0 * k) * sign,
                            (8.0 - 12.0 * k) * sign,
                        )
                    } else {
                        let k = 1.0 - (u - 0.75) / 0.25;
                        (-4.0 * sign * k, -2.0 * sign * k, -4.0 * sign * k)
                    }
                };
                let (lx, ly, lr) = dance(p, 1.0);
                let (rx, ry, rr) = dance(p, -1.0);
                left.translate_pct.0 += lx;
                left.translate_pct.1 += ly;
                left.rotate_deg += lr;
                right.translate_pct.0 += rx;
                right.translate_pct.1 += ry;
                right.rotate_deg += rr;
            }
        }

        AppLogoFrameSample {
            background_rgb_u32: bg,
            eye_rgb_u32: eye_rgb,
            blinking,
            click_reaction,
            easter_egg,
            left_eye: left,
            right_eye: right,
        }
    }
}

fn emit_rounded_rect_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<gpui::Path<Pixels>> {
    let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
    let k = 0.552_284_8 * r;
    let x0 = x;
    let y0 = y;
    let x1 = x + w;
    let y1 = y + h;
    let mut b = PathBuilder::fill();
    b.move_to(point(px(x0 + r), px(y0)));
    b.line_to(point(px(x1 - r), px(y0)));
    b.cubic_bezier_to(
        point(px(x1), px(y0 + r)),
        point(px(x1 - r + k), px(y0)),
        point(px(x1), px(y0 + r - k)),
    );
    b.line_to(point(px(x1), px(y1 - r)));
    b.cubic_bezier_to(
        point(px(x1 - r), px(y1)),
        point(px(x1), px(y1 - r + k)),
        point(px(x1 - r + k), px(y1)),
    );
    b.line_to(point(px(x0 + r), px(y1)));
    b.cubic_bezier_to(
        point(px(x0), px(y1 - r)),
        point(px(x0 + r - k), px(y1)),
        point(px(x0), px(y1 - r + k)),
    );
    b.line_to(point(px(x0), px(y0 + r)));
    b.cubic_bezier_to(
        point(px(x0 + r), px(y0)),
        point(px(x0), px(y0 + r - k)),
        point(px(x0 + r - k), px(y0)),
    );
    b.close();
    b.build().ok()
}

fn emit_disc(b: &mut PathBuilder, center: Point<Pixels>, r: f32) {
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let k = 0.552_284_8 * r;
    b.move_to(point(px(cx), px(cy - r)));
    b.cubic_bezier_to(point(px(cx + r), px(cy)), point(px(cx + k), px(cy - r)), point(px(cx + r), px(cy - k)));
    b.cubic_bezier_to(point(px(cx), px(cy + r)), point(px(cx + r), px(cy + k)), point(px(cx + k), px(cy + r)));
    b.cubic_bezier_to(point(px(cx - r), px(cy)), point(px(cx - k), px(cy + r)), point(px(cx - r), px(cy + k)));
    b.cubic_bezier_to(point(px(cx), px(cy - r)), point(px(cx - r), px(cy - k)), point(px(cx - k), px(cy - r)));
    b.close();
}

fn paint_app_logo_eye(
    window: &mut Window,
    origin_px: (f32, f32),
    scale_px: f32,
    eye_origin_frac: (f32, f32),
    pose: AppLogoEyePose,
    pts_vb: &[(f32, f32)],
    eye_rgb: u32,
) {
    let pivot_vb = (
        APP_LOGO_SIZE * eye_origin_frac.0,
        APP_LOGO_SIZE * eye_origin_frac.1,
    );
    let tx_vb = (pose.translate_pct.0 / 100.0) * APP_LOGO_SIZE;
    let ty_vb = (pose.translate_pct.1 / 100.0) * APP_LOGO_SIZE;
    let rad = pose.rotate_deg.to_radians();
    let (cr, sr) = (rad.cos(), rad.sin());
    let map_pt = |(vx, vy): (f32, f32)| -> Point<Pixels> {
        let dx = (vx - pivot_vb.0) * pose.scale.0;
        let dy = (vy - pivot_vb.1) * pose.scale.1;
        let rx = pivot_vb.0 + dx * cr - dy * sr + tx_vb;
        let ry = pivot_vb.1 + dx * sr + dy * cr + ty_vb;
        point(
            px(origin_px.0 + rx * scale_px),
            px(origin_px.1 + ry * scale_px),
        )
    };
    let stroke_px = (APP_LOGO_STROKE_WIDTH * scale_px).max(1.0);
    let mut stroke_b = PathBuilder::stroke(px(stroke_px));
    let mut caps_b = PathBuilder::fill();
    let cap_r = stroke_px * 0.5;
    for (idx, &v) in pts_vb.iter().enumerate() {
        let p = map_pt(v);
        if idx == 0 {
            stroke_b.move_to(p);
        } else {
            stroke_b.line_to(p);
        }
        emit_disc(&mut caps_b, p, cap_r);
    }
    if let Ok(path) = stroke_b.build() {
        window.paint_path(path, rgb(eye_rgb));
    }
    if let Ok(caps) = caps_b.build() {
        window.paint_path(caps, rgb(eye_rgb));
    }
}

fn paint_app_logo(bounds: Bounds<Pixels>, frame: &AppLogoFrameSample, window: &mut Window) {
    let bx = f32::from(bounds.origin.x);
    let by = f32::from(bounds.origin.y);
    let bw = f32::from(bounds.size.width).max(1.0);
    let bh = f32::from(bounds.size.height).max(1.0);
    let side = bw.min(bh);
    let ox = bx + (bw - side) * 0.5;
    let oy = by + (bh - side) * 0.5;
    let scale = side / APP_LOGO_SIZE;

    if let Some(bg_path) = emit_rounded_rect_path(ox, oy, side, side, APP_LOGO_CORNER * scale) {
        window.paint_path(bg_path, rgb(frame.background_rgb_u32));
    }
    paint_app_logo_eye(
        window,
        (ox, oy),
        scale,
        APP_LOGO_LEFT_ORIGIN,
        frame.left_eye,
        &APP_LOGO_LEFT_POLYLINE,
        frame.eye_rgb_u32,
    );
    paint_app_logo_eye(
        window,
        (ox, oy),
        scale,
        APP_LOGO_RIGHT_ORIGIN,
        frame.right_eye,
        &APP_LOGO_RIGHT_POLYLINE,
        frame.eye_rgb_u32,
    );
}

/// Renders the `240×240` `AppLogo` (`Production` / `Dev` / `Preview`) into a GPUI `Div`.
pub fn render_app_logo(state: &AppLogoState, now_ms: f64, size_px: f32) -> gpui::Div {
    let sz = if size_px.is_finite() && size_px > 0.0 {
        size_px
    } else {
        48.0
    };
    let frame = state.sample(now_ms);
    div()
        .relative()
        .w(px(sz))
        .h(px(sz))
        .flex_shrink_0()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_app_logo(bounds, &frame, window);
                },
            )
            .w(px(sz))
            .h(px(sz)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_plush_logo_resolves_automatic_and_custom_tinted_shapes() {
        let assets = BundledPlushAssets::global();
        let auto = ServerPlushLogo::new("team-alpha");
        assert!(auto.is_automatic());
        let auto_img = auto.resolve_rgba(assets).expect("auto server logo");
        assert_eq!(auto_img.dimensions(), (192, 192));

        let custom = ServerPlushLogo::new("team-alpha").with_appearance(Some(ServerAppearance {
            shape: ServerAppearanceShape::Hexagone,
            color: Some("#8b5cf6".to_owned()),
        }));
        assert!(!custom.is_automatic());
        let custom_img = custom.resolve_rgba(assets).expect("custom server logo");
        assert_eq!(custom_img.dimensions(), (192, 192));
    }

    #[test]
    fn plush_mascot_and_loader_stagger_balls_and_respect_reduced_motion() {
        let loader = PlushLoader::new(32.0, PlushLoaderLayout::Row);
        let early = loader.sample_balls(320.0, false);
        assert!(early[0].opacity > 0.0 && early[2].opacity == 0.0);
        let bouncing = loader.sample_balls(820.0, false);
        assert!(bouncing[0].translate_y_px < -1.0);
        let reduced = loader.sample_balls(820.0, true);
        assert_eq!(reduced[0].translate_y_px, 0.0);

        let mascot = PlushMascot::new(64.0).with_face(PlushMascotFace::Wide);
        let img = mascot
            .composite_rgba(BundledPlushAssets::global(), 100.0, false, false)
            .expect("mascot composite");
        assert_eq!(img.dimensions(), (64, 64));
    }

    #[test]
    fn global_search_and_marketplace_plush_enforce_idle_and_alpha_contracts() {
        let mut search = GlobalSearchPlushState::new(false);
        search.update_caret(280.0, 100.0, 200.0, 100.0, 0.0);
        let g0 = search.tick(100.0);
        assert_eq!(g0.gaze, Some((0.5, 0.0)));
        let dart = search.tick(620.0);
        assert!(dart.looking_for_result && dart.gaze.is_none());
        let tilt = search.tick(1_700.0);
        assert!(tilt.body_rotate_deg < -1.0);

        let mut reduced_search = GlobalSearchPlushState::new(true);
        reduced_search.update_caret(280.0, 100.0, 200.0, 100.0, 0.0);
        assert_eq!(reduced_search.tick(600.0).gaze, None);

        let mut mp = MarketplaceNoMatchPlushState::new(false);
        assert_eq!(mp.active_asset_name(), "fishing.webm");
        mp.on_loaded_top_left_alpha(200);
        assert_eq!(mp.active_asset_name(), "fishing-still.webp");
        let mp_reduced = MarketplaceNoMatchPlushState::new(true);
        assert_eq!(mp_reduced.active_asset_name(), "fishing-still.webp");
    }

    #[test]
    fn recovery_svg_and_app_logo_easter_egg_match_upstream() {
        let svg = recovery_plush_svg();
        assert!(svg.contains("viewBox=\"-110 -120 220 200\""));
        assert!(svg.contains("mix-blend-mode:plus-lighter"));
        assert_eq!(svg.matches("<svg preserveAspectRatio=\"none\"").count(), 4);

        let mut logo = AppLogoState::new(AppLogoVariant::Production)
            .with_animation(AppLogoAnimation::Blink);
        logo.look_at(PointerKind::Touch, 200.0, 200.0, 100.0, 100.0, 0.0, 0.0, 300.0, 300.0);
        assert_eq!(logo.eye_position_pct(), (0.0, 0.0));
        logo.look_at(PointerKind::Mouse, 200.0, 200.0, 100.0, 100.0, 0.0, 0.0, 200.0, 200.0);
        assert!((logo.eye_position_pct().0 - 2.4).abs() < 1e-4);

        for i in 0..4 {
            assert_eq!(logo.register_click(1_000.0 + (i as f64) * 150.0), AppLogoReaction::Wink);
        }
        assert_eq!(logo.register_click(1_700.0), AppLogoReaction::Party);
        let party = logo.sample(2_000.0);
        assert_eq!(party.easter_egg, Some("party"));
        assert_eq!(logo.sample(3_000.0).easter_egg, None);
    }
}
