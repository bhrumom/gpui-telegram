//! 1:1 2D Parametric `BloubBotEngine`, `Mulberry32` PRNG, 15 state poses, `departFige` mid-morph
//! continuity, pointer gaze, sub-pixel filter, SVG export, and GPUI vector canvas renderer.

use std::f64::consts::{PI, TAU};
use std::sync::LazyLock;

use gpui::{
    canvas, div, linear_color_stop, linear_gradient, point, px, rgb, Bounds, Div, Hsla,
    ParentElement, PathBuilder, Pixels, Point, Styled, Window,
};
use openbot_contracts::{BloubExpression, BloubParametricState, BloubShape, EngineShapeId};
use serde::{Deserialize, Serialize};

use super::eyefit::{
    blend_expression, compose_eye_transforms, decalage_for, decalage_for_custom_face,
    emit_eye_capsule_gpui, expression_def, eye_capsule_svg_path, eye_matrix_svg_attr,
    fit_eyes_zero_clipping, verify_eyes_zero_clipping, ComposedEyeTransform, DecalageOffset,
    ExpressionDef, EyeSpec, GazeAngles, EYE_H_DEFAULT, EYE_SPLIT_DEFAULT, EYE_W_DEFAULT,
    REST_GAZE,
};
use super::silhouettes::{
    alert_bar_silhouette, alert_drop_points, bloub_to_engine_shape, engine_shape_radii,
    exclaim_bar_silhouette, fmt_svg_num, lerp_silhouette, play_triangle_silhouette,
    project_silhouette_points, silhouette_svg_path, Silhouette, ALERT_DROP_PATH, COMET_DOT,
    DEFAULT_PAPER, DOT_PEAK, DOT_R, DOT_X, NOTIF_BLUE, PROFILE_EGG, PROFILE_HEXAGON, RAYON,
    VIEW_BOX, VIEW_BOX_STR,
};

/// Shape & expression morph duration in seconds (`0.45`).
pub const SHAPE_MORPH_SEC: f64 = 0.45;
/// Pointer look damping duration in seconds (`LOOK_MORPH = 0.24`).
pub const LOOK_MORPH_SEC: f64 = 0.24;
/// Pointer turn-in duration in seconds (`TURN_TIME = 1.1`).
pub const TURN_TIME_SEC: f64 = 1.1;
/// Standalone spin-in tour duration in seconds (`TOUR_TIME = 1.5`).
pub const TOUR_TIME_SEC: f64 = 1.5;
/// Scheduled blink duration in seconds (`be = 0.18`).
pub const BLINK_DURATION_SEC: f64 = 0.18;
/// State entry blink duration in seconds (`At = 0.20`).
pub const BLINK_IN_DURATION_SEC: f64 = 0.20;
/// Breathing period in seconds (`3.4`).
pub const BREATH_PERIOD_SEC: f64 = 3.4;
/// Breathing vertical scale amplitude (`0.005`).
pub const BREATH_AMPLITUDE: f64 = 0.005;
/// Sub-pixel jitter threshold in device pixels (`1 / 32 = 0.03125`).
pub const SUBPIXEL_FRACTION: f64 = 1.0 / 32.0;

/// Active avatar cadence (`80` FPS).
pub const AVATAR_FPS_ACTIVE: u32 = 80;
/// Background avatar cadence (`30` FPS).
pub const AVATAR_FPS_BACKGROUND: u32 = 30;
/// Mobile orbit cache FPS (`30`).
pub const ORBIT_FPS: u32 = 30;
/// Mobile orbit cycle duration (`3.6`s).
pub const ORBIT_CYCLE_SEC: f64 = 3.6;
/// Mobile orbit phase offset (`1.2`s).
pub const ORBIT_START_SEC: f64 = 1.2;
/// Mobile orbit prebaked frame count (`108`).
pub const ORBIT_FRAME_TOTAL: usize = 108;

/// Exact unsigned `u32` Mulberry32 PRNG matching JS `Math.imul` + `>>>` bit-for-bit (`ee`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mulberry32(u32);

impl Mulberry32 {
    #[inline]
    pub const fn new(seed: u32) -> Self {
        Self(seed)
    }

    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(1_831_565_813);
        let mut e = (self.0 ^ (self.0 >> 15)).wrapping_mul(1 | self.0);
        e = e.wrapping_add((e ^ (e >> 7)).wrapping_mul(61 | e)) ^ e;
        f64::from(e ^ (e >> 14)) / 4_294_967_296.0
    }
}

/// Pointer input device classification (`event.pointerType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerKind {
    Mouse,
    Pen,
    Touch,
}

/// Head/gaze override target (`Look` in `@norbert_bodziony/bloub`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LookOverride {
    pub yaw: f64,
    pub pitch: f64,
    pub mix: f64,
    pub spin: f64,
    pub wander: f64,
    pub blink: Option<f64>,
}

impl LookOverride {
    pub const REST: Self = Self {
        yaw: 0.0,
        pitch: 0.0,
        mix: 0.0,
        spin: 0.0,
        wander: 1.0,
        blink: None,
    };

    pub const PLUSH_REST: Self = Self {
        yaw: 0.0,
        pitch: 0.0,
        mix: 0.0,
        spin: 0.0,
        wander: 0.4,
        blink: None,
    };

    pub const PLUSH_THINK: Self = Self {
        yaw: 18.0,
        pitch: 22.0,
        mix: 1.0,
        spin: 0.0,
        wander: 0.4,
        blink: None,
    };

    pub const PLUSH_ASLEEP: Self = Self {
        yaw: 0.0,
        pitch: 0.0,
        mix: 0.0,
        spin: 0.0,
        wander: 0.15,
        blink: Some(0.0),
    };
}

impl Default for LookOverride {
    fn default() -> Self {
        Self::REST
    }
}

/// Parametric seed for a 3D depth-split orbital ring/swoosh/ribbon (`ne`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitalArcSeed {
    pub a: f64,
    pub k: f64,
    pub tilt: f64,
    pub speed: f64,
    pub phase: f64,
    pub sweep: f64,
    pub hue: f64,
    pub hue_span: f64,
    pub width: f64,
    pub cx: f64,
    pub cy: f64,
}

/// Linear gradient specification in viewBox coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct ArcGradient {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub stops: [String; 3],
}

/// Depth-split orbital arc sample (`front` & `back` SVG paths + viewBox polylines).
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitalArcSample {
    pub id: String,
    pub front: String,
    pub back: String,
    pub front_polylines: Vec<Vec<(f64, f64)>>,
    pub back_polylines: Vec<Vec<(f64, f64)>>,
    pub width: f64,
    pub opacity: f64,
    pub gradient: ArcGradient,
}

/// Particle / dot ornament sample in viewBox coordinates (`[-158, 158]`).
#[derive(Debug, Clone, PartialEq)]
pub struct DotSample {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub opacity: f64,
    pub depth: Option<f64>,
    pub d: Option<String>,
    pub rot_deg: f64,
}

/// Notification badge & rim notch sample in viewBox coordinates (`[-158, 158]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotifSample {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub notch: f64,
}

/// Organic wander, float drift, breathing scale, and blink lid sample (`xe`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivelinessSample {
    pub d_yaw: f64,
    pub d_pitch: f64,
    pub d_roll: f64,
    pub drift_x: f64,
    pub drift_y: f64,
    pub breath_sy: f64,
    pub lid: f64,
}

/// Complete sampled frame from `BloubBotEngine` (`SCOPE.md §3.1`).
#[derive(Debug, Clone, PartialEq)]
pub struct BloubFrameSample {
    pub shape: EngineShapeId,
    pub state: BloubParametricState,
    pub expression: BloubExpression,
    pub time_sec: f64,
    pub silhouette: Silhouette,
    pub body_path: String,
    pub breath_sy: f64,
    pub wander_active: bool,
    pub gaze: GazeAngles,
    pub split: f64,
    pub eyes: [ComposedEyeTransform; 2],
    pub zero_clipping: bool,
    pub body_alpha: f64,
    pub eye_alpha: f64,
    pub dots: Vec<DotSample>,
    pub arcs: Vec<OrbitalArcSample>,
    pub notif: Option<NotifSample>,
    pub dots_behind: bool,
}

#[inline]
pub(crate) fn ease_out_cubic(u: f64) -> f64 {
    let inv = 1.0 - u.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

#[inline]
pub(crate) fn ease_in_out_cubic(u: f64) -> f64 {
    let t = u.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let k = -2.0 * t + 2.0;
        1.0 - (k * k * k) * 0.5
    }
}

#[inline]
pub(crate) fn ease_out_quint(u: f64) -> f64 {
    let inv = 1.0 - u.clamp(0.0, 1.0);
    let inv2 = inv * inv;
    1.0 - inv2 * inv2 * inv
}

static BLINK_SCHEDULE: LazyLock<Vec<f64>> = LazyLock::new(|| {
    let mut rng = Mulberry32::new(24301);
    let mut schedule = Vec::with_capacity(320);
    let mut t = 1.4;
    while t < 900.0 {
        schedule.push(t);
        t += 1.9 + rng.next_f64() * 2.7;
        if rng.next_f64() < 0.18 {
            schedule.push(t);
            t += 0.24;
        }
    }
    schedule
});

/// Deterministic 317-entry Mulberry32(`24301`) blink schedule over `0..900` seconds (`P`).
#[inline]
pub fn blink_schedule() -> &'static [f64] {
    BLINK_SCHEDULE.as_slice()
}

/// Evaluates scheduled eyelid openness at `t` seconds (`F(t)` in `0.0..=1.0`, `be = 0.18`s).
pub fn blink_lid_at(t: f64) -> f64 {
    if !t.is_finite() || t < 0.0 {
        return 1.0;
    }
    let query_t = if t >= 900.0 { t % 900.0 } else { t };
    for &n in blink_schedule() {
        if query_t < n {
            break;
        }
        let r = (query_t - n) / BLINK_DURATION_SEC;
        if (0.0..=1.0).contains(&r) {
            return if r < 0.45 {
                1.0 - r / 0.45
            } else {
                (r - 0.45) / 0.55
            };
        }
    }
    1.0
}

/// Thinking-dot staggered cosine pulse (`st(e, idx)`).
#[inline]
pub fn dot_pulse(e: f64, idx: usize) -> f64 {
    let n = ((((e - (idx as f64) * 0.5) / 1.5) % 1.0) + 1.0) % 1.0;
    let wave = if n < 0.5 {
        0.5 - 0.5 * (n * TAU).cos()
    } else {
        0.0
    };
    (wave * 2.0).clamp(0.0, 1.0)
}

