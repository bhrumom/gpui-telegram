//! Hybrid 60fps GPUI 0.2.2 procedural viewport for Code Scene (`CS-F12`).
//!
//! Renders every `CodeSceneScript` (`cs1`) asset across all three modalities
//! (`Video` / `Animation` / `Game`) using three synchronized layers:
//! - Layer 1: Optional bounded BGRA8 `Arc<gpui::RenderImage>` framebuffer with
//!   mandatory `cx.drop_image(prev, None)` VRAM leak cleanup on every swap.
//! - Layer 2: 60fps GPUI vector `canvas` (`PathBuilder::fill()` / `stroke()` +
//!   `window.paint_path()` + `linear_gradient()`) mapped via `ObjectFit::Contain`.
//! - Layer 3: DPI-crisp GPUI `div()` HUD/caption text overlay for `DrawCmd::Text`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_code_scene::{
    compile_and_verify, AudioPulse, DrawCmd, FrameOutput, GameHudSnapshot, GameSessionTelemetry,
    PixelFrame, RgbaColor, SceneInputState, TextAlign, VerifiedProgram, VmBudgets, VmInstance,
};
use gpui_telegram_proto::code_scene_contracts::{AssetKind, AssetManifest, CodeSceneAssetDto};

/// Frame-request deduplication interval (~60fps) matching `grok_bot_avatar.rs`.
const SCENE_ANIM_DEDUP_MS: u64 = 14;
/// Clamp single-frame simulation step to avoid physics explosions after sleep/pause.
const MAX_FRAME_DT_SEC: f64 = 0.05;
/// Maximum audio pulse visualizer bars rendered in the floating mute/equalizer pill.
pub const AUDIO_BAR_COUNT: usize = 8;

/// Deduplicated active-card-only `window.request_animation_frame()` helper.
pub fn dedup_request_animation_frame(window: &Window) {
    thread_local! {
        static LAST_SCENE_ANIM_REQ: RefCell<HashMap<EntityId, Instant>> = RefCell::new(HashMap::new());
    }
    let view_id = window.current_view();
    let now = Instant::now();
    let gate = Duration::from_millis(SCENE_ANIM_DEDUP_MS);
    let should_req = LAST_SCENE_ANIM_REQ.with(|cell| {
        let mut map = cell.borrow_mut();
        if map.len() > 32 {
            let stale = Duration::from_secs(5);
            map.retain(|_, ts| now.saturating_duration_since(*ts) <= stale);
        }
        let allow = map
            .get(&view_id)
            .is_none_or(|prev| now.saturating_duration_since(*prev) >= gate);
        if allow {
            map.insert(view_id, now);
        }
        allow
    });
    if should_req {
        window.request_animation_frame();
    }
}

/// Converts a BGRA8 `PixelFrame` directly into a GPUI `RenderImage` with zero channel swap.
pub fn build_render_image_from_pixel_frame(frame: &PixelFrame) -> Option<Arc<gpui::RenderImage>> {
    if frame.width == 0 || frame.height == 0 {
        return None;
    }
    let expected = (frame.width as usize)
        .checked_mul(frame.height as usize)?
        .checked_mul(4)?;
    if frame.bgra.len() != expected {
        return None;
    }
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra.clone())?;
    let image_frame = image::Frame::new(img);
    Some(Arc::new(gpui::RenderImage::new(vec![image_frame])))
}

/// Letterboxed/contained stage rectangle in window pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneContainRect {
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: f32,
    pub height: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub logical_w: f32,
    pub logical_h: f32,
}

impl SceneContainRect {
    /// Computes the `ObjectFit::Contain` rect for logical stage `(logical_w, logical_h)` inside `bounds`.
    pub fn compute(bounds: Bounds<Pixels>, logical_w: u32, logical_h: u32) -> Self {
        let raw_bw = f32::from(bounds.size.width);
        let raw_bh = f32::from(bounds.size.height);
        let bw = if raw_bw.is_finite() { raw_bw.max(1.0) } else { 1.0 };
        let bh = if raw_bh.is_finite() { raw_bh.max(1.0) } else { 1.0 };
        let raw_ox = f32::from(bounds.origin.x);
        let raw_oy = f32::from(bounds.origin.y);
        let ox = if raw_ox.is_finite() { raw_ox } else { 0.0 };
        let oy = if raw_oy.is_finite() { raw_oy } else { 0.0 };
        let lw = (logical_w.max(1)) as f32;
        let lh = (logical_h.max(1)) as f32;

        let stage_aspect = lw / lh;
        let box_aspect = bw / bh;
        let (dx, dy, draw_w, draw_h) = if box_aspect > stage_aspect {
            let w = bh * stage_aspect;
            ((bw - w) * 0.5, 0.0, w, bh)
        } else {
            let h = bw / stage_aspect;
            (0.0, (bh - h) * 0.5, bw, h)
        };
        let safe_w = draw_w.max(1.0);
        let safe_h = draw_h.max(1.0);
        Self {
            origin_x: ox + dx,
            origin_y: oy + dy,
            width: safe_w,
            height: safe_h,
            scale_x: safe_w / lw,
            scale_y: safe_h / lh,
            logical_w: lw,
            logical_h: lh,
        }
    }

