//! OpenBot WebGL/SVG theme transition shaders (`sweepTheme` `1200ms` & `beam` `1400ms`)
//! and GPUI `0.2.2` overlay elements (`OB-F05`).
//!
//! 1:1 port of upstream `src/renderer/src/theme-sweep.ts:L1-L777`:
//! - `sweepTheme` (`SWEEP_MS = 1200`, `down = before == Dark && after == Light`, `END_RADIUS = 1.9069767`)
//! - `beam` (`BEAM_MS = 1400`, `BEAM_DEADLINE_MS = 2800`, radial wave from server icon origin)
//! - Spectral `spectrum(t)` & `wash_color(t)`, 3-harmonic crest perturbation, RGB chromatic
//!   displacement `[1.08, 1.0, 0.92]`, `reduced_motion -> duration_ms = 0` (`Instant`),
//!   and BGRA8 `gpui::RenderImage` + `gpui::canvas` passthrough overlays.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, img, point, px, Bounds, Div, EntityId, Hsla, IntoElement, ObjectFit,
    ParentElement, PathBuilder, Pixels, Point, RenderImage, Rgba, Size, Styled, StyledImage,
    Window,
};
use serde::{Deserialize, Serialize};

use super::tokens::OpenBotColorScheme;

/// View-transition CSS type tag (`theme-sweep.ts:L3`).
pub const SWEEP_TYPE: &str = "theme-sweep";
/// Duration of `sweepTheme` in milliseconds (`theme-sweep.ts:L4`).
pub const SWEEP_MS: u32 = 1200;
/// Duration of server-switch `beam` in milliseconds (`theme-sweep.ts:L710`).
pub const BEAM_MS: u32 = 1400;
/// Hard deadline of `beam` in milliseconds (`BEAM_MS * 2`, `theme-sweep.ts:L711`).
pub const BEAM_DEADLINE_MS: u32 = 2800;

/// Leading Gaussian band reach (`theme-sweep.ts:L5`).
pub const AHEAD: f32 = 0.14;
/// Trailing Gaussian band reach (`theme-sweep.ts:L6`).
pub const BEHIND: f32 = 0.32;
/// Mask reveal lag behind wave crest (`theme-sweep.ts:L7`).
pub const REVEAL_LAG: f32 = 0.03;
/// Mask reveal ramp width (`theme-sweep.ts:L8`).
pub const REVEAL: f32 = 0.14;
/// Terminal normalized sweep radius `(1 + BEHIND * 2) / 0.86` (`theme-sweep.ts:L9`).
pub const END_RADIUS: f32 = 1.906_976_7;

/// Ripple crest scale (`theme-sweep.ts:L11`).
pub const CREST: f32 = 0.09;
/// Ripple forward push in px (`theme-sweep.ts:L12`).
pub const PUSH: f32 = 22.0;
/// Ripple trailing trough pull in px (`theme-sweep.ts:L13`).
pub const PULL: f32 = 8.0;
/// Lateral edge displacement gain (`theme-sweep.ts:L14`).
pub const EDGE_GAIN: f32 = 2.2;
/// Peak Gaussian blur stdDeviation in px (`theme-sweep.ts:L15`).
pub const BLUR: f32 = 9.0;
/// Chromatic channel spread (`0.08` -> `[1.08, 1.0, 0.92]`, `theme-sweep.ts:L16, L284-L286`).
pub const CHANNEL_SPREAD: f32 = 0.08;
/// Exact R/G/B chromatic displacement multipliers (`[1 + 0.08, 1.0, 1 - 0.08]`).
pub const CHROMATIC_FACTORS: [f32; 3] = [1.08, 1.0, 0.92];
/// Ripple ahead span in crest units (`theme-sweep.ts:L17`).
pub const RIPPLE_AHEAD: f32 = 4.0;
/// Ripple behind span in crest units (`theme-sweep.ts:L18`).
pub const RIPPLE_BEHIND: f32 = 6.0;