/// 3-harmonic organic oscillator (`w(t, period, phase)`).
#[inline]
pub fn harmonic_wave(t: f64, period: f64, phase: f64) -> f64 {
    let r = (TAU * t) / period;
    0.55 * (r + phase).sin()
        + 0.30 * (2.0 * r + 1.7 * phase + 1.1).sin()
        + 0.15 * (3.0 * r + 2.3 * phase + 2.4).sin()
}

/// Samples organic gaze wander, float drift, `3.4`s breathing `sy`, and blink lid (`xe`).
pub fn liveliness_at(t: f64, wander: f64, blink: bool, float_enabled: bool) -> LivelinessSample {
    let d_yaw = (harmonic_wave(t, 11.3, 0.4) * 5.5 + harmonic_wave(t, 3.7, 2.1) * 1.6) * wander;
    let d_pitch = (harmonic_wave(t, 9.1, 1.3) * 4.2 + harmonic_wave(t, 4.3, 0.7) * 1.3) * wander;
    let d_roll = harmonic_wave(t, 13.7, 3.2) * 2.2 * wander;
    let drift_x = if float_enabled {
        harmonic_wave(t, 7.9, 1.9) * 0.006
    } else {
        0.0
    };
    let drift_y = if float_enabled {
        harmonic_wave(t, 5.3, 0.3) * 0.007
    } else {
        0.0
    };
    let breath_sy = if float_enabled {
        1.0 + ((t / BREATH_PERIOD_SEC) * TAU).sin() * BREATH_AMPLITUDE
    } else {
        1.0
    };
    let lid = if blink { blink_lid_at(t) } else { 1.0 };
    LivelinessSample {
        d_yaw,
        d_pitch,
        d_roll,
        drift_x,
        drift_y,
        breath_sy,
        lid,
    }
}

/// Converts HSL (`hue` degrees, `s`, `l` in `0..=1`) to `#rrggbb` (`te`).
pub fn hsl_to_hex(hue: f64, s: f64, l: f64) -> String {
    let h = ((hue % 360.0) + 360.0) % 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = l - c * 0.5;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let to_u8 = |v: f64| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", to_u8(r1), to_u8(g1), to_u8(b1))
}

/// Samples a 3D depth-split orbital arc (`ne(seed, t, scale, id, opacity)`).
pub fn sample_orbital_arc(
    seed: &OrbitalArcSeed,
    t: f64,
    scale: f64,
    id: String,
    opacity: f64,
) -> OrbitalArcSample {
    let a0 = seed.phase + t * seed.speed * TAU;
    let (si, co) = seed.tilt.sin_cos();
    let cz = (1.0 - seed.k * seed.k).max(0.0).sqrt();
    let span = seed.sweep * TAU;

    let mut front = String::new();
    let mut back = String::new();
    let mut front_polylines: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut back_polylines: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut prev_behind: Option<bool> = None;

    for step in 0..=64 {
        let ang = a0 + ((step as f64) / 64.0) * span;
        let (sa, ca) = ang.sin_cos();
        let mx = (seed.a * (ca * co + sa * -si * seed.k) + seed.cx) * scale;
        let my = (seed.a * (ca * si + sa * co * seed.k) + seed.cy) * scale;
        let behind = (seed.a * sa * cz) < 0.0;
        let cont = prev_behind == Some(behind);
        let cmd = if cont { 'L' } else { 'M' };

        if behind {
            back.push(cmd);
            back.push_str(&fmt_svg_num(mx));
            back.push(' ');
            back.push_str(&fmt_svg_num(my));
            if cont {
                if let Some(last) = back_polylines.last_mut() {
                    last.push((mx, my));
                }
            } else {
                back_polylines.push(vec![(mx, my)]);
            }
        } else {
            front.push(cmd);
            front.push_str(&fmt_svg_num(mx));
            front.push(' ');
            front.push_str(&fmt_svg_num(my));
            if cont {
                if let Some(last) = front_polylines.last_mut() {
                    last.push((mx, my));
                }
            } else {
                front_polylines.push(vec![(mx, my)]);
            }
        }
        prev_behind = Some(behind);
    }

    let dx = co * seed.a * scale;
    let dy = si * seed.a * scale;
    let cx = seed.cx * scale;
    let cy = seed.cy * scale;
    let gradient = ArcGradient {
        x1: super::silhouettes::js_round2(cx - dx),
        y1: super::silhouettes::js_round2(cy - dy),
        x2: super::silhouettes::js_round2(cx + dx),
        y2: super::silhouettes::js_round2(cy + dy),
        stops: [
            hsl_to_hex(seed.hue, 0.55, 0.62),
            hsl_to_hex(seed.hue + 0.5 * seed.hue_span, 0.55, 0.62),
            hsl_to_hex(seed.hue + seed.hue_span, 0.55, 0.62),
        ],
    };

    OrbitalArcSample {
        id,
        front,
        back,
        front_polylines,
        back_polylines,
        width: seed.width * scale,
        opacity: opacity.clamp(0.0, 1.0),
        gradient,
    }
}

/// Seed for a `burst` state particle (`oe`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurstParticleSeed {
    pub birth: f64,
    pub angle: f64,
    pub rho: f64,
}

/// 6 Mulberry32(`659918`) orbital rings (`re`).
pub static RINGS: LazyLock<[OrbitalArcSeed; 6]> = LazyLock::new(|| {
    let mut rng = Mulberry32::new(659_918);
    std::array::from_fn(|i| {
        let idx = i as f64;
        OrbitalArcSeed {
            a: 1.3 + rng.next_f64() * 0.1,
            k: 0.05 + rng.next_f64() * 0.4,
            tilt: (idx / 6.0) * PI + rng.next_f64() * 0.5,
            speed: 3.0 + rng.next_f64() * 0.7,
            phase: rng.next_f64() * TAU,
            sweep: 0.6 + rng.next_f64() * 0.25,
            hue: idx * 60.0 + rng.next_f64() * 30.0,
            hue_span: 60.0 + rng.next_f64() * 60.0,
            width: 0.05 + rng.next_f64() * 0.012,
            cx: 0.0,
            cy: 0.1,
        }
    })
});

/// 4 deterministic `play` state swoosh arcs (`D`).
pub static SWOOSH: LazyLock<[OrbitalArcSeed; 4]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let idx = i as f64;
        OrbitalArcSeed {
            a: 0.78 + idx * 0.2,
            k: 0.05 + idx * 0.02,
            tilt: -0.62 + idx * 0.05,
            speed: 0.3,
            phase: 0.06 * idx,
            sweep: 0.4,
            hue: 95.0 + idx * 62.0,
            hue_span: 100.0,
            width: 0.05,
            cx: 0.0,
            cy: -0.12,
        }
    })
});

/// 5 Mulberry32(`48879`) burst particle seeds (`oe`).
pub static BURST: LazyLock<[BurstParticleSeed; 5]> = LazyLock::new(|| {
    let mut rng = Mulberry32::new(48_879);
    std::array::from_fn(|i| BurstParticleSeed {
        birth: (i as f64) * 0.2,
        angle: rng.next_f64() * TAU,
        rho: 0.58 + rng.next_f64() * 0.18,
    })
});

/// 4 Mulberry32(`49383`) comet ribbon seeds (`le`).
pub static COMET_RIBBONS: LazyLock<[OrbitalArcSeed; 4]> = LazyLock::new(|| {
    let mut rng = Mulberry32::new(49_383);
    std::array::from_fn(|i| {
        let idx = i as f64;
        let n = idx - 1.5;
        OrbitalArcSeed {
            a: 0.85 * (1.0 + n * 0.03),
            k: (0.15 / 0.85) * (1.0 + n * 0.16),
            tilt: (34.0 * PI) / 180.0 + n * 0.035,
            speed: 210.0 / 360.0,
            phase: -idx * 0.045 + rng.next_f64() * 0.012,
            sweep: 0.34,
            hue: idx * 85.0 + rng.next_f64() * 20.0,
            hue_span: 80.0,
            width: 0.095,
            cx: 0.0,
            cy: 0.0,
        }
    })
});

/// Mobile orbit frame index (`bloub-orbit.ts:L59-L62`).
#[inline]
pub fn orbit_frame_index(clock_sec: f64, count: usize) -> usize {
    if count == 0 || !clock_sec.is_finite() {
        return 0;
    }
    let wrapped =
        (((clock_sec + ORBIT_START_SEC) % ORBIT_CYCLE_SEC) + ORBIT_CYCLE_SEC) % ORBIT_CYCLE_SEC;
    ((wrapped * (ORBIT_FPS as f64)).floor() as usize) % count
}