    #[inline]
    pub fn map_point(&self, lx: f32, ly: f32) -> Point<Pixels> {
        let sx = if lx.is_finite() { lx } else { 0.0 };
        let sy = if ly.is_finite() { ly } else { 0.0 };
        point(
            px(self.origin_x + sx * self.scale_x),
            px(self.origin_y + sy * self.scale_y),
        )
    }

    #[inline]
    pub fn uniform_scale(&self) -> f32 {
        self.scale_x.min(self.scale_y).max(0.01)
    }
}

/// Maps a GPUI window-space pointer position inside `bounds` into clamped logical scene coordinates.
pub fn map_point_contain_scene(
    pt: Point<Pixels>,
    bounds: Bounds<Pixels>,
    logical_w: u32,
    logical_h: u32,
) -> (f64, f64) {
    let rect = SceneContainRect::compute(bounds, logical_w, logical_h);
    let px_x = f32::from(pt.x);
    let px_y = f32::from(pt.y);
    let ux = if px_x.is_finite() {
        ((px_x - rect.origin_x) / rect.width).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let uy = if px_y.is_finite() {
        ((px_y - rect.origin_y) / rect.height).clamp(0.0, 1.0)
    } else {
        0.5
    };
    (
        (ux as f64) * (logical_w.max(1) as f64),
        (uy as f64) * (logical_h.max(1) as f64),
    )
}

#[inline]
fn to_gpui_rgba(c: RgbaColor) -> Rgba {
    rgba(c.to_u32_rgba())
}

#[inline]
fn to_gpui_rgba_scaled_alpha(c: RgbaColor, alpha_mult: f32) -> Rgba {
    let a = ((c.a as f32) * alpha_mult.clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u8;
    rgba(RgbaColor::new(c.r, c.g, c.b, a).to_u32_rgba())
}

fn emit_rect_path(builder: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, rect: &SceneContainRect) {
    builder.move_to(rect.map_point(x, y));
    builder.line_to(rect.map_point(x + w, y));
    builder.line_to(rect.map_point(x + w, y + h));
    builder.line_to(rect.map_point(x, y + h));
    builder.close();
}

fn emit_round_rect_path(
    builder: &mut PathBuilder,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    rect: &SceneContainRect,
) {
    let r = radius.clamp(0.0, (w * 0.5).min(h * 0.5));
    if r <= 0.1 {
        emit_rect_path(builder, x, y, w, h, rect);
        return;
    }
    const K: f32 = 0.55228475;
    let kr = r * K;

    builder.move_to(rect.map_point(x + r, y));
    builder.line_to(rect.map_point(x + w - r, y));
    builder.cubic_bezier_to(
        rect.map_point(x + w, y + r),
        rect.map_point(x + w - r + kr, y),
        rect.map_point(x + w, y + r - kr),
    );
    builder.line_to(rect.map_point(x + w, y + h - r));
    builder.cubic_bezier_to(
        rect.map_point(x + w - r, y + h),
        rect.map_point(x + w, y + h - r + kr),
        rect.map_point(x + w - r + kr, y + h),
    );
    builder.line_to(rect.map_point(x + r, y + h));
    builder.cubic_bezier_to(
        rect.map_point(x, y + h - r),
        rect.map_point(x + r - kr, y + h),
        rect.map_point(x, y + h - r + kr),
    );
    builder.line_to(rect.map_point(x, y + r));
    builder.cubic_bezier_to(
        rect.map_point(x + r, y),
        rect.map_point(x, y + r - kr),
        rect.map_point(x + r - kr, y),
    );
    builder.close();
}

fn emit_circle_path(builder: &mut PathBuilder, cx: f32, cy: f32, r: f32, rect: &SceneContainRect) {
    const K: f32 = 0.55228475;
    let k = r * K;
    builder.move_to(rect.map_point(cx, cy - r));
    builder.cubic_bezier_to(
        rect.map_point(cx + r, cy),
        rect.map_point(cx + k, cy - r),
        rect.map_point(cx + r, cy - k),
    );
    builder.cubic_bezier_to(
        rect.map_point(cx, cy + r),
        rect.map_point(cx + r, cy + k),
        rect.map_point(cx + k, cy + r),
    );
    builder.cubic_bezier_to(
        rect.map_point(cx - r, cy),
        rect.map_point(cx - k, cy + r),
        rect.map_point(cx - r, cy + k),
    );
    builder.cubic_bezier_to(
        rect.map_point(cx, cy - r),
        rect.map_point(cx - r, cy - k),
        rect.map_point(cx - k, cy - r),
    );
    builder.close();
}

/// Builds a tessellated GPUI [`Path<Pixels>`] for a single vector [`DrawCmd`].
/// Returns `None` for non-path commands (`Text`) or degenerate zero-area primitives.
pub fn build_vector_path_for_cmd(
    cmd: &DrawCmd,
    rect: &SceneContainRect,
) -> Option<Path<Pixels>> {
    if !cmd.all_floats_finite() || !cmd.is_visible() {
        return None;
    }
    let scale = rect.uniform_scale();
    match cmd {
        DrawCmd::Clear { .. } | DrawCmd::LinearGradientBg { .. } => {
            let mut b = PathBuilder::fill();
            emit_rect_path(&mut b, 0.0, 0.0, rect.logical_w, rect.logical_h, rect);
            b.build().ok()
        }
        DrawCmd::FillRect { x, y, w, h, .. } => {
            if *w <= 0.0 || *h <= 0.0 {
                return None;
            }
            let mut b = PathBuilder::fill();
            emit_rect_path(&mut b, *x, *y, *w, *h, rect);
            b.build().ok()
        }
        DrawCmd::RoundRect {
            x, y, w, h, radius, ..
        } => {
            if *w <= 0.0 || *h <= 0.0 {
                return None;
            }
            let mut b = PathBuilder::fill();
            emit_round_rect_path(&mut b, *x, *y, *w, *h, *radius, rect);
            b.build().ok()
        }
        DrawCmd::StrokeRect {
            x,
            y,
            w,
            h,
            stroke_width,
            ..
        } => {
            if *w <= 0.0 || *h <= 0.0 {
                return None;
            }
            let sw = (*stroke_width * scale).max(0.75);
            let mut b = PathBuilder::stroke(px(sw));
            emit_rect_path(&mut b, *x, *y, *w, *h, rect);
            b.build().ok()
        }
        DrawCmd::FillCircle { cx, cy, radius, .. } | DrawCmd::GlowCircle { cx, cy, radius, .. } => {
            if *radius <= 0.0 {
                return None;
            }
            let mut b = PathBuilder::fill();
            emit_circle_path(&mut b, *cx, *cy, *radius, rect);
            b.build().ok()
        }
        DrawCmd::StrokeCircle {
            cx,
            cy,
            radius,
            stroke_width,
            ..
        } => {
            if *radius <= 0.0 {
                return None;
            }
            let sw = (*stroke_width * scale).max(0.75);
            let mut b = PathBuilder::stroke(px(sw));
            emit_circle_path(&mut b, *cx, *cy, *radius, rect);
            b.build().ok()
        }
        DrawCmd::Line {
            x1,
            y1,
            x2,
            y2,
            stroke_width,
            ..
        } => {
            if (*x2 - *x1).hypot(*y2 - *y1) < 0.01 {
                return None;
            }
            let sw = (*stroke_width * scale).max(0.75);
            let mut b = PathBuilder::stroke(px(sw));
            b.move_to(rect.map_point(*x1, *y1));
            b.line_to(rect.map_point(*x2, *y2));
            b.build().ok()
        }
        DrawCmd::Polyline {
            points,
            stroke_width,
            ..
        } => {
            if points.len() < 2 {
                return None;
            }
            let sw = (*stroke_width * scale).max(0.75);
            let mut b = PathBuilder::stroke(px(sw));
            b.move_to(rect.map_point(points[0][0], points[0][1]));
            for pt in &points[1..] {
                b.line_to(rect.map_point(pt[0], pt[1]));
            }
            b.build().ok()
        }
        DrawCmd::FillPolygon { points, .. } => {
            if points.len() < 3 {
                return None;
            }
            let mut b = PathBuilder::fill();
            b.move_to(rect.map_point(points[0][0], points[0][1]));
            for pt in &points[1..] {
                b.line_to(rect.map_point(pt[0], pt[1]));
            }
            b.close();
            b.build().ok()
        }
        DrawCmd::Arc {
            cx,
            cy,
            radius,
            start_rad,
            end_rad,
            stroke_width,
            ..
        } => {
            if *radius <= 0.0 {
                return None;
            }
            let span = (*end_rad - *start_rad).clamp(-TAU * 4.0, TAU * 4.0);
            if span.abs() < 0.001 {
                return None;
            }
            let steps = ((span.abs() / 0.14).ceil() as usize).clamp(6, 72);
            let sw = (*stroke_width * scale).max(0.75);
            let mut b = PathBuilder::stroke(px(sw));
            for i in 0..=steps {
                let frac = (i as f32) / (steps as f32);
                let angle = *start_rad + span * frac;
                let px_x = *cx + angle.cos() * *radius;
                let px_y = *cy + angle.sin() * *radius;
                if i == 0 {
                    b.move_to(rect.map_point(px_x, px_y));
                } else {
                    b.line_to(rect.map_point(px_x, px_y));
                }
            }
            b.build().ok()
        }
        DrawCmd::Bezier {
            x0,
            y0,
            cx1,
            cy1,
            cx2,
            cy2,
            x1,
            y1,
            stroke_width,
            ..
        } => {
            let sw = (*stroke_width * scale).max(0.75);
            let mut b = PathBuilder::stroke(px(sw));
            b.move_to(rect.map_point(*x0, *y0));
            b.cubic_bezier_to(
                rect.map_point(*x1, *y1),
                rect.map_point(*cx1, *cy1),
                rect.map_point(*cx2, *cy2),
            );
            b.build().ok()
        }
        DrawCmd::Star {
            cx,
            cy,
            outer_r,
            inner_r,
            points,
            rotation_rad,
            ..
        } => {
            let n = (*points).max(3) as usize;
            let total = n * 2;
            let mut b = PathBuilder::fill();
            for i in 0..total {
                let r = if i % 2 == 0 { *outer_r } else { *inner_r };
                let angle = *rotation_rad + (i as f32) * (TAU / (total as f32));
                let vx = *cx + angle.cos() * r;
                let vy = *cy + angle.sin() * r;
                if i == 0 {
                    b.move_to(rect.map_point(vx, vy));
                } else {
                    b.line_to(rect.map_point(vx, vy));
                }
            }
            b.close();
            b.build().ok()
        }
        DrawCmd::Text { .. } => None,
    }
}

/// Paints all Layer-2 vector `DrawCmd`s into GPUI's native vector canvas.
pub fn paint_draw_cmds(
    bounds: Bounds<Pixels>,
    logical_w: u32,
    logical_h: u32,
    cmds: &[DrawCmd],
    window: &mut Window,
) {
    let rect = SceneContainRect::compute(bounds, logical_w, logical_h);
    for cmd in cmds {
        if !cmd.is_visible() || !cmd.all_floats_finite() {
            continue;
        }
        match cmd {
            DrawCmd::LinearGradientBg {
                top,
                bottom,
                angle_deg,
            } => {
                if let Some(path) = build_vector_path_for_cmd(cmd, &rect) {
                    window.paint_path(
                        path,
                        linear_gradient(
                            *angle_deg,
                            linear_color_stop(to_gpui_rgba(*top), 0.0),
                            linear_color_stop(to_gpui_rgba(*bottom), 1.0),
                        ),
                    );
                }
            }
            DrawCmd::GlowCircle {
                cx,
                cy,
                radius,
                color,
                intensity,
            } => {
                let boost = intensity.clamp(0.2, 2.5);
                for (mult, alpha_frac) in [(2.2, 0.10), (1.55, 0.22), (1.18, 0.42)] {
                    let mut halo = PathBuilder::fill();
                    emit_circle_path(&mut halo, *cx, *cy, *radius * mult, &rect);
                    if let Ok(path) = halo.build() {
                        window.paint_path(
                            path,
                            to_gpui_rgba_scaled_alpha(*color, (alpha_frac * boost).min(0.85)),
                        );
                    }
                }
                if let Some(core) = build_vector_path_for_cmd(cmd, &rect) {
                    window.paint_path(core, to_gpui_rgba(*color));
                }
            }
            DrawCmd::Clear { color }
            | DrawCmd::FillRect { color, .. }
            | DrawCmd::RoundRect { color, .. }
            | DrawCmd::StrokeRect { color, .. }
            | DrawCmd::FillCircle { color, .. }
            | DrawCmd::StrokeCircle { color, .. }
            | DrawCmd::Line { color, .. }
            | DrawCmd::Polyline { color, .. }
            | DrawCmd::FillPolygon { color, .. }
            | DrawCmd::Arc { color, .. }
            | DrawCmd::Bezier { color, .. }
            | DrawCmd::Star { color, .. } => {
                if let Some(path) = build_vector_path_for_cmd(cmd, &rect) {
                    window.paint_path(path, to_gpui_rgba(*color));
                }
            }
            DrawCmd::Text { .. } => {}
        }
    }
}

/// Renders Layer-3 DPI-crisp text overlays (`DrawCmd::Text`) positioned inside the contained stage rect.
pub fn render_draw_text_layer(
    bounds: Option<Bounds<Pixels>>,
    logical_w: u32,
    logical_h: u32,
    cmds: &[DrawCmd],
) -> Vec<AnyElement> {
    let Some(bounds) = bounds else {
        return Vec::new();
    };
    let rect = SceneContainRect::compute(bounds, logical_w, logical_h);
    let box_ox = f32::from(bounds.origin.x);
    let box_oy = f32::from(bounds.origin.y);
    let local_ox = rect.origin_x - box_ox;
    let local_oy = rect.origin_y - box_oy;
    let scale = rect.uniform_scale();

    let mut nodes = Vec::new();
    for (idx, cmd) in cmds.iter().enumerate() {
        if let DrawCmd::Text {
            x,
            y,
            content,
            size,
            color,
            align,
        } = cmd
        {
            if !color.is_visible() || content.trim().is_empty() {
                continue;
            }
            let font_px = (*size * scale).clamp(8.0, 96.0);
            let px_x = local_ox + (*x * rect.scale_x);
            let px_y = local_oy + (*y * rect.scale_y) - font_px * 0.55;
            let text_el = match align {
                TextAlign::Center => {
                    let box_w = rect.width.max(40.0);
                    let left = (px_x - box_w * 0.5).clamp(local_ox - box_w * 0.25, local_ox + rect.width);
                    div()
                        .id(("cs-txt", idx))
                        .absolute()
                        .left(px(left))
                        .top(px(px_y))
                        .w(px(box_w))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(font_px))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(to_gpui_rgba(*color))
                                .child(content.clone()),
                        )
                        .into_any_element()
                }
                TextAlign::Right => {
                    let right_edge = (local_ox + rect.width - px_x).max(0.0);
                    div()
                        .id(("cs-txt", idx))
                        .absolute()
                        .right(px(right_edge))
                        .top(px(px_y))
                        .text_size(px(font_px))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(to_gpui_rgba(*color))
                        .child(content.clone())
                        .into_any_element()
                }
                TextAlign::Left => div()
                    .id(("cs-txt", idx))
                    .absolute()
                    .left(px(px_x))
                    .top(px(px_y))
                    .text_size(px(font_px))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(to_gpui_rgba(*color))
                    .child(content.clone())
                    .into_any_element(),
            };
            nodes.push(text_el);
        }
    }
    nodes
}