/// Radial `beam` band spread (`theme-sweep.ts:L712`).
pub const BEAM_SPREAD: f32 = 0.55;
/// Peak `beam` wash alpha (`theme-sweep.ts:L713`).
pub const BEAM_ALPHA: f32 = 0.7;
/// Radial ring crest width (`theme-sweep.ts:L471`).
pub const RING_CREST: f32 = 0.12;
/// Radial ring chromatic push in px (`theme-sweep.ts:L472`).
pub const RING_PUSH: f32 = 44.0;

const ANIM_DEDUP_MS: u64 = 8;

/// Kind of OpenBot theme/server transition (`"sweep_theme" | "beam" | "instant"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeTransitionKind {
    #[default]
    SweepTheme,
    Beam,
    Instant,
}

impl ThemeTransitionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SweepTheme => "sweep_theme",
            Self::Beam => "beam",
            Self::Instant => "instant",
        }
    }
}

/// Deterministic descriptor for an OpenBot theme sweep or server-switch beam transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeTransitionSpec {
    pub kind: ThemeTransitionKind,
    pub duration_ms: u32,
    pub down: bool,
    pub reduced_motion: bool,
}

impl ThemeTransitionSpec {
    /// Builds a `sweepTheme` spec (`1200ms` normal, `0ms` `Instant` under `reduced_motion`).
    ///
    /// Sweeps top-down (`down = true`) strictly when transitioning from `Dark` to `Light` (`theme-sweep.ts:L438`).
    pub fn sweep_theme(
        before: OpenBotColorScheme,
        after: OpenBotColorScheme,
        reduced_motion: bool,
    ) -> Self {
        let down = before == OpenBotColorScheme::Dark && after == OpenBotColorScheme::Light;
        if reduced_motion {
            Self {
                kind: ThemeTransitionKind::Instant,
                duration_ms: 0,
                down,
                reduced_motion: true,
            }
        } else {
            Self {
                kind: ThemeTransitionKind::SweepTheme,
                duration_ms: SWEEP_MS,
                down,
                reduced_motion: false,
            }
        }
    }

    /// Builds a server-switch `beam` spec (`1400ms` normal, `0ms` `Instant` under `reduced_motion`).
    pub fn beam(origin: Point<Pixels>, direction_out: bool, reduced_motion: bool) -> Self {
        let _ = origin;
        if reduced_motion {
            Self {
                kind: ThemeTransitionKind::Instant,
                duration_ms: 0,
                down: !direction_out,
                reduced_motion: true,
            }
        } else {
            Self {
                kind: ThemeTransitionKind::Beam,
                duration_ms: BEAM_MS,
                down: !direction_out,
                reduced_motion: false,
            }
        }
    }

    /// Builds a zero-duration `Instant` transition spec.
    pub const fn instant(reduced_motion: bool) -> Self {
        Self {
            kind: ThemeTransitionKind::Instant,
            duration_ms: 0,
            down: false,
            reduced_motion,
        }
    }
}

#[inline]
fn js_round_i32(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}

/// Cosine `easeInOut(t)` (`0.5 - 0.5 * cos(PI * t)`, `theme-sweep.ts:L223-L225`).
#[inline]
pub fn ease_in_out(t: f32) -> f32 {
    let clamped = t.clamp(0.0, 1.0);
    0.5 - 0.5 * (PI * clamped).cos()
}

