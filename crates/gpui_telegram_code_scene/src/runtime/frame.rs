//! Vector draw command list, NaN/Inf sanitization, and `FrameOutput`.

use serde::{Deserialize, Serialize};

use crate::model::{AudioPulse, GameHudSnapshot, RgbaColor};
use crate::runtime::pixel::PixelFrame;

pub const MAX_COORD_ABS: f64 = 16_384.0;
pub const MAX_RADIUS: f64 = 4_096.0;
pub const MIN_STROKE_WIDTH: f64 = 0.25;
pub const MAX_STROKE_WIDTH: f64 = 128.0;
pub const MIN_FONT_SIZE: f64 = 6.0;
pub const MAX_FONT_SIZE: f64 = 144.0;
pub const MAX_POLY_POINTS: usize = 256;
pub const MAX_TEXT_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "center" | "middle" => Self::Center,
            "right" | "end" => Self::Right,
            _ => Self::Left,
        }
    }
}

/// Sanitized vector draw primitives consumed by GPUI `CodeSceneViewport` in M4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DrawCmd {
    Clear {
        color: RgbaColor,
    },
    LinearGradientBg {
        top: RgbaColor,
        bottom: RgbaColor,
        angle_deg: f32,
    },
    FillRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: RgbaColor,
    },
    RoundRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: RgbaColor,
    },
    StrokeRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        stroke_width: f32,
        color: RgbaColor,
    },
    FillCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        color: RgbaColor,
    },
    StrokeCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        stroke_width: f32,
        color: RgbaColor,
    },
    GlowCircle {
        cx: f32,
        cy: f32,
        radius: f32,
        color: RgbaColor,
        intensity: f32,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        stroke_width: f32,
        color: RgbaColor,
    },
    Polyline {
        points: Vec<[f32; 2]>,
        stroke_width: f32,
        color: RgbaColor,
    },
    FillPolygon {
        points: Vec<[f32; 2]>,
        color: RgbaColor,
    },
    Arc {
        cx: f32,
        cy: f32,
        radius: f32,
        start_rad: f32,
        end_rad: f32,
        stroke_width: f32,
        color: RgbaColor,
    },
    Bezier {
        x0: f32,
        y0: f32,
        cx1: f32,
        cy1: f32,
        cx2: f32,
        cy2: f32,
        x1: f32,
        y1: f32,
        stroke_width: f32,
        color: RgbaColor,
    },
    Star {
        cx: f32,
        cy: f32,
        outer_r: f32,
        inner_r: f32,
        points: u8,
        rotation_rad: f32,
        color: RgbaColor,
    },
    Text {
        x: f32,
        y: f32,
        content: String,
        size: f32,
        color: RgbaColor,
        align: TextAlign,
    },
}

impl DrawCmd {
    /// Returns true if this command paints at least one visible non-zero-alpha element.
    pub fn is_visible(&self) -> bool {
        match self {
            Self::Clear { color } => color.is_visible(),
            Self::LinearGradientBg { top, bottom, .. } => top.is_visible() || bottom.is_visible(),
            Self::FillRect { w, h, color, .. }
            | Self::RoundRect { w, h, color, .. }
            | Self::StrokeRect { w, h, color, .. } => color.is_visible() && *w > 0.0 && *h > 0.0,
            Self::FillCircle { radius, color, .. }
            | Self::StrokeCircle { radius, color, .. }
            | Self::GlowCircle { radius, color, .. }
            | Self::Arc { radius, color, .. } => color.is_visible() && *radius > 0.0,
            Self::Line { color, .. } | Self::Bezier { color, .. } => color.is_visible(),
            Self::Polyline { points, color, .. } => color.is_visible() && points.len() >= 2,
            Self::FillPolygon { points, color } => color.is_visible() && points.len() >= 3,
            Self::Star {
                outer_r,
                points,
                color,
                ..
            } => color.is_visible() && *outer_r > 0.0 && *points >= 3,
            Self::Text { content, color, .. } => color.is_visible() && !content.trim().is_empty(),
        }
    }