/// Live sandboxed VM & hybrid renderer state for a single active Code Scene stage.
pub struct CodeSceneViewport {
    pub active_scene_id: Option<String>,
    pub manifest: AssetManifest,
    pub vm: Option<VmInstance>,
    pub last_frame: Option<FrameOutput>,
    pub pixel_image: Option<Arc<gpui::RenderImage>>,
    pub last_pixel_revision: u64,
    pub last_tick_at: Option<Instant>,
    pub paused: bool,
    pub playback_speed: f64,
    pub game_input_captured: bool,
    pub input_state: SceneInputState,
    pub viewport_bounds: Arc<parking_lot::Mutex<Option<Bounds<Pixels>>>>,
    pub last_error: Option<String>,
    pub measured_fps: f32,
    pub audio_muted: bool,
    pub audio_bars: [f32; AUDIO_BAR_COUNT],
    pub dropped_images_count: u64,
}

impl CodeSceneViewport {
    pub fn new() -> Self {
        Self {
            active_scene_id: None,
            manifest: AssetManifest::default(),
            vm: None,
            last_frame: None,
            pixel_image: None,
            last_pixel_revision: 0,
            last_tick_at: None,
            paused: false,
            playback_speed: 1.0,
            game_input_captured: false,
            input_state: SceneInputState::default(),
            viewport_bounds: Arc::new(parking_lot::Mutex::new(None)),
            last_error: None,
            measured_fps: 60.0,
            audio_muted: false,
            audio_bars: [0.15; AUDIO_BAR_COUNT],
            dropped_images_count: 0,
        }
    }