/// Standalone `1.5`s spin-in tour look (`tourLook(t)`).
pub fn tour_look(t_sec: f64) -> LookOverride {
    let u = ease_in_out_cubic((t_sec / TOUR_TIME_SEC).clamp(0.0, 1.0));
    LookOverride {
        yaw: 0.0,
        pitch: 0.0,
        mix: 0.0,
        spin: 360.0 * (1.0 - u),
        wander: 1.0,
        blink: None,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StateMeta {
    pub dur: f64,
    pub min_dur: f64,
    pub morph: f64,
    pub blink_in: bool,
    pub base_face: bool,
    pub base_body: bool,
}

/// Returns timing and composition metadata (`STATE_BY_ID`) for any of the 15 parametric states.
pub const fn state_meta(state: BloubParametricState) -> StateMeta {
    match state {
        BloubParametricState::Idle => StateMeta {
            dur: 2.4,
            min_dur: 0.6,
            morph: 0.45,
            blink_in: false,
            base_face: true,
            base_body: true,
        },
        BloubParametricState::Thinking => StateMeta {
            dur: 2.6,
            min_dur: 0.6,
            morph: 0.4,
            blink_in: true,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Wink => StateMeta {
            dur: 1.6,
            min_dur: 0.6,
            morph: 0.3,
            blink_in: true,
            base_face: false,
            base_body: true,
        },
        BloubParametricState::Wide => StateMeta {
            dur: 1.8,
            min_dur: 0.6,
            morph: 0.55,
            blink_in: true,
            base_face: false,
            base_body: true,
        },
        BloubParametricState::Alert => StateMeta {
            dur: 2.4,
            min_dur: 2.0,
            morph: 0.45,
            blink_in: false,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Notify => StateMeta {
            dur: 2.2,
            min_dur: 0.6,
            morph: 0.5,
            blink_in: true,
            base_face: false,
            base_body: true,
        },
        BloubParametricState::Exclaim => StateMeta {
            dur: 2.0,
            min_dur: 0.6,
            morph: 0.45,
            blink_in: false,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Sleep => StateMeta {
            dur: 2.4,
            min_dur: 0.6,
            morph: 0.5,
            blink_in: false,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Egg => StateMeta {
            dur: 1.8,
            min_dur: 0.6,
            morph: 0.4,
            blink_in: true,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Hexagon => StateMeta {
            dur: 1.6,
            min_dur: 0.6,
            morph: 0.4,
            blink_in: true,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Play => StateMeta {
            dur: 2.0,
            min_dur: 0.6,
            morph: 0.5,
            blink_in: true,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Orbit => StateMeta {
            dur: 3.4,
            min_dur: 2.5,
            morph: 0.6,
            blink_in: false,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Swirl => StateMeta {
            dur: 1.3,
            min_dur: 1.3,
            morph: 0.3,
            blink_in: true,
            base_face: true,
            base_body: true,
        },
        BloubParametricState::Burst => StateMeta {
            dur: 2.6,
            min_dur: 2.4,
            morph: 0.4,
            blink_in: false,
            base_face: false,
            base_body: false,
        },
        BloubParametricState::Comet => StateMeta {
            dur: 2.4,
            min_dur: 2.4,
            morph: 0.45,
            blink_in: false,
            base_face: false,
            base_body: false,
        },
    }
}

#[inline]
pub const fn state_pose_duration(state: BloubParametricState) -> f64 {
    state_meta(state).dur
}

#[inline]
pub const fn state_morph_duration(state: BloubParametricState) -> f64 {
    state_meta(state).morph
}

#[derive(Debug, Clone)]
struct RawArcSpec {
    id: String,
    seed: OrbitalArcSeed,
    t: f64,
    opacity: f64,
}

#[derive(Debug, Clone)]
struct RawPose {
    sil: Silhouette,
    off_x: f64,
    off_y: f64,
    gaze: GazeAngles,
    split: f64,
    eyes: [EyeSpec; 2],
    eye_alpha: f64,
    body_alpha: f64,
    dots: Vec<DotSample>,
    arcs: Vec<RawArcSpec>,
    notif: Option<NotifSample>,
    dots_behind: bool,
}

impl Default for RawPose {
    fn default() -> Self {
        Self {
            sil: Silhouette::circle(1.0),
            off_x: 0.0,
            off_y: 0.0,
            gaze: REST_GAZE,
            split: EYE_SPLIT_DEFAULT,
            eyes: [
                EyeSpec::new(EYE_W_DEFAULT, EYE_H_DEFAULT, 0.0, 1.0),
                EyeSpec::new(EYE_W_DEFAULT, EYE_H_DEFAULT, 0.0, 1.0),
            ],
            eye_alpha: 1.0,
            body_alpha: 1.0,
            dots: Vec::new(),
            arcs: Vec::new(),
            notif: None,
            dots_behind: false,
        }
    }
}

fn sym_open_eyes(w: f64, h: f64) -> [EyeSpec; 2] {
    [EyeSpec::new(w, h, 0.0, 1.0), EyeSpec::new(w, h, 0.0, 1.0)]
}

fn sample_burst_dots(e: f64) -> Vec<DotSample> {
    let mut out = Vec::new();
    for seed in BURST.iter() {
        let age = e - seed.birth;
        if !(0.0..=0.62).contains(&age) {
            continue;
        }
        let rad = seed.rho * 0.75_f64.powf(age * 10.0);
        let ang = seed.angle + age * ((100.0 * PI) / 180.0);
        let opacity = (age / 0.06).clamp(0.0, 1.0) * ((0.62 - age) / 0.08).clamp(0.0, 1.0);
        out.push(DotSample {
            x: ang.cos() * rad,
            y: ang.sin() * rad,
            r: 0.04 + 0.028 * (age / 0.55).clamp(0.0, 1.0),
            opacity,
            depth: Some((1.0 - rad / 0.8).clamp(0.0, 1.0)),
            d: None,
            rot_deg: 0.0,
        });
    }
    out
}

fn eval_state_raw_pose(state: BloubParametricState, elapsed: f64) -> RawPose {
    let e = elapsed.max(0.0);
    match state {
        BloubParametricState::Idle => RawPose::default(),
        BloubParametricState::Thinking => {
            let mid_p = dot_pulse(e, 1);
            let spread = 0.3 + 0.7 * ease_out_cubic((e / 0.3).clamp(0.0, 1.0));
            let center_r = DOT_R * (1.0 + (DOT_PEAK - 1.0) * mid_p);
            let mut dots = Vec::with_capacity(2);
            for idx in [0usize, 2usize] {
                let rp = dot_pulse(e, idx);
                dots.push(DotSample {
                    x: DOT_X[idx] * spread,
                    y: 0.0,
                    r: DOT_R * (1.0 + (DOT_PEAK - 1.0) * rp),
                    opacity: 0.55 + 0.45 * rp,
                    depth: None,
                    d: None,
                    rot_deg: 0.0,
                });
            }
            RawPose {
                sil: Silhouette {
                    radii: [center_r; 64],
                    cx: DOT_X[1],
                    ..Silhouette::default()
                },
                eye_alpha: 0.0,
                dots,
                ..RawPose::default()
            }
        }
        BloubParametricState::Wink => RawPose {
            gaze: GazeAngles {
                yaw: -5.37,
                pitch: 4.55,
                roll: 6.7,
            },
            split: 16.25,
            eyes: [
                EyeSpec::new(0.236, 0.464, 0.0, 1.0),
                EyeSpec::new(0.447, 0.089, 0.0, 1.0),
            ],
            ..RawPose::default()
        },
        BloubParametricState::Wide => RawPose {
            gaze: GazeAngles {
                yaw: 6.92,
                pitch: -21.96,
                roll: 11.6,
            },
            split: 18.43,
            eyes: sym_open_eyes(0.356, 0.875),
            ..RawPose::default()
        },
        BloubParametricState::Alert => {
            let t = (e / 1.5).clamp(0.0, 1.0);
            let n = ease_in_out_cubic(t) * 0.82 - 0.087;
            let r = if e > 1.6 {
                ((e - 1.6) / 0.4).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let cx = n * (1.0 - r) + 0.1 * r;
            let osc = (e * 2.5 * TAU).sin() * 0.005;
            let rot = 17.7_f64.to_radians();
            RawPose {
                sil: alert_bar_silhouette(rot, cx, -0.325 - osc),
                eye_alpha: 0.0,
                dots: vec![DotSample {
                    x: cx - rot.sin() * 0.58,
                    y: -0.325 + rot.cos() * 0.58 + osc * 2.8,
                    r: 0.118,
                    opacity: 1.0,
                    depth: None,
                    d: Some(ALERT_DROP_PATH.clone()),
                    rot_deg: 17.7,
                }],
                ..RawPose::default()
            }
        }
        BloubParametricState::Notify => {
            let t = (e / 0.45).clamp(0.0, 1.0);
            let pop = 1.0 + 0.14 * (t * PI).sin() * (1.0 - t * 0.35);
            let r = 0.15 * if t < 1.0 { pop } else { 1.0 };
            let ang = (-42.0_f64).to_radians();
            RawPose {
                gaze: GazeAngles {
                    yaw: -21.94,
                    pitch: -5.82,
                    roll: -12.2,
                },
                split: 18.89,
                eyes: sym_open_eyes(0.505, 0.498),
                notif: Some(NotifSample {
                    x: ang.cos() * 1.003,
                    y: ang.sin() * 1.003,
                    r,
                    notch: r + 0.054,
                }),
                ..RawPose::default()
            }
        }
        BloubParametricState::Exclaim => RawPose {
            sil: exclaim_bar_silhouette(),
            eye_alpha: 0.0,
            dots: vec![DotSample {
                x: -0.012,
                y: 0.526,
                r: 0.113,
                opacity: 1.0,
                depth: None,
                d: None,
                rot_deg: 0.0,
            }],
            ..RawPose::default()
        },
        BloubParametricState::Sleep => RawPose {
            sil: Silhouette {
                radii: [0.1585; 64],
                cy: 0.11 + ((TAU / 0.6) * e).sin() * 0.19,
                ..Silhouette::default()
            },
            eye_alpha: 0.0,
            ..RawPose::default()
        },
        BloubParametricState::Egg => RawPose {
            sil: Silhouette::from_radii(PROFILE_EGG),
            gaze: GazeAngles {
                yaw: 19.97,
                pitch: 26.01,
                roll: -17.1,
            },
            split: 11.07,
            eyes: sym_open_eyes(0.164, 0.385),
            ..RawPose::default()
        },
        BloubParametricState::Hexagon => RawPose {
            sil: Silhouette::from_radii(PROFILE_HEXAGON),
            gaze: GazeAngles {
                yaw: 23.11,
                pitch: 24.42,
                roll: -13.3,
            },
            split: 13.37,
            eyes: sym_open_eyes(0.177, 0.411),
            ..RawPose::default()
        },
        BloubParametricState::Play => {
            let op = (e / 0.35).clamp(0.0, 1.0) * ((2.2 - e) / 0.5).clamp(0.0, 1.0);
            let arcs = SWOOSH
                .iter()
                .enumerate()
                .map(|(k, &s)| RawArcSpec {
                    id: format!("sw{k}"),
                    seed: OrbitalArcSeed {
                        cx: 0.45 - e * 0.42,
                        ..s
                    },
                    t: e,
                    opacity: op,
                })
                .collect();
            RawPose {
                sil: play_triangle_silhouette(0.0),
                gaze: GazeAngles {
                    yaw: 12.0,
                    pitch: -8.0,
                    roll: -6.0,
                },
                split: 15.0,
                eyes: sym_open_eyes(0.18, 0.34),
                arcs,
                ..RawPose::default()
            }
        }
        BloubParametricState::Orbit => {
            let t = ease_in_out_cubic((e / 0.35).clamp(0.0, 1.0));
            let rot = -TAU * 1.25 * e * t;
            let r = ease_in_out_cubic(((e - 1.6) / 0.9).clamp(0.0, 1.0));
            let tri = play_triangle_silhouette(rot);
            let mut radii = [0.0; 64];
            for i in 0..64 {
                radii[i] = tri.radii[i] + (1.0 - tri.radii[i]) * r;
            }
            let s = (e / 0.8).clamp(0.0, 1.0) * ((3.6 - e) / 0.9).clamp(0.0, 1.0);
            let arcs = RINGS
                .iter()
                .enumerate()
                .map(|(k, &rg)| RawArcSpec {
                    id: format!("rg{k}"),
                    seed: rg,
                    t: e,
                    opacity: s * ((e - (k as f64) * 0.13) / 0.3).clamp(0.0, 1.0),
                })
                .collect();
            RawPose {
                sil: Silhouette {
                    radii,
                    rot,
                    cx: tri.cx * (1.0 - r),
                    cy: tri.cy * (1.0 - r),
                    sx: 1.0,
                    sy: 1.0,
                },
                gaze: GazeAngles {
                    yaw: 28.49 + (e * 6.5).sin() * 65.0 * (1.0 - r),
                    pitch: -4.0 + r * 32.0,
                    roll: -13.0,
                },
                eyes: sym_open_eyes(0.18, 0.34 + r * 0.07),
                arcs,
                ..RawPose::default()
            }
        }
        BloubParametricState::Swirl => {
            let arcs = RINGS[..3]
                .iter()
                .enumerate()
                .map(|(k, &rg)| RawArcSpec {
                    id: format!("sw{k}"),
                    seed: rg,
                    t: e,
                    opacity: ((e - (k as f64) * 0.06) / 0.14).clamp(0.0, 1.0)
                        * ((1.22 - e) / 0.34).clamp(0.0, 1.0),
                })
                .collect();
            RawPose {
                arcs,
                ..RawPose::default()
            }
        }
        BloubParametricState::Burst => {
            let t = 1.0 - 0.834 * ease_out_quint((e / 0.7).clamp(0.0, 1.0));
            let n = ease_out_quint(((e - 1.7) / 0.7).clamp(0.0, 1.0));
            RawPose {
                sil: Silhouette::circle(t + (1.0 - t) * n),
                eye_alpha: ((e - 1.85) / 0.4).clamp(0.0, 1.0),
                dots: sample_burst_dots(e),
                dots_behind: true,
                ..RawPose::default()
            }
        }
        BloubParametricState::Comet => {
            let t = 1.0 - (1.0 - COMET_DOT) * ease_out_quint((e / 0.55).clamp(0.0, 1.0));
            let n = ease_out_quint(((e - 1.85) / 0.6).clamp(0.0, 1.0));
            let op = ((e - 0.15) / 0.25).clamp(0.0, 1.0) * ((1.95 - e) / 0.3).clamp(0.0, 1.0);
            let arcs = COMET_RIBBONS
                .iter()
                .enumerate()
                .map(|(k, &cm)| RawArcSpec {
                    id: format!("cm{k}"),
                    seed: cm,
                    t: e,
                    opacity: op,
                })
                .collect();
            RawPose {
                sil: Silhouette {
                    radii: [t + (1.0 - t) * n; 64],
                    cy: (((e / 1.7).clamp(0.0, 1.0)) * PI).sin() * 0.035,
                    ..Silhouette::default()
                },
                eye_alpha: ((e - 2.0) / 0.35).clamp(0.0, 1.0),
                arcs,
                ..RawPose::default()
            }
        }
    }
}

fn blend_raw_pose(a: &RawPose, b: &RawPose, n: f64) -> RawPose {
    let lerp = |u: f64, v: f64| u + (v - u) * n;
    let blend_eye = |ea: EyeSpec, eb: EyeSpec| EyeSpec {
        w: lerp(ea.w, eb.w),
        h: lerp(ea.h, eb.h),
        tilt: lerp(ea.tilt, eb.tilt),
        open: lerp(ea.open, eb.open),
    };
    let mut dots = Vec::with_capacity(a.dots.len() + b.dots.len());
    for d in &a.dots {
        let mut c = d.clone();
        c.opacity *= 1.0 - n;
        if c.opacity > 1e-4 {
            dots.push(c);
        }
    }
    for d in &b.dots {
        let mut c = d.clone();
        c.opacity *= n;
        if c.opacity > 1e-4 {
            dots.push(c);
        }
    }
    let mut arcs = Vec::with_capacity(a.arcs.len() + b.arcs.len());
    for arc in &a.arcs {
        let op = arc.opacity * (1.0 - n);
        if op > 1e-4 {
            arcs.push(RawArcSpec {
                id: format!("a{}", arc.id),
                seed: arc.seed,
                t: arc.t,
                opacity: op,
            });
        }
    }
    for arc in &b.arcs {
        let op = arc.opacity * n;
        if op > 1e-4 {
            arcs.push(RawArcSpec {
                id: format!("b{}", arc.id),
                seed: arc.seed,
                t: arc.t,
                opacity: op,
            });
        }
    }
    RawPose {
        sil: lerp_silhouette(&a.sil, &b.sil, n),
        off_x: lerp(a.off_x, b.off_x),
        off_y: lerp(a.off_y, b.off_y),
        gaze: GazeAngles {
            yaw: lerp(a.gaze.yaw, b.gaze.yaw),
            pitch: lerp(a.gaze.pitch, b.gaze.pitch),
            roll: lerp(a.gaze.roll, b.gaze.roll),
        },
        split: lerp(a.split, b.split),
        eyes: [
            blend_eye(a.eyes[0], b.eyes[0]),
            blend_eye(a.eyes[1], b.eyes[1]),
        ],
        eye_alpha: lerp(a.eye_alpha, b.eye_alpha),
        body_alpha: lerp(a.body_alpha, b.body_alpha),
        dots,
        arcs,
        notif: if n < 0.5 { a.notif } else { b.notif },
        dots_behind: if n < 0.5 {
            a.dots_behind
        } else {
            b.dots_behind
        },
    }
}

/// Sub-pixel jitter suppressor (`sous-pixel.ts`, `1/32` device pixel tolerance).
#[derive(Debug, Clone, Default)]
pub struct SubPixelFilter {
    previous: Option<[ComposedEyeTransform; 2]>,
}

impl SubPixelFilter {
    #[inline]
    pub const fn new() -> Self {
        Self { previous: None }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.previous = None;
    }

    pub fn filter_eyes(
        &mut self,
        eyes: [ComposedEyeTransform; 2],
        device_pixel_width: f64,
    ) -> [ComposedEyeTransform; 2] {
        let Some(prev) = self.previous else {
            self.previous = Some(eyes);
            return eyes;
        };
        let px_w = device_pixel_width.max(1.0);
        let tolerance = (VIEW_BOX / px_w) * SUBPIXEL_FRACTION;
        let same_eye = |p: &ComposedEyeTransform, c: &ComposedEyeTransform| -> bool {
            if p.culled != c.culled
                || (p.alpha - c.alpha).abs() > 1e-6
                || (p.w - c.w).abs() > 1e-6
                || (p.h - c.h).abs() > 1e-6
                || (p.open - c.open).abs() > 1e-6
                || (p.rest_open - c.rest_open).abs() > 1e-6
            {
                return false;
            }
            if c.culled {
                return true;
            }
            let ax = c.w * 0.75;
            let ay = c.h * 0.75;
            let da = (c.matrix[0] - p.matrix[0]).abs();
            let db = (c.matrix[1] - p.matrix[1]).abs();
            let dc = (c.matrix[2] - p.matrix[2]).abs();
            let dd = (c.matrix[3] - p.matrix[3]).abs();
            let dtx = (c.matrix[4] - p.matrix[4]).abs();
            let dty = (c.matrix[5] - p.matrix[5]).abs();
            (da * ax + dc * ay + dtx).hypot(db * ax + dd * ay + dty) <= tolerance
        };

        if same_eye(&prev[0], &eyes[0]) && same_eye(&prev[1], &eyes[1]) {
            prev
        } else {
            self.previous = Some(eyes);
            eyes
        }
    }
}

#[derive(Debug, Clone)]
struct ExprWeight {
    face: ExpressionDef,
    custom: bool,
    poids: f64,
}

#[derive(Debug, Clone)]
struct ShapeWeight {
    shape: EngineShapeId,
    poids: f64,
}

/// Full 2D parametric `BloubBotEngine` (`@norbert_bodziony/bloub@0.7.0`).
#[derive(Debug, Clone)]
pub struct BloubBotEngine {
    cur_shape: EngineShapeId,
    shape_prev_radii: Option<[f64; 64]>,
    shape_depart: Vec<ShapeWeight>,
    shape_at: f64,

    cur_expr: ExpressionDef,
    cur_expr_custom: bool,
    expr_prev: Option<ExpressionDef>,
    expr_depart: Vec<ExprWeight>,
    expr_at: f64,

    cur_state: BloubParametricState,
    prev_state: Option<BloubParametricState>,
    t_cur: f64,
    t_prev: f64,
    pose_phase: f64,
    prev_pose_phase: f64,
    depart_fige: Option<RawPose>,
    decalage_fige: Option<DecalageOffset>,
    blink_at: f64,

    look_cur: LookOverride,
    look_prev: Option<LookOverride>,
    look_at: f64,
    look_morph: f64,
    pointer_active: bool,
    follow_start: Option<f64>,

    eye_scale: f64,
    initial_phase: f64,
    decalage_disabled: bool,
}

impl BloubBotEngine {
    /// Creates a new `BloubBotEngine` at rest in `(shape, expr, state)`.
    pub fn new(shape: EngineShapeId, expr: BloubExpression, state: BloubParametricState) -> Self {
        Self {
            cur_shape: shape,
            shape_prev_radii: None,
            shape_depart: Vec::new(),
            shape_at: f64::NEG_INFINITY,

            cur_expr: expression_def(expr),
            cur_expr_custom: false,
            expr_prev: None,
            expr_depart: Vec::new(),
            expr_at: f64::NEG_INFINITY,

            cur_state: state,
            prev_state: None,
            t_cur: f64::NEG_INFINITY,
            t_prev: f64::NEG_INFINITY,
            pose_phase: 0.0,
            prev_pose_phase: 0.0,
            depart_fige: None,
            decalage_fige: None,
            blink_at: f64::NEG_INFINITY,

            look_cur: LookOverride::REST,
            look_prev: None,
            look_at: f64::NEG_INFINITY,
            look_morph: LOOK_MORPH_SEC,
            pointer_active: false,
            follow_start: None,

            eye_scale: 1.0,
            initial_phase: 0.0,
            decalage_disabled: false,
        }
    }

    #[inline]
    pub fn from_bloub_shape(
        shape: BloubShape,
        expr: BloubExpression,
        state: BloubParametricState,
    ) -> Self {
        Self::new(bloub_to_engine_shape(shape), expr, state)
    }

    #[inline]
    pub fn with_eye_scale(mut self, eye_scale: f64) -> Self {
        self.set_eye_scale(eye_scale);
        self
    }

    #[inline]
    pub fn set_eye_scale(&mut self, eye_scale: f64) {
        if eye_scale.is_finite() && eye_scale > 0.0 {
            self.eye_scale = eye_scale;
        }
    }

    #[inline]
    pub fn with_initial_phase(mut self, phase_sec: f64) -> Self {
        self.set_initial_phase(phase_sec);
        self
    }

    #[inline]
    pub fn set_initial_phase(&mut self, phase_sec: f64) {
        if phase_sec.is_finite() && phase_sec >= 0.0 {
            self.initial_phase = phase_sec;
        }
    }

    #[inline]
    pub const fn shape(&self) -> EngineShapeId {
        self.cur_shape
    }

    #[inline]
    pub const fn expression(&self) -> BloubExpression {
        self.cur_expr.id
    }

    #[inline]
    pub const fn state(&self) -> BloubParametricState {
        self.cur_state
    }

    /// Transitions to `next` silhouette over `0.45`s (`easeOutQuint`) with mid-morph continuity.
    pub fn set_shape(&mut self, next: EngineShapeId, now_sec: f64) {
        if next == self.cur_shape {
            return;
        }
        let cur_radii = self.shape_radii_at(now_sec);
        let r = (now_sec - self.shape_at) / SHAPE_MORPH_SEC;
        if self.shape_prev_radii.is_some() && (0.0..1.0).contains(&r) {
            let e = ease_out_quint(r.clamp(0.0, 1.0));
            for w in &mut self.shape_depart {
                w.poids *= 1.0 - e;
            }
            self.shape_depart.retain(|w| w.poids > 1e-9);
            self.shape_depart.push(ShapeWeight {
                shape: self.cur_shape,
                poids: e,
            });
        } else {
            self.shape_depart.clear();
            self.shape_depart.push(ShapeWeight {
                shape: self.cur_shape,
                poids: 1.0,
            });
        }
        self.shape_prev_radii = Some(cur_radii);
        self.cur_shape = next;
        self.shape_at = now_sec;
    }

    /// Transitions to `next` expression over `0.45`s (`easeOutQuint`) with `expr_depart` continuity.
    pub fn set_expression(&mut self, next: BloubExpression, now_sec: f64) {
        if !self.cur_expr_custom && self.cur_expr.id == next {
            return;
        }
        self.apply_expression_transition(expression_def(next), false, now_sec);
    }

    #[inline]
    pub fn set_decalage_disabled(&mut self, disabled: bool) {
        self.decalage_disabled = disabled;
    }

    /// Sets a custom `ExpressionDef` immediately without a morph transition (for initial construction).
    pub fn set_expression_def_immediate(&mut self, face: ExpressionDef) {
        self.cur_expr = face;
        self.cur_expr_custom = true;
        self.expr_prev = None;
        self.expr_depart.clear();
        self.expr_at = f64::NEG_INFINITY;
    }

    /// Transitions to a custom `ExpressionDef` (e.g. `PlushFeltRenderer` `ASLEEP_FACE`/`FAILED_FACE`).
    pub fn set_expression_def(&mut self, face: ExpressionDef, now_sec: f64) {
        if self.cur_expr == face && self.cur_expr_custom {
            return;
        }
        self.apply_expression_transition(face, true, now_sec);
    }

    fn apply_expression_transition(&mut self, next: ExpressionDef, custom: bool, now_sec: f64) {
        let cur_face = self.expression_at(now_sec);
        let r = (now_sec - self.expr_at) / SHAPE_MORPH_SEC;
        if self.expr_prev.is_some() && (0.0..1.0).contains(&r) {
            let e = ease_out_quint(r.clamp(0.0, 1.0));
            for w in &mut self.expr_depart {
                w.poids *= 1.0 - e;
            }
            self.expr_depart.retain(|w| w.poids > 1e-9);
            self.expr_depart.push(ExprWeight {
                face: self.cur_expr,
                custom: self.cur_expr_custom,
                poids: e,
            });
        } else {
            self.expr_depart.clear();
            self.expr_depart.push(ExprWeight {
                face: self.cur_expr,
                custom: self.cur_expr_custom,
                poids: 1.0,
            });
        }
        self.expr_prev = Some(cur_face);
        self.cur_expr = next;
        self.cur_expr_custom = custom;
        self.expr_at = now_sec;
    }

    /// Transitions to `next` state, freezing `depart_fige` + `decalage_fige` on mid-morph interruption.
    pub fn set_state(&mut self, next: BloubParametricState, now_sec: f64, phase: f64) {
        if next == self.cur_state {
            return;
        }
        let cur_meta = state_meta(self.cur_state);
        let interrupted = self.prev_state.is_some() && (now_sec - self.t_cur) < cur_meta.morph;
        self.depart_fige = if interrupted {
            Some(self.pose_composee(now_sec))
        } else {
            None
        };
        self.decalage_fige = if interrupted {
            Some(self.decalage_compose(now_sec))
        } else {
            None
        };

        self.prev_state = Some(self.cur_state);
        self.t_prev = self.t_cur;
        self.prev_pose_phase = self.pose_phase;
        self.pose_phase = if phase.is_finite() { phase } else { 0.0 };
        self.cur_state = next;
        self.t_cur = now_sec;

        if state_meta(next).blink_in {
            self.blink_at = now_sec;
        }
        if !state_meta(next).base_face && self.pointer_active {
            self.pointer_active = false;
            self.follow_start = None;
            self.set_look_override(None, now_sec, TURN_TIME_SEC);
        }
    }

    /// Updates pointer gaze (`Mouse`/`Pen` -> `pointer=true, wander=0.0`; `Touch` -> `pointer=false, wander=1.0`).
    pub fn set_pointer(&mut self, kind: PointerKind, nx: f64, ny: f64, active: bool, now_sec: f64) {
        if kind == PointerKind::Touch || !active || !state_meta(self.cur_state).base_face {
            if self.pointer_active || self.follow_start.is_some() {
                self.pointer_active = false;
                self.follow_start = None;
                self.set_look_override(None, now_sec, TURN_TIME_SEC);
            }
            return;
        }
        let start = *self.follow_start.get_or_insert(now_sec);
        self.pointer_active = true;
        let tour = ease_out_quint(((now_sec - start) / TURN_TIME_SEC).clamp(0.0, 1.0));
        let nx_c = if nx.is_finite() {
            nx.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let ny_c = if ny.is_finite() {
            ny.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let target = LookOverride {
            yaw: -26.0 + nx_c * 16.0,
            pitch: 10.0 - ny_c * 13.0,
            mix: tour,
            spin: 360.0 * (1.0 - tour),
            wander: 0.0,
            blink: None,
        };
        self.set_look_override(Some(target), now_sec, LOOK_MORPH_SEC);
    }

    #[inline]
    pub const fn is_pointer_active(&self) -> bool {
        self.pointer_active
    }

    /// Applies an explicit `LookOverride` (`THINK_LOOK`/`ASLEEP_LOOK`/head-shake/tour), ignoring non-finite fields.
    pub fn set_look_override(&mut self, look: Option<LookOverride>, now_sec: f64, morph_sec: f64) {
        let current = self.look_at_time(now_sec);
        let raw = look.unwrap_or(LookOverride::REST);
        let sanitized = LookOverride {
            yaw: if raw.yaw.is_finite() {
                raw.yaw
            } else {
                current.yaw
            },
            pitch: if raw.pitch.is_finite() {
                raw.pitch
            } else {
                current.pitch
            },
            mix: if raw.mix.is_finite() {
                raw.mix.clamp(0.0, 1.0)
            } else {
                current.mix
            },
            spin: if raw.spin.is_finite() {
                raw.spin
            } else {
                current.spin
            },
            wander: if raw.wander.is_finite() {
                raw.wander.clamp(0.0, 1.0)
            } else {
                current.wander
            },
            blink: raw
                .blink
                .filter(|b| b.is_finite())
                .map(|b| b.clamp(0.0, 1.0)),
        };
        self.look_prev = Some(current);
        self.look_cur = sanitized;
        self.look_at = now_sec;
        self.look_morph = if morph_sec.is_finite() && morph_sec > 1e-6 {
            morph_sec
        } else {
            LOOK_MORPH_SEC
        };
    }

    /// Sets the active `LookOverride` immediately without introducing an extra morph layer.
    pub fn set_look_immediate(&mut self, look: LookOverride) {
        self.look_prev = None;
        self.look_cur = LookOverride {
            yaw: if look.yaw.is_finite() { look.yaw } else { 0.0 },
            pitch: if look.pitch.is_finite() { look.pitch } else { 0.0 },
            mix: if look.mix.is_finite() { look.mix.clamp(0.0, 1.0) } else { 0.0 },
            spin: if look.spin.is_finite() { look.spin } else { 0.0 },
            wander: if look.wander.is_finite() { look.wander.clamp(0.0, 1.0) } else { 0.0 },
            blink: look.blink.filter(|b| b.is_finite()).map(|b| b.clamp(0.0, 1.0)),
        };
        self.look_at = f64::NEG_INFINITY;
    }

    /// Samples the avatar frame at `now_sec` (or canonical frozen pose when `reduced_motion == true`).
    pub fn sample(&self, now_sec: f64, reduced_motion: bool) -> BloubFrameSample {
        if reduced_motion {
            let frozen_t = if self.cur_state == BloubParametricState::Idle {
                0.0
            } else {
                self.cur_state.canonical_pose_time()
            };
            return self.sample_frozen_at(frozen_t);
        }
        let effective_t = (if now_sec.is_finite() { now_sec } else { 0.0 }) + self.initial_phase;
        self.compose_frame_at(effective_t, false)
    }

    /// Samples a static frozen pose (`frozenAt`) with organic wander/float/blink disabled.
    pub fn sample_frozen_at(&self, frozen_sec: f64) -> BloubFrameSample {
        let t = if frozen_sec.is_finite() {
            frozen_sec.max(0.0)
        } else {
            0.0
        };
        self.compose_frame_at(t, true)
    }

    fn shape_radii_at(&self, t: f64) -> [f64; 64] {
        let target = engine_shape_radii(self.cur_shape);
        let Some(prev) = self.shape_prev_radii else {
            return target;
        };
        let r = (t - self.shape_at) / SHAPE_MORPH_SEC;
        if r >= 1.0 || !r.is_finite() {
            return target;
        }
        let u = ease_out_quint(r.clamp(0.0, 1.0));
        let mut out = [0.0; 64];
        for i in 0..64 {
            out[i] = prev[i] + (target[i] - prev[i]) * u;
        }
        out
    }

    fn expression_at(&self, t: f64) -> ExpressionDef {
        let Some(prev) = self.expr_prev else {
            return self.cur_expr;
        };
        let r = (t - self.expr_at) / SHAPE_MORPH_SEC;
        if r >= 1.0 || !r.is_finite() {
            return self.cur_expr;
        }
        blend_expression(&prev, &self.cur_expr, ease_out_quint(r.clamp(0.0, 1.0)))
    }

    fn eval_single_state(&self, state: BloubParametricState, elapsed: f64, t: f64) -> RawPose {
        let mut pose = eval_state_raw_pose(state, elapsed);
        let meta = state_meta(state);
        if meta.base_body {
            pose.sil.radii = self.shape_radii_at(t);
        }
        if meta.base_face {
            let face = self.expression_at(t);
            pose.gaze = face.gaze;
            pose.split = face.split;
            pose.eyes = face.eyes;
        }
        pose
    }

    fn pose_composee(&self, t: f64) -> RawPose {
        let cur_meta = state_meta(self.cur_state);
        let elapsed_cur = if self.t_cur.is_finite() {
            (t - self.t_cur).max(0.0) + self.pose_phase
        } else {
            t.max(0.0) + self.pose_phase
        };
        let b = self.eval_single_state(self.cur_state, elapsed_cur, t);
        let Some(prev_st) = self.prev_state else {
            return b;
        };
        let r = (t - self.t_cur) / cur_meta.morph;
        if r >= 1.0 || !r.is_finite() {
            return b;
        }
        let u = ease_out_quint(r.clamp(0.0, 1.0));
        let a = if let Some(ref frozen) = self.depart_fige {
            frozen.clone()
        } else {
            let elapsed_prev = if self.t_prev.is_finite() {
                (t - self.t_prev).max(0.0) + self.prev_pose_phase
            } else {
                t.max(0.0) + self.prev_pose_phase
            };
            self.eval_single_state(prev_st, elapsed_prev, t)
        };
        blend_raw_pose(&a, &b, u)
    }

    fn decalage_for_face_entry(
        shape: EngineShapeId,
        state: BloubParametricState,
        face: &ExpressionDef,
        custom: bool,
    ) -> DecalageOffset {
        if custom {
            decalage_for_custom_face(shape, state, face)
        } else {
            decalage_for(shape, state, Some(face.id))
        }
    }

    fn decalage_for_shape_and_state(
        &self,
        shape: EngineShapeId,
        state: BloubParametricState,
        t: f64,
    ) -> DecalageOffset {
        let meta = state_meta(state);
        if !meta.base_body {
            return DecalageOffset::default();
        }
        if !meta.base_face {
            return decalage_for(shape, state, None);
        }
        let target =
            Self::decalage_for_face_entry(shape, state, &self.cur_expr, self.cur_expr_custom);
        if self.expr_prev.is_none() || self.expr_depart.is_empty() {
            return target;
        }
        let r = (t - self.expr_at) / SHAPE_MORPH_SEC;
        if r >= 1.0 || !r.is_finite() {
            return target;
        }
        let e = ease_out_quint(r.clamp(0.0, 1.0));
        let mut sx = target.x * e;
        let mut sy = target.y * e;
        for w in &self.expr_depart {
            let d = Self::decalage_for_face_entry(shape, state, &w.face, w.custom);
            let weight = w.poids * (1.0 - e);
            sx += d.x * weight;
            sy += d.y * weight;
        }
        DecalageOffset { x: sx, y: sy }
    }

    fn decalage_for_single_state(&self, state: BloubParametricState, t: f64) -> DecalageOffset {
        let target = self.decalage_for_shape_and_state(self.cur_shape, state, t);
        if self.shape_prev_radii.is_none() || self.shape_depart.is_empty() {
            return target;
        }
        let r = (t - self.shape_at) / SHAPE_MORPH_SEC;
        if r >= 1.0 || !r.is_finite() {
            return target;
        }
        let e = ease_out_quint(r.clamp(0.0, 1.0));
        let mut sx = target.x * e;
        let mut sy = target.y * e;
        for w in &self.shape_depart {
            let d = self.decalage_for_shape_and_state(w.shape, state, t);
            let weight = w.poids * (1.0 - e);
            sx += d.x * weight;
            sy += d.y * weight;
        }
        DecalageOffset { x: sx, y: sy }
    }

    fn decalage_compose(&self, t: f64) -> DecalageOffset {
        let db = self.decalage_for_single_state(self.cur_state, t);
        let Some(prev_st) = self.prev_state else {
            return db;
        };
        let cur_meta = state_meta(self.cur_state);
        let r = (t - self.t_cur) / cur_meta.morph;
        if r >= 1.0 || !r.is_finite() {
            return db;
        }
        let u = ease_out_quint(r.clamp(0.0, 1.0));
        let da = self
            .decalage_fige
            .unwrap_or_else(|| self.decalage_for_single_state(prev_st, t));
        DecalageOffset {
            x: da.x + (db.x - da.x) * u,
            y: da.y + (db.y - da.y) * u,
        }
    }

    fn look_at_time(&self, t: f64) -> LookOverride {
        let Some(prev) = self.look_prev else {
            return self.look_cur;
        };
        let r = (t - self.look_at) / self.look_morph.max(1e-6);
        if r >= 1.0 || !r.is_finite() {
            return self.look_cur;
        }
        let u = ease_out_quint(r.clamp(0.0, 1.0));
        let lerp = |a: f64, b: f64| a + (b - a) * u;
        let blink = if prev.blink.is_some() || self.look_cur.blink.is_some() {
            Some(lerp(
                prev.blink.unwrap_or(1.0),
                self.look_cur.blink.unwrap_or(1.0),
            ))
        } else {
            None
        };
        LookOverride {
            yaw: lerp(prev.yaw, self.look_cur.yaw),
            pitch: lerp(prev.pitch, self.look_cur.pitch),
            mix: lerp(prev.mix, self.look_cur.mix),
            spin: lerp(prev.spin, self.look_cur.spin),
            wander: lerp(prev.wander, self.look_cur.wander),
            blink,
        }
    }

    fn compose_frame_at(&self, t: f64, frozen: bool) -> BloubFrameSample {
        let mut pose = if frozen {
            let mut p = eval_state_raw_pose(self.cur_state, t);
            let meta = state_meta(self.cur_state);
            if meta.base_body {
                p.sil.radii = engine_shape_radii(self.cur_shape);
            }
            if meta.base_face {
                p.gaze = self.cur_expr.gaze;
                p.split = self.cur_expr.split;
                p.eyes = self.cur_expr.eyes;
            }
            p
        } else {
            self.pose_composee(t)
        };

        let decalage = if self.decalage_disabled {
            DecalageOffset::default()
        } else if frozen {
            Self::decalage_for_face_entry(
                self.cur_shape,
                self.cur_state,
                &self.cur_expr,
                self.cur_expr_custom,
            )
        } else {
            self.decalage_compose(t)
        };
        let decalage = if self.decalage_disabled {
            DecalageOffset::default()
        } else if state_meta(self.cur_state).base_body || !frozen {
            if frozen && !state_meta(self.cur_state).base_face {
                decalage_for(self.cur_shape, self.cur_state, None)
            } else {
                decalage
            }
        } else {
            DecalageOffset::default()
        };

        let look = if frozen {
            self.look_cur
        } else {
            self.look_at_time(t)
        };
        let eyes_visible = pose.eye_alpha > 0.01;
        let eff_wander = if !frozen && eyes_visible {
            look.wander
        } else {
            0.0
        };
        let life = liveliness_at(t, eff_wander, !frozen && eyes_visible, !frozen);

        let effective_lid = if frozen {
            1.0
        } else {
            let p = ((t - self.blink_at) / BLINK_IN_DURATION_SEC).clamp(0.0, 1.0);
            let m = if self.blink_at.is_finite() && p < 1.0 {
                (2.0 * p - 1.0).abs()
            } else {
                1.0
            };
            let h = life.lid.min(m);
            match look.blink {
                None => h,
                Some(b) if (b - 1.0).abs() < 1e-12 => h,
                Some(b) => 1.0 - (1.0 - h) * b.clamp(0.0, 1.0),
            }
        };

        pose.sil.sy *= life.breath_sy;
        let gaze = GazeAngles {
            yaw: pose.gaze.yaw + (look.yaw - pose.gaze.yaw) * look.mix + life.d_yaw - look.spin,
            pitch: pose.gaze.pitch + (look.pitch - pose.gaze.pitch) * look.mix + life.d_pitch,
            roll: pose.gaze.roll + life.d_roll,
        };
        let mut eyes = compose_eye_transforms(
            &pose.sil,
            gaze,
            pose.split,
            &pose.eyes,
            pose.eye_alpha,
            effective_lid,
            pose.off_x + life.drift_x,
            pose.off_y + life.drift_y,
            decalage,
            RAYON,
            self.eye_scale,
        );
        let zero_clipping = if !self.decalage_disabled && state_meta(self.cur_state).base_body {
            fit_eyes_zero_clipping(&pose.sil, &mut eyes)
        } else {
            verify_eyes_zero_clipping(&pose.sil, &eyes)
        };
        let body_path = silhouette_svg_path(&pose.sil, RAYON);

        let dots = pose
            .dots
            .into_iter()
            .map(|d| DotSample {
                x: d.x * RAYON,
                y: d.y * RAYON,
                r: d.r * RAYON,
                ..d
            })
            .collect();
        let arcs = pose
            .arcs
            .into_iter()
            .map(|a| sample_orbital_arc(&a.seed, a.t, RAYON, a.id, a.opacity))
            .collect();
        let notif = pose.notif.map(|n| NotifSample {
            x: n.x * RAYON,
            y: n.y * RAYON,
            r: n.r * RAYON,
            notch: n.notch * RAYON,
        });

        BloubFrameSample {
            shape: self.cur_shape,
            state: self.cur_state,
            expression: self.cur_expr.id,
            time_sec: t,
            silhouette: pose.sil,
            body_path,
            breath_sy: life.breath_sy,
            wander_active: !frozen && eyes_visible && eff_wander > 1e-6,
            gaze,
            split: pose.split,
            eyes,
            zero_clipping,
            body_alpha: pose.body_alpha,
            eye_alpha: pose.eye_alpha,
            dots,
            arcs,
            notif,
            dots_behind: pose.dots_behind,
        }
    }
}

fn parse_hex_u32(raw: &str, fallback: u32) -> u32 {
    let s = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    match s.len() {
        6 | 8 => u32::from_str_radix(&s[..6], 16).unwrap_or(fallback),
        3 => {
            let mut out = 0u32;
            for b in s.bytes() {
                let Some(d) = (b as char).to_digit(16) else {
                    return fallback;
                };
                out = (out << 8) | (d << 4) | d;
            }
            out
        }
        _ => fallback,
    }
}

fn mix_hex_str(a_hex: &str, b_hex: &str, t: f64) -> String {
    let u = t.clamp(0.0, 1.0);
    let ca = parse_hex_u32(a_hex, 0xf9f9f9);
    let cb = parse_hex_u32(b_hex, 0x0a0a0c);
    let ch = |shift: u32| -> u8 {
        let va = ((ca >> shift) & 0xff) as f64;
        let vb = ((cb >> shift) & 0xff) as f64;
        (va + (vb - va) * u).round().clamp(0.0, 255.0) as u8
    };
    format!("#{:02x}{:02x}{:02x}", ch(16), ch(8), ch(0))
}

fn append_svg_dots(out: &mut String, dots: &[DotSample], ink_hex: &str, paper_hex: &str) {
    for dot in dots {
        if dot.opacity <= 0.001 {
            continue;
        }
        let fill = match dot.depth {
            Some(depth) => mix_hex_str(paper_hex, ink_hex, depth),
            None => ink_hex.to_string(),
        };
        if let Some(ref d_path) = dot.d {
            out.push_str(&format!(
                "<path d=\"{}\" transform=\"translate({} {}) rotate({}) scale(100)\" fill=\"{}\" opacity=\"{}\"/>",
                d_path,
                fmt_svg_num(dot.x),
                fmt_svg_num(dot.y),
                fmt_svg_num(dot.rot_deg),
                fill,
                fmt_svg_num(dot.opacity),
            ));
        } else {
            out.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" opacity=\"{}\"/>",
                fmt_svg_num(dot.x),
                fmt_svg_num(dot.y),
                fmt_svg_num(dot.r),
                fill,
                fmt_svg_num(dot.opacity),
            ));
        }
    }
}

/// Renders a complete standalone SVG (`viewBox="-158 -158 316 316"`) for a flat `BloubFrameSample`.
pub fn render_flat_svg(sample: &BloubFrameSample, ink_hex: &str, paper_hex: &str) -> String {
    let ink = if ink_hex.trim().is_empty() {
        "#0a0a0c"
    } else {
        ink_hex.trim()
    };
    let paper = if paper_hex.trim().is_empty() {
        DEFAULT_PAPER
    } else {
        paper_hex.trim()
    };

    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{VIEW_BOX_STR}\" aria-hidden=\"true\" focusable=\"false\">"
    ));

    out.push_str("<defs>");
    for (idx, arc) in sample.arcs.iter().enumerate() {
        let g = &arc.gradient;
        out.push_str(&format!(
            "<linearGradient id=\"bloub-grad-{idx}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">",
            fmt_svg_num(g.x1),
            fmt_svg_num(g.y1),
            fmt_svg_num(g.x2),
            fmt_svg_num(g.y2),
        ));
        out.push_str(&format!(
            "<stop offset=\"0%\" stop-color=\"{}\"/>",
            g.stops[0]
        ));
        out.push_str(&format!(
            "<stop offset=\"50%\" stop-color=\"{}\"/>",
            g.stops[1]
        ));
        out.push_str(&format!(
            "<stop offset=\"100%\" stop-color=\"{}\"/>",
            g.stops[2]
        ));
        out.push_str("</linearGradient>");
    }
    out.push_str("<mask id=\"bloub-body-mask\"><rect x=\"-158\" y=\"-158\" width=\"316\" height=\"316\" fill=\"#ffffff\"/>");
    for eye in &sample.eyes {
        if !eye.culled && eye.alpha > 0.001 {
            out.push_str(&format!(
                "<path d=\"{}\" transform=\"{}\" fill=\"#000000\" fill-opacity=\"{}\"/>",
                eye_capsule_svg_path(eye.w, eye.h),
                eye_matrix_svg_attr(&eye.matrix),
                fmt_svg_num(eye.alpha),
            ));
        }
    }
    if let Some(notif) = sample.notif {
        out.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#000000\"/>",
            fmt_svg_num(notif.x),
            fmt_svg_num(notif.y),
            fmt_svg_num(notif.notch),
        ));
    }
    out.push_str("</mask>");
    if let Some(notif) = sample.notif {
        out.push_str(&format!(
            "<mask id=\"bloub-paper-mask\"><rect x=\"-158\" y=\"-158\" width=\"316\" height=\"316\" fill=\"#ffffff\"/><circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#000000\"/></mask>",
            fmt_svg_num(notif.x),
            fmt_svg_num(notif.y),
            fmt_svg_num(notif.notch),
        ));
    }
    out.push_str("</defs>");

    out.push_str("<g data-layer=\"arcs-back\">");
    for (idx, arc) in sample.arcs.iter().enumerate() {
        if !arc.back.is_empty() && arc.opacity > 0.001 {
            out.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"url(#bloub-grad-{idx})\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\"{}\"/>",
                arc.back,
                fmt_svg_num(arc.width),
                fmt_svg_num(arc.opacity),
            ));
        }
    }
    out.push_str("</g>");

    if sample.dots_behind {
        out.push_str("<g data-layer=\"dots-back\">");
        append_svg_dots(&mut out, &sample.dots, ink, paper);
        out.push_str("</g>");
    }

    out.push_str("<g data-layer=\"body\">");
    if sample.notif.is_some() {
        out.push_str(&format!(
            "<path d=\"{}\" fill=\"{paper}\" opacity=\"{}\" mask=\"url(#bloub-paper-mask)\"/>",
            sample.body_path,
            fmt_svg_num(sample.body_alpha),
        ));
    } else {
        out.push_str(&format!(
            "<path d=\"{}\" fill=\"{paper}\" opacity=\"{}\"/>",
            sample.body_path,
            fmt_svg_num(sample.body_alpha),
        ));
    }
    out.push_str(&format!(
        "<path d=\"{}\" fill=\"{ink}\" opacity=\"{}\" mask=\"url(#bloub-body-mask)\"/>",
        sample.body_path,
        fmt_svg_num(sample.body_alpha),
    ));
    out.push_str("</g>");

    if !sample.dots_behind && !sample.dots.is_empty() {
        out.push_str("<g data-layer=\"dots-front\">");
        append_svg_dots(&mut out, &sample.dots, ink, paper);
        out.push_str("</g>");
    }

    if let Some(notif) = sample.notif {
        out.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{NOTIF_BLUE}\"/>",
            fmt_svg_num(notif.x),
            fmt_svg_num(notif.y),
            fmt_svg_num(notif.r),
        ));
    }

    out.push_str("<g data-layer=\"arcs-front\">");
    for (idx, arc) in sample.arcs.iter().enumerate() {
        if !arc.front.is_empty() && arc.opacity > 0.001 {
            out.push_str(&format!(
                "<path d=\"{}\" fill=\"none\" stroke=\"url(#bloub-grad-{idx})\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\"{}\"/>",
                arc.front,
                fmt_svg_num(arc.width),
                fmt_svg_num(arc.opacity),
            ));
        }
    }
    out.push_str("</g></svg>");
    out
}

fn emit_vb_circle(
    builder: &mut PathBuilder,
    center: Point<Pixels>,
    px_per_vb: f32,
    vx: f64,
    vy: f64,
    vr: f64,
) {
    const K: f32 = 0.55228475;
    let cx = f32::from(center.x) + (vx as f32) * px_per_vb;
    let cy = f32::from(center.y) + (vy as f32) * px_per_vb;
    let r = (vr as f32) * px_per_vb;
    let k = r * K;

    builder.move_to(point(px(cx), px(cy - r)));
    builder.cubic_bezier_to(
        point(px(cx + r), px(cy)),
        point(px(cx + k), px(cy - r)),
        point(px(cx + r), px(cy - k)),
    );
    builder.cubic_bezier_to(
        point(px(cx), px(cy + r)),
        point(px(cx + r), px(cy + k)),
        point(px(cx + k), px(cy + r)),
    );
    builder.cubic_bezier_to(
        point(px(cx - r), px(cy)),
        point(px(cx - k), px(cy + r)),
        point(px(cx - r), px(cy + k)),
    );
    builder.cubic_bezier_to(
        point(px(cx), px(cy - r)),
        point(px(cx - r), px(cy - k)),
        point(px(cx - k), px(cy - r)),
    );
    builder.close();
}

fn build_notched_body_gpui_path(
    sil: &Silhouette,
    notif: Option<NotifSample>,
    center: Point<Pixels>,
    px_per_vb: f32,
) -> Option<gpui::Path<Pixels>> {
    let Some(n) = notif else {
        return super::silhouettes::silhouette_gpui_path(sil, center, px_per_vb);
    };
    let anchors = project_silhouette_points(sil, RAYON);
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let mut builder = PathBuilder::fill();
    let mut first = true;

    for r in 0..64 {
        let a = anchors[(r + 63) % 64];
        let o = anchors[r];
        let s = anchors[(r + 1) % 64];
        let c = anchors[(r + 2) % 64];
        let c1 = (o.0 + (s.0 - a.0) / 6.0, o.1 + (s.1 - a.1) / 6.0);
        let c2 = (s.0 - (c.0 - o.0) / 6.0, s.1 - (c.1 - o.1) / 6.0);
        for sub in 0..4 {
            let t = (sub as f64) * 0.25;
            let u = 1.0 - t;
            let mut vx =
                u * u * u * o.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * s.0;
            let mut vy =
                u * u * u * o.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * s.1;
            let dx = vx - n.x;
            let dy = vy - n.y;
            let dist = dx.hypot(dy);
            if dist < n.notch {
                if dist > 1e-6 {
                    vx = n.x + (dx / dist) * n.notch;
                    vy = n.y + (dy / dist) * n.notch;
                } else {
                    let norm = n.x.hypot(n.y).max(1e-6);
                    vx = n.x - (n.x / norm) * n.notch;
                    vy = n.y - (n.y / norm) * n.notch;
                }
            }
            let pt = point(
                px(cx + (vx as f32) * px_per_vb),
                px(cy + (vy as f32) * px_per_vb),
            );
            if first {
                builder.move_to(pt);
                first = false;
            } else {
                builder.line_to(pt);
            }
        }
    }
    builder.close();
    builder.build().ok()
}

fn paint_arc_layer(
    arcs: &[OrbitalArcSample],
    back: bool,
    center: Point<Pixels>,
    px_per_vb: f32,
    window: &mut Window,
) {
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    for arc in arcs {
        if arc.opacity <= 0.001 {
            continue;
        }
        let polylines = if back {
            &arc.back_polylines
        } else {
            &arc.front_polylines
        };
        if polylines.is_empty() {
            continue;
        }
        let stroke_px = ((arc.width as f32) * px_per_vb).max(0.75);
        let c0: Hsla = rgb(parse_hex_u32(&arc.gradient.stops[0], 0x3b93f0)).into();
        let c2: Hsla = rgb(parse_hex_u32(&arc.gradient.stops[2], 0x8b5cf6)).into();
        let op = arc.opacity.clamp(0.0, 1.0) as f32;
        let angle_deg = (arc.gradient.y2 - arc.gradient.y1)
            .atan2(arc.gradient.x2 - arc.gradient.x1)
            .to_degrees() as f32
            + 90.0;

        for poly in polylines {
            if poly.len() < 2 {
                continue;
            }
            let mut sb = PathBuilder::stroke(px(stroke_px));
            sb.move_to(point(
                px(cx + (poly[0].0 as f32) * px_per_vb),
                px(cy + (poly[0].1 as f32) * px_per_vb),
            ));
            for &(vx, vy) in &poly[1..] {
                sb.line_to(point(
                    px(cx + (vx as f32) * px_per_vb),
                    px(cy + (vy as f32) * px_per_vb),
                ));
            }
            if let Ok(path) = sb.build() {
                window.paint_path(
                    path,
                    linear_gradient(
                        angle_deg,
                        linear_color_stop(c0.opacity(op), 0.0),
                        linear_color_stop(c2.opacity(op), 1.0),
                    ),
                );
            }
        }
    }
}

fn paint_dots_gpui(
    dots: &[DotSample],
    ink_hex: &str,
    paper_hex: &str,
    center: Point<Pixels>,
    px_per_vb: f32,
    window: &mut Window,
) {
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    for dot in dots {
        if dot.opacity <= 0.001 {
            continue;
        }
        let hex = match dot.depth {
            Some(depth) => mix_hex_str(paper_hex, ink_hex, depth),
            None => ink_hex.to_string(),
        };
        let fill: Hsla = rgb(parse_hex_u32(&hex, 0x0a0a0c)).into();
        let op = dot.opacity.clamp(0.0, 1.0) as f32;

        if dot.d.is_some() {
            let pts = alert_drop_points();
            let (sr, cr) = dot.rot_deg.to_radians().sin_cos();
            let mut b = PathBuilder::fill();
            for (idx, &(lx, ly)) in pts.iter().enumerate() {
                let rx = (lx * cr - ly * sr) * RAYON + dot.x;
                let ry = (lx * sr + ly * cr) * RAYON + dot.y;
                let pt = point(
                    px(cx + (rx as f32) * px_per_vb),
                    px(cy + (ry as f32) * px_per_vb),
                );
                if idx == 0 {
                    b.move_to(pt);
                } else {
                    b.line_to(pt);
                }
            }
            b.close();
            if let Ok(path) = b.build() {
                window.paint_path(path, fill.opacity(op));
            }
        } else {
            let mut b = PathBuilder::fill();
            emit_vb_circle(&mut b, center, px_per_vb, dot.x, dot.y, dot.r);
            if let Ok(path) = b.build() {
                window.paint_path(path, fill.opacity(op));
            }
        }
    }
}

/// Paints a sampled flat `BloubBot` directly onto a GPUI `Window` within `bounds`.
pub fn paint_flat_bloub(
    bounds: Bounds<Pixels>,
    sample: &BloubFrameSample,
    ink_hex: &str,
    paper_hex: &str,
    outline: Option<(Hsla, f32)>,
    window: &mut Window,
) {
    let bw = f32::from(bounds.size.width);
    let bh = f32::from(bounds.size.height);
    let side = bw.min(bh);
    if !side.is_finite() || side <= 0.5 {
        return;
    }

    let center = point(
        px(f32::from(bounds.origin.x) + bw * 0.5),
        px(f32::from(bounds.origin.y) + bh * 0.5),
    );
    let px_per_vb = (side * 1.28) / (VIEW_BOX as f32);
    let ink_color: Hsla = rgb(parse_hex_u32(ink_hex, 0x0a0a0c)).into();
    let paper_color: Hsla = rgb(parse_hex_u32(paper_hex, 0xf9f9f9)).into();
    let body_op = sample.body_alpha.clamp(0.0, 1.0) as f32;

    paint_arc_layer(&sample.arcs, true, center, px_per_vb, window);

    if sample.dots_behind && !sample.dots.is_empty() {
        paint_dots_gpui(&sample.dots, ink_hex, paper_hex, center, px_per_vb, window);
    }

    if body_op > 0.001 {
        if let Some((outline_color, width_px)) = outline {
            if width_px > 0.0 {
                let mut ob = PathBuilder::stroke(px(width_px * 2.0));
                super::silhouettes::emit_silhouette_gpui(
                    &mut ob,
                    &sample.silhouette,
                    center,
                    px_per_vb,
                );
                if let Ok(path) = ob.build() {
                    window.paint_path(path, outline_color);
                }
            }
        }
        if let Some(paper_path) =
            build_notched_body_gpui_path(&sample.silhouette, sample.notif, center, px_per_vb)
        {
            window.paint_path(paper_path, paper_color.opacity(body_op));
        }
        if let Some(ink_path) =
            build_notched_body_gpui_path(&sample.silhouette, sample.notif, center, px_per_vb)
        {
            window.paint_path(ink_path, ink_color.opacity(body_op));
        }
        for eye in &sample.eyes {
            if eye.culled || eye.alpha <= 0.001 {
                continue;
            }
            let mut eb = PathBuilder::fill();
            emit_eye_capsule_gpui(&mut eb, eye, center, px_per_vb);
            if let Ok(epath) = eb.build() {
                let eye_op = (eye.alpha.clamp(0.0, 1.0) as f32) * body_op;
                window.paint_path(epath, paper_color.opacity(eye_op));
            }
        }
    }

    if !sample.dots_behind && !sample.dots.is_empty() {
        paint_dots_gpui(&sample.dots, ink_hex, paper_hex, center, px_per_vb, window);
    }

    if let Some(notif) = sample.notif {
        let notif_color: Hsla = rgb(parse_hex_u32(NOTIF_BLUE, 0x2496e8)).into();
        let mut nb = PathBuilder::fill();
        emit_vb_circle(&mut nb, center, px_per_vb, notif.x, notif.y, notif.r);
        if let Ok(npath) = nb.build() {
            window.paint_path(npath, notif_color);
        }
    }

    paint_arc_layer(&sample.arcs, false, center, px_per_vb, window);
}

/// Renders a stateless GPUI `Div` element drawing the flat vector `BloubFrameSample`.
pub fn render_flat_bloub_element(
    sample: &BloubFrameSample,
    ink_hex: &str,
    paper_hex: &str,
    size_px: f32,
) -> Div {
    let safe_size = if size_px.is_finite() && size_px > 0.0 {
        size_px
    } else {
        32.0
    };
    let sample_owned = sample.clone();
    let ink_owned = if ink_hex.trim().is_empty() {
        "#0a0a0c".to_string()
    } else {
        ink_hex.trim().to_string()
    };
    let paper_owned = if paper_hex.trim().is_empty() {
        DEFAULT_PAPER.to_string()
    } else {
        paper_hex.trim().to_string()
    };

    div()
        .relative()
        .w(px(safe_size))
        .h(px(safe_size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_flat_bloub(
                        bounds,
                        &sample_owned,
                        &ink_owned,
                        &paper_owned,
                        None,
                        window,
                    );
                },
            )
            .w(px(safe_size))
            .h(px(safe_size)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulberry32_and_decor_tables_match_upstream_bit_for_bit() {
        let sched = blink_schedule();
        assert_eq!(sched.len(), 317);
        assert!((sched[0] - 1.4).abs() < 1e-12);
        assert!((sched[1] - 5.217086).abs() < 1e-5);
        assert!((sched[2] - 9.687224).abs() < 1e-5);
        assert!((sched[3] - 9.927224).abs() < 1e-5);
        assert!((sched[3] - sched[2] - 0.24).abs() < 1e-12);

        assert!((RINGS[0].a - 1.374315593).abs() < 1e-8);
        assert!((RINGS[0].k - 0.274936611).abs() < 1e-8);
        assert!((RINGS[0].hue - 4.538150977).abs() < 1e-8);
        assert!((BURST[0].angle - 4.177982462).abs() < 1e-8);
        assert!((BURST[0].rho - 0.635673838).abs() < 1e-8);
        assert!((COMET_RIBBONS[0].phase - 0.005392308).abs() < 1e-8);
        assert!((COMET_RIBBONS[0].hue - 4.084076267).abs() < 1e-8);
    }

    #[test]
    fn mid_morph_continuity_pointer_filter_and_reduced_motion_hold() {
        let mut engine = BloubBotEngine::new(
            EngineShapeId::Triangle,
            BloubExpression::Neutre,
            BloubParametricState::Idle,
        );
        let frozen = engine.sample(2.5, true);
        assert_eq!(frozen.breath_sy, 1.0);
        assert!(!frozen.wander_active);
        assert!(frozen.zero_clipping);

        engine.set_pointer(PointerKind::Touch, 0.8, -0.5, true, 1.0);
        assert!(!engine.is_pointer_active());
        assert!(engine.sample(1.0, false).wander_active);

        engine.set_pointer(PointerKind::Mouse, 0.8, -0.5, true, 1.0);
        assert!(engine.is_pointer_active());
        assert!(!engine.sample(1.25, false).wander_active);
        engine.set_pointer(PointerKind::Mouse, 0.0, 0.0, false, 1.3);

        engine.set_state(BloubParametricState::Wide, 2.0, 0.0);
        let before = engine.sample(2.15, false);
        engine.set_state(BloubParametricState::Notify, 2.15, 0.0);
        let after = engine.sample(2.15, false);
        let max_jump = before
            .silhouette
            .radii
            .iter()
            .zip(after.silhouette.radii.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f64, f64::max);
        assert!(max_jump < 1e-9);

        let mut filter = SubPixelFilter::new();
        let first = filter.filter_eyes(after.eyes, 320.0);
        let second = filter.filter_eyes(after.eyes, 320.0);
        assert_eq!(first, second);

        for st in BloubParametricState::ALL {
            let st_engine = BloubBotEngine::new(EngineShapeId::Cercle, BloubExpression::Neutre, st);
            let s = st_engine.sample(st.canonical_pose_time(), false);
            let svg = render_flat_svg(&s, "#3b93f0", DEFAULT_PAPER);
            assert!(svg.contains(VIEW_BOX_STR));
            if st == BloubParametricState::Notify {
                assert!(svg.contains(NOTIF_BLUE));
            }
        }
    }
}