    /// Verifies zero NaN or Inf floats exist anywhere in this draw command.
    pub fn all_floats_finite(&self) -> bool {
        match self {
            Self::Clear { .. } => true,
            Self::LinearGradientBg { angle_deg, .. } => angle_deg.is_finite(),
            Self::FillRect { x, y, w, h, .. } => [x, y, w, h].iter().all(|v| v.is_finite()),
            Self::RoundRect {
                x, y, w, h, radius, ..
            } => [x, y, w, h, radius].iter().all(|v| v.is_finite()),
            Self::StrokeRect {
                x,
                y,
                w,
                h,
                stroke_width,
                ..
            } => [x, y, w, h, stroke_width].iter().all(|v| v.is_finite()),
            Self::FillCircle { cx, cy, radius, .. } => {
                [cx, cy, radius].iter().all(|v| v.is_finite())
            }
            Self::StrokeCircle {
                cx,
                cy,
                radius,
                stroke_width,
                ..
            } => [cx, cy, radius, stroke_width].iter().all(|v| v.is_finite()),
            Self::GlowCircle {
                cx,
                cy,
                radius,
                intensity,
                ..
            } => [cx, cy, radius, intensity].iter().all(|v| v.is_finite()),
            Self::Line {
                x1,
                y1,
                x2,
                y2,
                stroke_width,
                ..
            } => [x1, y1, x2, y2, stroke_width].iter().all(|v| v.is_finite()),
            Self::Polyline {
                points,
                stroke_width,
                ..
            } => stroke_width.is_finite()
                && points.iter().all(|p| p[0].is_finite() && p[1].is_finite()),
            Self::FillPolygon { points, .. } => {
                points.iter().all(|p| p[0].is_finite() && p[1].is_finite())
            }
            Self::Arc {
                cx,
                cy,
                radius,
                start_rad,
                end_rad,
                stroke_width,
                ..
            } => [cx, cy, radius, start_rad, end_rad, stroke_width]
                .iter()
                .all(|v| v.is_finite()),
            Self::Bezier {
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
            } => [x0, y0, cx1, cy1, cx2, cy2, x1, y1, stroke_width]
                .iter()
                .all(|v| v.is_finite()),
            Self::Star {
                cx,
                cy,
                outer_r,
                inner_r,
                rotation_rad,
                ..
            } => [cx, cy, outer_r, inner_r, rotation_rad]
                .iter()
                .all(|v| v.is_finite()),
            Self::Text { x, y, size, .. } => [x, y, size].iter().all(|v| v.is_finite()),
        }
    }
}

/// Complete sanitized frame result returned by `VmInstance::step_frame` / `seek_video`.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameOutput {
    pub draw_cmds: Vec<DrawCmd>,
    pub pixel_bgra: Option<PixelFrame>,
    pub timeline_sec: f64,
    pub duration_sec: Option<f64>,
    pub loop_count: u32,
    pub game_hud: Option<GameHudSnapshot>,
    pub audio_pulses: Vec<AudioPulse>,
    pub ops_used: u64,
}

/// Bounded draw-command & audio-pulse collector with strict NaN/Inf sanitization.
#[derive(Debug, Clone)]
pub struct DrawListBuilder {
    cmds: Vec<DrawCmd>,
    audio_pulses: Vec<AudioPulse>,
    max_draw_cmds: usize,
    max_audio_pulses: usize,
    dropped_invalid_draws: usize,
    draw_cap_hit: bool,
}

impl DrawListBuilder {
    pub fn new(max_draw_cmds: usize, max_audio_pulses: usize) -> Self {
        Self {
            cmds: Vec::with_capacity(64),
            audio_pulses: Vec::new(),
            max_draw_cmds,
            max_audio_pulses,
            dropped_invalid_draws: 0,
            draw_cap_hit: false,
        }
    }

    pub fn push(&mut self, cmd: DrawCmd) {
        if self.cmds.len() >= self.max_draw_cmds {
            self.draw_cap_hit = true;
            return;
        }
        if !cmd.all_floats_finite() {
            self.dropped_invalid_draws += 1;
            return;
        }
        self.cmds.push(cmd);
    }

    pub fn push_audio(&mut self, pulse: AudioPulse) {
        if self.audio_pulses.len() < self.max_audio_pulses {
            self.audio_pulses.push(pulse);
        }
    }

    pub fn record_invalid_draw(&mut self) {
        self.dropped_invalid_draws += 1;
    }

    pub fn dropped_invalid_draws(&self) -> usize {
        self.dropped_invalid_draws
    }

    pub fn draw_cap_hit(&self) -> bool {
        self.draw_cap_hit
    }

    pub fn finish(self) -> (Vec<DrawCmd>, Vec<AudioPulse>, bool, usize) {
        (
            self.cmds,
            self.audio_pulses,
            self.draw_cap_hit,
            self.dropped_invalid_draws,
        )
    }
}

pub fn clamp_coord(v: f64) -> Option<f32> {
    if !v.is_finite() {
        return None;
    }
    Some(v.clamp(-MAX_COORD_ABS, MAX_COORD_ABS) as f32)
}

pub fn clamp_non_neg_size(v: f64) -> Option<f32> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    Some(v.clamp(0.0, MAX_COORD_ABS) as f32)
}

pub fn clamp_radius(v: f64) -> Option<f32> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    Some(v.clamp(0.0, MAX_RADIUS) as f32)
}

pub fn clamp_stroke(v: f64) -> Option<f32> {
    if !v.is_finite() {
        return None;
    }
    Some(v.clamp(MIN_STROKE_WIDTH, MAX_STROKE_WIDTH) as f32)
}

pub fn clamp_font_size(v: f64) -> Option<f32> {
    if !v.is_finite() {
        return None;
    }
    Some(v.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE) as f32)
}

pub fn sanitize_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_TEXT_BYTES));
    for ch in raw.chars() {
        if ch.is_control() && ch != '\t' {
            continue;
        }
        let next_len = out.len() + ch.len_utf8();
        if next_len > MAX_TEXT_BYTES {
            break;
        }
        out.push(ch);
    }
    out
}