    /// Replaces the uploaded BGRA8 `RenderImage` while releasing the previous GPU texture via `cx.drop_image`.
    pub fn swap_pixel_image(&mut self, next: Option<Arc<gpui::RenderImage>>, cx: &mut App) {
        let prev = match next {
            Some(img) => self.pixel_image.replace(img),
            None => self.pixel_image.take(),
        };
        if let Some(old_img) = prev {
            cx.drop_image(old_img, None);
            self.dropped_images_count = self.dropped_images_count.saturating_add(1);
        }
    }

    /// Loads a `CodeSceneAssetDto` (optionally reusing a pre-warmed [`VerifiedProgram`]) and renders frame 0 immediately.
    pub fn load_asset(
        &mut self,
        asset: &CodeSceneAssetDto,
        prewarmed: Option<VerifiedProgram>,
        cx: &mut App,
    ) -> Result<(), String> {
        self.clear_input_state();
        self.game_input_captured = false;
        self.paused = false;
        self.playback_speed = 1.0;
        self.last_tick_at = None;
        self.last_pixel_revision = 0;
        self.swap_pixel_image(None, cx);

        let manifest = asset.manifest.clamped();
        let verified = match prewarmed {
            Some(v) => v,
            None => compile_and_verify(&asset.source_code, &manifest).map_err(|d| d.to_string())?,
        };
        let mut instance =
            VmInstance::new(verified, VmBudgets::default()).map_err(|e| e.to_string())?;
        instance.set_playback_speed(self.playback_speed);
        let initial_frame = instance
            .step_frame(0.0, &self.input_state)
            .map_err(|e| e.to_string())?;

        self.apply_frame_output(&initial_frame, cx);
        self.active_scene_id = Some(asset.scene_id.clone());
        self.manifest = manifest;
        self.vm = Some(instance);
        self.last_frame = Some(initial_frame);
        self.last_error = None;
        Ok(())
    }