/// Hermite `smoothstep(from, to, value)` (`theme-sweep.ts:L227-L230`).
#[inline]
pub fn smoothstep(from: f32, to: f32, value: f32) -> f32 {
    let span = to - from;
    if span.abs() <= f32::EPSILON {
        return if value >= to { 1.0 } else { 0.0 };
    }
    let t = ((value - from) / span).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Normalized sweep radius at `progress` (`max(0.001, END_RADIUS * easeInOut(p))`, `theme-sweep.ts:L430`).
#[inline]
pub fn sweep_radius_at(progress: f32) -> f32 {
    (END_RADIUS * ease_in_out(progress)).max(0.001)
}

/// Spectral wash opacity at `progress` (`0.8 * smoothstep(0, 0.12, p)`, `theme-sweep.ts:L232-L234`).
#[inline]
pub fn sweep_alpha_at(progress: f32) -> f32 {
    0.8 * smoothstep(0.0, 0.12, progress)
}

/// Cosine palette `spectrum(t)` (`FRAGMENT_SHADER`, `theme-sweep.ts:L101-L103`).
#[inline]
pub fn spectrum(t: f32) -> [f32; 3] {
    [
        0.74 + 0.50 * (TAU * (t + 0.92)).cos(),
        0.72 + 0.52 * (TAU * (t + 0.25)).cos(),
        0.76 + 0.48 * (TAU * (t + 0.58)).cos(),
    ]
}

/// Filtered 3-tap spectral wash color `washColor(t)` (`FRAGMENT_SHADER`, `theme-sweep.ts:L105-L107`).
#[inline]
pub fn wash_color(t: f32) -> [f32; 3] {
    let c0 = spectrum(t);
    let cm = spectrum(t - 0.18);
    let cp = spectrum(t + 0.18);
    [
        c0[0] * 0.5 + cm[0] * 0.25 + cp[0] * 0.25,
        c0[1] * 0.5 + cm[1] * 0.25 + cp[1] * 0.25,
        c0[2] * 0.5 + cm[2] * 0.25 + cp[2] * 0.25,
    ]
}

/// 3-harmonic horizontal crest perturbation (`theme-sweep.ts:L124-L125`).
#[inline]
pub fn sweep_crest_perturbation(uv_x: f32, time_sec: f32) -> f32 {
    1.0 + 0.08 * (uv_x * 7.0 - time_sec * 3.2).sin()
        + 0.04 * (uv_x * 15.0 + time_sec * 4.1).sin()
        + 0.02 * (uv_x * 29.0 - time_sec * 5.3).sin()
}

/// 3-harmonic angular ring perturbation for `beam` (`theme-sweep.ts:L117-L118`).
#[inline]
pub fn beam_ring_perturbation(angle_rad: f32, time_sec: f32) -> f32 {
    1.0 + 0.08 * (angle_rad * 5.0 - time_sec * 3.2).sin()
        + 0.04 * (angle_rad * 11.0 + time_sec * 4.1).sin()
        + 0.02 * (angle_rad * 23.0 - time_sec * 5.3).sin()
}

/// Displacement curve `rippleMove(u)` (`theme-sweep.ts:L50-L54`).
#[inline]
pub fn ripple_move(u: f32) -> f32 {
    let crest = (-u * u).exp();
    let trough_arg = (u + 2.6) / 1.4;
    let trough = (-(trough_arg * trough_arg)).exp();
    0.5 + (0.5 * (crest * PUSH - trough * PULL)) / PUSH
}

/// Blur envelope `rippleBlur(u)` (`theme-sweep.ts:L56-L58`).
#[inline]
pub fn ripple_blur(u: f32) -> f32 {
    let arg = (u * 0.5 + 0.2) / 0.9;
    (-(arg * arg)).exp()
}

/// Chromatic R/G/B half-displacement offsets `[round(m * 1.08), round(m), round(m * 0.92)]` (`theme-sweep.ts:L284-L286`).
#[inline]
pub fn channel_halves(move_px: f32) -> [i32; 3] {
    [
        js_round_i32(move_px * CHROMATIC_FACTORS[0]),
        js_round_i32(move_px * CHROMATIC_FACTORS[1]),
        js_round_i32(move_px * CHROMATIC_FACTORS[2]),
    ]
}

/// Chromatic R/G/B displacement halves for `sweepTheme` at `(down, progress)` (`theme-sweep.ts:L366, L380`).
#[inline]
pub fn sweep_chromatic_halves(down: bool, progress: f32) -> [i32; 3] {
    let ahead = if down { 1.0_f32 } else { -1.0_f32 };
    let strength = (PI * progress.clamp(0.0, 1.0)).sin();
    channel_halves(-ahead * PUSH * EDGE_GAIN * strength)
}

/// Runtime state and CPU shader evaluator for `sweepTheme` (`1200ms` spectral arch transition).
#[derive(Debug, Clone)]
pub struct ThemeSweepState {
    pub spec: ThemeTransitionSpec,
    pub before_scheme: OpenBotColorScheme,
    pub after_scheme: OpenBotColorScheme,
    pub down: bool,
    pub started_at: Instant,
    pub elapsed_ms: f32,
}

impl ThemeSweepState {
    /// Creates a new `ThemeSweepState`. Under `reduced_motion`, `duration_ms = 0` (`Instant`).
    pub fn new(
        before: OpenBotColorScheme,
        after: OpenBotColorScheme,
        reduced_motion: bool,
    ) -> Self {
        let spec = ThemeTransitionSpec::sweep_theme(before, after, reduced_motion);
        Self {
            spec,
            before_scheme: before,
            after_scheme: after,
            down: spec.down,
            started_at: Instant::now(),
            elapsed_ms: 0.0,
        }
    }

    /// Returns a deterministic snapshot sampled at `elapsed_ms`.
    pub fn at_elapsed_ms(&self, elapsed_ms: f32) -> Self {
        let mut copy = self.clone();
        copy.elapsed_ms = elapsed_ms.max(0.0);
        copy
    }

    /// Advances `self.elapsed_ms` from `self.started_at`.
    pub fn tick_now(&mut self) {
        self.elapsed_ms = self.started_at.elapsed().as_secs_f32() * 1000.0;
    }

    pub fn progress(&self) -> f32 {
        if self.spec.reduced_motion || self.spec.duration_ms == 0 {
            1.0
        } else {
            (self.elapsed_ms / self.spec.duration_ms as f32).clamp(0.0, 1.0)
        }
    }

    pub fn is_finished(&self) -> bool {
        self.spec.reduced_motion
            || self.spec.duration_ms == 0
            || self.elapsed_ms >= self.spec.duration_ms as f32
    }

    pub fn radius(&self) -> f32 {
        sweep_radius_at(self.progress())
    }

    pub fn alpha(&self) -> f32 {
        if self.is_finished() {
            0.0
        } else {
            sweep_alpha_at(self.progress())
        }
    }

    /// Returns `(reveal_from_pct, reveal_to_pct)` mask stops (`theme-sweep.ts:L236-L246`).
    pub fn reveal_stops_pct(&self) -> (f32, f32) {
        let r = self.radius();
        let from = (1.0 + (-REVEAL_LAG - REVEAL) / r).max(0.0) * 100.0;
        let to = (1.0 + (-REVEAL_LAG) / r).max(0.0) * 100.0;
        (from, to)
    }

    /// Returns `[red_px, green_px, blue_px]` chromatic displacement offsets (`[1.08, 1.0, 0.92]`).
    pub fn chromatic_displacement_px(&self) -> [i32; 3] {
        if self.is_finished() {
            [0, 0, 0]
        } else {
            sweep_chromatic_halves(self.down, self.progress())
        }
    }

    /// Current Gaussian blur `stdDeviation` in px (`BLUR * sin(PI * p)`).
    pub fn blur_std_deviation(&self) -> f32 {
        if self.is_finished() {
            0.0
        } else {
            BLUR * (PI * self.progress()).sin()
        }
    }
}

#[derive(Clone, Copy)]
struct ShaderUniforms {
    res_w: f32,
    res_h: f32,
    u_radius: f32,
    u_progress: f32,
    u_time: f32,
    u_down: bool,
    u_circle: bool,
    u_origin_gl: (f32, f32),
    u_spread: f32,
    u_alpha: f32,
}

fn eval_fragment_shader(u: &ShaderUniforms, screen_x: f32, screen_y: f32) -> [f32; 4] {
    let w = u.res_w.max(1.0);
    let h = u.res_h.max(1.0);
    let gl_x = screen_x.clamp(0.0, w);
    let gl_y = (h - screen_y).clamp(0.0, h);
    let uv_x = gl_x / w;
    let mut uv_y = gl_y / h;
    let t = u.u_time;

    let (s, around) = if u.u_circle {
        let px_norm = (gl_x - u.u_origin_gl.0) / h;
        let py_norm = (gl_y - u.u_origin_gl.1) / h;
        let angle = py_norm.atan2(px_norm);
        let ring = u.u_radius * beam_ring_perturbation(angle, t);
        let dist = px_norm.hypot(py_norm) - ring;
        let ar = 0.3 * smoothstep(0.05, 0.4, u.u_radius) * (angle * 3.0 + t * 1.5).sin();
        (dist, ar)
    } else {
        if u.u_down {
            uv_y = 1.0 - uv_y;
        }
        let x =
            uv_x - 0.5 + 0.05 * (uv_y * 6.0 + t * 2.5).sin() + 0.02 * (uv_y * 13.0 - t * 3.6).sin();
        let radius = u.u_radius * sweep_crest_perturbation(uv_x, t);
        let k = x.hypot(uv_y / radius.max(0.001));
        ((k - 1.0) * u.u_radius, 0.0)
    };

    let reach = ((if s > 0.0 { AHEAD } else { BEHIND }) * u.u_spread).max(1e-4);
    let alpha = (-(s * s) / (reach * reach)).exp() * u.u_alpha;
    let warp_x = uv_x + 0.08 * (uv_y * 5.0 + t * 1.7).sin();
    let warp_y = uv_y + 0.08 * (uv_x * 6.0 - t * 2.1).sin();
    let hue = u.u_progress * 0.75
        + s * 0.9
        + around
        + 0.1 * (warp_x * 7.0 + warp_y * 4.0 + t * 2.0).sin();
    let wash = wash_color(hue);
    [
        wash[0].clamp(0.0, 1.0),
        wash[1].clamp(0.0, 1.0),
        wash[2].clamp(0.0, 1.0),
        alpha.clamp(0.0, 1.0),
    ]
}

fn rasterize_shader_bgra(uniforms: &ShaderUniforms, width: u32, height: u32) -> Arc<RenderImage> {
    let w = width.max(1);
    let h = height.max(1);
    let mut img_buf = image::RgbaImage::new(w, h);
    let sx = uniforms.res_w.max(1.0) / w as f32;
    let sy = uniforms.res_h.max(1.0) / h as f32;
    for y in 0..h {
        let py = (y as f32 + 0.5) * sy;
        for x in 0..w {
            let px_coord = (x as f32 + 0.5) * sx;
            let [r, g, b, a] = eval_fragment_shader(uniforms, px_coord, py);
            let r_u8 = (r * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
            let g_u8 = (g * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
            let b_u8 = (b * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
            let a_u8 = (a * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
            // GPUI RenderImage requires BGRA8 channel ordering (`pixel.swap(0, 2)`).
            img_buf.put_pixel(x, y, image::Rgba([b_u8, g_u8, r_u8, a_u8]));
        }
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(img_buf)]))
}

impl ThemeSweepState {
    fn shader_uniforms(&self, width: f32, height: f32) -> ShaderUniforms {
        ShaderUniforms {
            res_w: width.max(1.0),
            res_h: height.max(1.0),
            u_radius: self.radius(),
            u_progress: self.progress(),
            u_time: self.elapsed_ms / 1000.0,
            u_down: self.down,
            u_circle: false,
            u_origin_gl: (0.0, 0.0),
            u_spread: 1.0,
            u_alpha: self.alpha(),
        }
    }

    /// Evaluates straight `[r, g, b, a]` in `0.0..=1.0` at screen coordinates `(x, y)`.
    pub fn sample_fragment(&self, x: f32, y: f32, width: f32, height: f32) -> [f32; 4] {
        if self.is_finished() {
            return [0.0, 0.0, 0.0, 0.0];
        }
        let u = self.shader_uniforms(width, height);
        eval_fragment_shader(&u, x, y)
    }

    /// Rasterizes the spectral wash band into a BGRA8 `gpui::RenderImage`.
    pub fn rasterize_wash_bgra(&self, width: u32, height: u32) -> Arc<RenderImage> {
        let u = self.shader_uniforms(width.max(1) as f32, height.max(1) as f32);
        rasterize_shader_bgra(&u, width, height)
    }
}

/// Runtime state and CPU shader evaluator for server-switch `beam` (`1400ms` radial wave, `2800ms` deadline).
#[derive(Debug, Clone)]
pub struct BeamWaveState {
    pub spec: ThemeTransitionSpec,
    pub origin: Point<Pixels>,
    pub viewport_size: Size<Pixels>,
    pub direction_out: bool,
    pub duration_ms: u32,
    pub deadline_ms: u32,
    pub started_at: Instant,
    pub elapsed_ms: f32,
}

impl BeamWaveState {
    /// Creates a new radial `BeamWaveState` (`1400ms` normal, `0ms` under `reduced_motion`).
    pub fn new(
        origin: Point<Pixels>,
        viewport_size: Size<Pixels>,
        direction_out: bool,
        reduced_motion: bool,
    ) -> Self {
        let spec = ThemeTransitionSpec::beam(origin, direction_out, reduced_motion);
        let duration_ms = spec.duration_ms;
        let deadline_ms = if reduced_motion { 0 } else { BEAM_DEADLINE_MS };
        Self {
            spec,
            origin,
            viewport_size,
            direction_out,
            duration_ms,
            deadline_ms,
            started_at: Instant::now(),
            elapsed_ms: 0.0,
        }
    }

    /// Returns a deterministic snapshot sampled at `elapsed_ms`.
    pub fn at_elapsed_ms(&self, elapsed_ms: f32) -> Self {
        let mut copy = self.clone();
        copy.elapsed_ms = elapsed_ms.max(0.0);
        copy
    }

    pub fn tick_now(&mut self) {
        self.elapsed_ms = self.started_at.elapsed().as_secs_f32() * 1000.0;
    }

    /// Terminal radial reach `(maxCornerDist / height + 2 * BEHIND * BEAM_SPREAD) / 0.86` (`theme-sweep.ts:L727-L733`).
    pub fn end_radius(&self) -> f32 {
        let w = f32::from(self.viewport_size.width).max(1.0);
        let h = f32::from(self.viewport_size.height).max(1.0);
        let ox = f32::from(self.origin.x);
        let oy = f32::from(self.origin.y);
        let corner = ox
            .hypot(oy)
            .max((w - ox).hypot(oy))
            .max(ox.hypot(h - oy))
            .max((w - ox).hypot(h - oy));
        (corner / h + 2.0 * BEHIND * BEAM_SPREAD) / 0.86
    }

    pub fn progress(&self) -> f32 {
        if self.spec.reduced_motion || self.duration_ms == 0 {
            1.0
        } else {
            (self.elapsed_ms / self.duration_ms as f32).clamp(0.0, 1.0)
        }
    }

    pub fn forward(&self) -> f32 {
        let p = self.progress();
        if self.direction_out {
            p
        } else {
            1.0 - p
        }
    }

    pub fn is_finished(&self) -> bool {
        self.spec.reduced_motion
            || self.duration_ms == 0
            || self.elapsed_ms >= self.duration_ms as f32
            || self.elapsed_ms >= self.deadline_ms as f32
    }

    pub fn radius(&self) -> f32 {
        (self.end_radius() * ease_in_out(self.forward())).max(0.001)
    }

    /// Radial wash alpha (`0.7 * smoothstep(0.03, 0.18, radius) * (1 - smoothstep(0.75, 1.0, forward))`, `theme-sweep.ts:L763`).
    pub fn alpha(&self) -> f32 {
        if self.is_finished() {
            return 0.0;
        }
        let r = self.radius();
        let fwd = self.forward();
        BEAM_ALPHA * smoothstep(0.03, 0.18, r) * (1.0 - smoothstep(0.75, 1.0, fwd))
    }
}

impl BeamWaveState {
    fn ring_reach(&self) -> f32 {
        let span = if self.direction_out {
            RIPPLE_AHEAD
        } else {
            RIPPLE_BEHIND
        };
        1.0 + span * RING_CREST
    }

    /// Chromatic `[red_px, green_px, blue_px]` ring displacement offsets (`theme-sweep.ts:L658-L668`).
    pub fn chromatic_displacement_px(&self) -> [i32; 3] {
        if self.is_finished() {
            return [0, 0, 0];
        }
        let sign = if self.direction_out {
            -1.0_f32
        } else {
            1.0_f32
        };
        let strength = (PI * self.progress()).sin();
        channel_halves(sign * RING_PUSH * self.ring_reach() * strength)
    }

    fn shader_uniforms(&self, width: f32, height: f32) -> ShaderUniforms {
        let vp_w = f32::from(self.viewport_size.width).max(1.0);
        let vp_h = f32::from(self.viewport_size.height).max(1.0);
        let sx = width.max(1.0) / vp_w;
        let sy = height.max(1.0) / vp_h;
        let ox = f32::from(self.origin.x) * sx;
        let oy = f32::from(self.origin.y) * sy;
        ShaderUniforms {
            res_w: width.max(1.0),
            res_h: height.max(1.0),
            u_radius: self.radius(),
            u_progress: self.progress(),
            u_time: self.elapsed_ms / 1000.0,
            u_down: false,
            u_circle: true,
            u_origin_gl: (ox, height.max(1.0) - oy),
            u_spread: BEAM_SPREAD,
            u_alpha: self.alpha(),
        }
    }

    /// Evaluates straight `[r, g, b, a]` at screen coordinates `(x, y)` (`theme-sweep.ts:L114-L136`).
    pub fn sample_fragment(&self, x: f32, y: f32, width: f32, height: f32) -> [f32; 4] {
        if self.is_finished() {
            return [0.0, 0.0, 0.0, 0.0];
        }
        let u = self.shader_uniforms(width, height);
        eval_fragment_shader(&u, x, y)
    }

    /// Rasterizes the radial `beam` spectral ring into a BGRA8 `gpui::RenderImage`.
    pub fn rasterize_wash_bgra(&self, width: u32, height: u32) -> Arc<RenderImage> {
        let u = self.shader_uniforms(width.max(1) as f32, height.max(1) as f32);
        rasterize_shader_bgra(&u, width, height)
    }
}

fn dedup_request_animation_frame(window: &Window) {
    thread_local! {
        static LAST_ANIM_REQ: RefCell<HashMap<EntityId, Instant>> = RefCell::new(HashMap::new());
    }
    let view_id = window.current_view();
    let now = Instant::now();
    let gate = Duration::from_millis(ANIM_DEDUP_MS);
    let allow = LAST_ANIM_REQ.with(|cell| {
        let mut map = cell.borrow_mut();
        if map.len() > 32 {
            let stale = Duration::from_secs(5);
            map.retain(|_, ts| now.saturating_duration_since(*ts) <= stale);
        }
        let ok = map
            .get(&view_id)
            .is_none_or(|prev| now.saturating_duration_since(*prev) >= gate);
        if ok {
            map.insert(view_id, now);
        }
        ok
    });
    if allow {
        window.request_animation_frame();
    }
}

fn paint_sweep_chromatic_crest(
    bounds: Bounds<Pixels>,
    state: &ThemeSweepState,
    window: &mut Window,
) {
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    if !bw.is_finite() || !bh.is_finite() || bw <= 1.0 || bh <= 1.0 || state.is_finished() {
        return;
    }
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let t_sec = state.elapsed_ms / 1000.0;
    let base_radius = state.radius();
    let crest_alpha = (state.alpha() * 0.55).clamp(0.0, 1.0);
    let halves = state.chromatic_displacement_px();
    let tints = [
        Rgba {
            r: 1.0,
            g: 0.40,
            b: 0.58,
            a: crest_alpha,
        },
        Rgba {
            r: 0.44,
            g: 0.98,
            b: 0.78,
            a: crest_alpha,
        },
        Rgba {
            r: 0.43,
            g: 0.72,
            b: 1.0,
            a: crest_alpha,
        },
    ];

    for (ch_idx, tint) in tints.into_iter().enumerate() {
        let dy_shift = halves[ch_idx] as f32 * 0.35;
        let mut builder = PathBuilder::stroke(px(2.25));
        const SEGMENTS: usize = 48;
        for step in 0..=SEGMENTS {
            let u = step as f32 / SEGMENTS as f32;
            let across = u - 0.5;
            let arch = (1.0 - (across * across).clamp(0.0, 0.99)).sqrt();
            let r_local = base_radius * sweep_crest_perturbation(u, t_sec) * arch;
            let norm_y = if state.down { r_local } else { 1.0 - r_local };
            let pt = point(
                px(ox + u * bw),
                px((oy + norm_y * bh + dy_shift).clamp(oy - 32.0, oy + bh + 32.0)),
            );
            if step == 0 {
                builder.move_to(pt);
            } else {
                builder.line_to(pt);
            }
        }
        if let Ok(path) = builder.build() {
            let color: Hsla = tint.into();
            window.paint_path(path, color);
        }
    }
    dedup_request_animation_frame(window);
}

fn paint_beam_chromatic_ring(bounds: Bounds<Pixels>, state: &BeamWaveState, window: &mut Window) {
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    if !bw.is_finite() || !bh.is_finite() || bw <= 1.0 || bh <= 1.0 || state.is_finished() {
        return;
    }
    let ox = f32::from(bounds.origin.x) + f32::from(state.origin.x);
    let oy = f32::from(bounds.origin.y) + f32::from(state.origin.y);
    let t_sec = state.elapsed_ms / 1000.0;
    let base_r_px = state.radius() * bh;
    let ring_alpha = (state.alpha() * 0.6).clamp(0.0, 1.0);
    let halves = state.chromatic_displacement_px();
    let tints = [
        Rgba {
            r: 1.0,
            g: 0.40,
            b: 0.58,
            a: ring_alpha,
        },
        Rgba {
            r: 0.44,
            g: 0.98,
            b: 0.78,
            a: ring_alpha,
        },
        Rgba {
            r: 0.43,
            g: 0.72,
            b: 1.0,
            a: ring_alpha,
        },
    ];

    for (ch_idx, tint) in tints.into_iter().enumerate() {
        let radial_shift = halves[ch_idx] as f32 * 0.25;
        let mut builder = PathBuilder::stroke(px(2.0));
        const SEGMENTS: usize = 64;
        for step in 0..=SEGMENTS {
            let angle = (step as f32 / SEGMENTS as f32) * TAU;
            let r_px = (base_r_px * beam_ring_perturbation(angle, t_sec) + radial_shift).max(1.0);
            let pt = point(px(ox + angle.cos() * r_px), px(oy + angle.sin() * r_px));
            if step == 0 {
                builder.move_to(pt);
            } else {
                builder.line_to(pt);
            }
        }
        builder.close();
        if let Ok(path) = builder.build() {
            let color: Hsla = tint.into();
            window.paint_path(path, color);
        }
    }
    dedup_request_animation_frame(window);
}

/// Renders a pointer-passthrough GPUI overlay for `sweepTheme` (`1200ms` spectral arch wash + chromatic crest).
pub fn render_theme_sweep_overlay(state: &ThemeSweepState) -> Div {
    if state.is_finished() {
        return div();
    }
    let wash = state.rasterize_wash_bgra(160, 96);
    let state_clone = state.clone();
    div()
        .absolute()
        .inset_0()
        .child(
            img(wash)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
        )
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_sweep_chromatic_crest(bounds, &state_clone, window);
                },
            )
            .size_full(),
        )
}

/// Renders a pointer-passthrough GPUI overlay for server-switch `beam` (`1400ms` radial spectral wave).
pub fn render_beam_overlay(state: &BeamWaveState) -> Div {
    if state.is_finished() {
        return div();
    }
    let wash = state.rasterize_wash_bgra(160, 96);
    let state_clone = state.clone();
    div()
        .absolute()
        .inset_0()
        .child(
            img(wash)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
        )
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_beam_chromatic_ring(bounds, &state_clone, window);
                },
            )
            .size_full(),
        )
}