    /// Compiles and hot-reloads custom `source_code` into the running viewport without changing card index.
    pub fn hot_reload_source(
        &mut self,
        scene_id: &str,
        source_code: &str,
        manifest: &AssetManifest,
        cx: &mut App,
    ) -> Result<(), String> {
        let clamped = manifest.clamped();
        let verified = compile_and_verify(source_code, &clamped).map_err(|d| d.to_string())?;
        let mut instance =
            VmInstance::new(verified, VmBudgets::default()).map_err(|e| e.to_string())?;
        instance.set_playback_speed(self.playback_speed);
        self.last_pixel_revision = 0;
        self.swap_pixel_image(None, cx);
        let frame = instance
            .step_frame(0.0, &self.input_state)
            .map_err(|e| e.to_string())?;
        self.apply_frame_output(&frame, cx);
        self.active_scene_id = Some(scene_id.to_string());
        self.manifest = clamped;
        self.vm = Some(instance);
        self.last_frame = Some(frame);
        self.last_error = None;
        self.last_tick_at = Some(Instant::now());
        Ok(())
    }

    /// Releases active VM and GPU texture cleanly (used on logout / account switch).
    pub fn unload(&mut self, cx: &mut App) {
        self.active_scene_id = None;
        self.vm = None;
        self.last_frame = None;
        self.last_tick_at = None;
        self.last_pixel_revision = 0;
        self.game_input_captured = false;
        self.clear_input_state();
        self.swap_pixel_image(None, cx);
    }

    /// Advances the active VM by elapsed wall-clock time (clamped to `[0, 0.05]s`).
    /// Returns `(delta_ms, frame_output)` when stepped so telemetry can be updated.
    pub fn tick(&mut self, is_visible: bool, cx: &mut App) -> Option<(u64, FrameOutput)> {
        if !is_visible || self.paused {
            self.last_tick_at = None;
            return None;
        }
        let now = Instant::now();
        let raw_dt = self
            .last_tick_at
            .map(|prev| now.saturating_duration_since(prev).as_secs_f64())
            .unwrap_or(1.0 / 60.0);
        self.last_tick_at = Some(now);
        let dt = raw_dt.clamp(0.0, MAX_FRAME_DT_SEC);
        if dt > 0.0005 {
            let inst_fps = (1.0 / dt).clamp(1.0, 120.0) as f32;
            self.measured_fps = self.measured_fps * 0.85 + inst_fps * 0.15;
        }
        let delta_ms = (dt * 1000.0).round() as u64;

        let input_snapshot = self.input_state.clone();
        self.input_state.clicked = false;

        let vm = self.vm.as_mut()?;
        match vm.step_frame(dt, &input_snapshot) {
            Ok(frame) => {
                self.apply_frame_output(&frame, cx);
                self.last_frame = Some(frame.clone());
                self.last_error = None;
                Some((delta_ms, frame))
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
                self.paused = true;
                None
            }
        }
    }

    /// Deterministically seeks a Code Video timeline to `target_sec`.
    pub fn seek_video(&mut self, target_sec: f64, cx: &mut App) -> Result<(), String> {
        let vm = self
            .vm
            .as_mut()
            .ok_or_else(|| "no active VM loaded".to_string())?;
        let frame = vm.seek_video(target_sec).map_err(|e| e.to_string())?;
        self.apply_frame_output(&frame, cx);
        self.last_frame = Some(frame);
        self.last_error = None;
        self.last_tick_at = Some(Instant::now());
        Ok(())
    }

    /// Restarts the active VM back to `on_init` (used by `R` shortcut & Game Over Play Again).
    pub fn restart(&mut self, cx: &mut App) -> Result<(), String> {
        self.clear_input_state();
        if self.vm.is_none() {
            return Err("no active VM loaded".to_string());
        }
        self.last_pixel_revision = 0;
        self.swap_pixel_image(None, cx);
        let input_snapshot = self.input_state.clone();
        let frame = {
            let vm = self
                .vm
                .as_mut()
                .ok_or_else(|| "no active VM loaded".to_string())?;
            vm.restart().map_err(|e| e.to_string())?;
            vm.step_frame(0.0, &input_snapshot)
                .map_err(|e| e.to_string())?
        };
        self.apply_frame_output(&frame, cx);
        self.last_frame = Some(frame);
        self.paused = false;
        self.last_error = None;
        self.last_tick_at = Some(Instant::now());
        Ok(())
    }

    pub fn toggle_pause(&mut self) -> bool {
        self.paused = !self.paused;
        if let Some(vm) = self.vm.as_mut() {
            vm.set_paused(self.paused);
        }
        if !self.paused {
            self.last_tick_at = Some(Instant::now());
        } else {
            self.last_tick_at = None;
        }
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if let Some(vm) = self.vm.as_mut() {
            vm.set_paused(paused);
        }
        if paused {
            self.last_tick_at = None;
        }
    }

    pub fn set_playback_speed(&mut self, speed: f64) {
        self.playback_speed = speed.clamp(0.5, 2.0);
        if let Some(vm) = self.vm.as_mut() {
            vm.set_playback_speed(self.playback_speed);
        }
    }

    pub fn cycle_playback_speed(&mut self) -> f64 {
        let next = if self.playback_speed < 0.75 {
            1.0
        } else if self.playback_speed < 1.25 {
            1.5
        } else if self.playback_speed < 1.75 {
            2.0
        } else {
            0.5
        };
        self.set_playback_speed(next);
        next
    }

    pub fn set_game_input_captured(&mut self, captured: bool) {
        self.game_input_captured = captured && self.manifest.kind == AssetKind::Game;
        if !self.game_input_captured {
            self.clear_input_state();
        }
    }

    pub fn clear_input_state(&mut self) {
        self.input_state = SceneInputState::default();
    }

    pub fn update_pointer_from_window(&mut self, pt: Point<Pixels>, mouse_down: Option<bool>, clicked: bool) {
        let bounds_opt = *self.viewport_bounds.lock();
        if let Some(bounds) = bounds_opt {
            let (lx, ly) = map_point_contain_scene(pt, bounds, self.manifest.width, self.manifest.height);
            self.input_state.mouse_x = lx;
            self.input_state.mouse_y = ly;
            self.input_state.pointer_active = true;
        }
        if let Some(down) = mouse_down {
            self.input_state.mouse_down = down;
        }
        if clicked {
            self.input_state.clicked = true;
        }
    }

    pub fn set_game_key(&mut self, key: &str, pressed: bool) -> bool {
        if !self.game_input_captured {
            return false;
        }
        match key.to_ascii_lowercase().as_str() {
            "left" | "arrowleft" | "a" => {
                self.input_state.left = pressed;
                true
            }
            "right" | "arrowright" | "d" => {
                self.input_state.right = pressed;
                true
            }
            "up" | "arrowup" | "w" => {
                self.input_state.up = pressed;
                true
            }
            "down" | "arrowdown" | "s" => {
                self.input_state.down = pressed;
                true
            }
            "space" | " " | "enter" => {
                self.input_state.action = pressed;
                true
            }
            _ => false,
        }
    }

    pub fn game_telemetry(&self) -> Option<GameSessionTelemetry> {
        self.vm.as_ref().map(VmInstance::game_telemetry)
    }

    pub fn game_hud(&self) -> Option<&GameHudSnapshot> {
        self.last_frame.as_ref().and_then(|f| f.game_hud.as_ref())
    }

    pub fn timeline_progress(&self) -> (f64, Option<f64>, u32) {
        match self.last_frame.as_ref() {
            Some(f) => (f.timeline_sec, f.duration_sec, f.loop_count),
            None => (0.0, self.manifest.duration_sec, 0),
        }
    }

    fn apply_frame_output(&mut self, frame: &FrameOutput, cx: &mut App) {
        if let Some(ref pf) = frame.pixel_bgra {
            if pf.revision != self.last_pixel_revision || self.pixel_image.is_none() {
                if let Some(img) = build_render_image_from_pixel_frame(pf) {
                    self.swap_pixel_image(Some(img), cx);
                    self.last_pixel_revision = pf.revision;
                }
            }
        }
        self.update_audio_bars(&frame.audio_pulses);
    }

    fn update_audio_bars(&mut self, pulses: &[AudioPulse]) {
        for bar in &mut self.audio_bars {
            *bar = (*bar * 0.84).max(0.12);
        }
        if self.audio_muted {
            return;
        }
        for pulse in pulses {
            let norm_freq = ((pulse.freq_hz - 110.0) / 1400.0).clamp(0.0, 0.999);
            let idx = (norm_freq * (AUDIO_BAR_COUNT as f32)) as usize;
            let amp = (pulse.gain.clamp(0.15, 1.0) * 0.95).max(0.45);
            self.audio_bars[idx] = self.audio_bars[idx].max(amp);
            if idx > 0 {
                self.audio_bars[idx - 1] = self.audio_bars[idx - 1].max(amp * 0.65);
            }
            if idx + 1 < AUDIO_BAR_COUNT {
                self.audio_bars[idx + 1] = self.audio_bars[idx + 1].max(amp * 0.65);
            }
        }
    }

    /// Builds the 3-layer hybrid GPUI stage element (Layer 1 BGRA8 image + Layer 2 vector canvas + Layer 3 text HUD).
    pub fn render_stage_layers(
        &self,
        element_id: &'static str,
        should_pump_raf: bool,
        visual_offset_y: f32,
    ) -> AnyElement {
        let vp_bounds = Arc::clone(&self.viewport_bounds);
        let cached_bounds = *self.viewport_bounds.lock();
        let logical_w = self.manifest.width;
        let logical_h = self.manifest.height;
        let draw_cmds = self
            .last_frame
            .as_ref()
            .map(|f| f.draw_cmds.clone())
            .unwrap_or_default();
        let text_nodes = render_draw_text_layer(cached_bounds, logical_w, logical_h, &draw_cmds);
        let pixel_img = self.pixel_image.clone();

        div()
            .id(element_id)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x07070B))
            .child(
                div()
                    .relative()
                    .size_full()
                    .top(px(visual_offset_y))
                    // Layer 1: Optional bounded BGRA8 pixel/shader buffer
                    .when_some(pixel_img, |el, img_arc| {
                        el.child(
                            img(img_arc)
                                .absolute()
                                .inset_0()
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                    })
                    // Layer 2: 60fps GPUI vector canvas + bounds tracker + active-card-only RAF pump
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |bounds, _, window, _cx| {
                                *vp_bounds.lock() = Some(bounds);
                                paint_draw_cmds(bounds, logical_w, logical_h, &draw_cmds, window);
                                if should_pump_raf {
                                    dedup_request_animation_frame(window);
                                }
                            },
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    )
                    // Layer 3: Crisp DPI-aware text commands
                    .children(text_nodes),
            )
            .into_any_element()
    }
}

impl Default for CodeSceneViewport {
    fn default() -> Self {
        Self::new()
    }
}
