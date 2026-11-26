//! 3D spherical gaze basis, horizon culling/fade, 16 rest expressions, eye capsule paths,
//! and `LazyLock` 12-angle x 8-step binary-search contour `decalage` (`BloubBot-RRilvzPQ.js:L142-L399, L750-L753, L1356-L1524, L1750-L1766`).

use std::f64::consts::TAU;
use std::sync::LazyLock;

use gpui::{point, px, Path, PathBuilder, Pixels, Point};
use openbot_contracts::{BloubExpression, BloubParametricState, EngineShapeId};
use serde::{Deserialize, Serialize};

use super::silhouettes::{
    engine_shape_index, engine_shape_radii, fmt_svg_num, project_silhouette_points,
    radius_at_angle, Silhouette, RAYON,
};

/// Default inter-ocular angular half-split in degrees (`me = 15.46`).
pub const EYE_SPLIT_DEFAULT: f64 = 15.46;
/// Default eye capsule width in unit scale (`he = 0.186`).
pub const EYE_W_DEFAULT: f64 = 0.186;
/// Default eye capsule height in unit scale (`ge = 0.412`).
pub const EYE_H_DEFAULT: f64 = 0.412;
/// Horizon cull threshold (`depth <= 0.02` culls the eye).
pub const HORIZON_CULL_DEPTH: f64 = 0.02;
/// Horizon alpha fade ramp (`depth / 0.12`, so `depth == 0.06` yields `0.5`).
pub const HORIZON_FADE_DEPTH: f64 = 0.12;
/// Closed eyelid vertical squash floor (`0.06`).
pub const BLINK_CLOSED_SCALE: f64 = 0.06;
/// Worst-case organic wander yaw bound in degrees (`dt = 7.1`).
pub const WANDER_YAW_BOUND: f64 = 7.1;
/// Worst-case organic wander pitch bound in degrees (`ft = 5.5`).
pub const WANDER_PITCH_BOUND: f64 = 5.5;

/// 3D spherical head gaze angles in degrees (`yaw`, `pitch`, `roll`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GazeAngles {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

impl Default for GazeAngles {
    fn default() -> Self {
        REST_GAZE
    }
}

/// Default rest gaze (`_e = { yaw: 28.49, pitch: 28.62, roll: -13.0 }`).
pub const REST_GAZE: GazeAngles = GazeAngles {
    yaw: 28.49,
    pitch: 28.62,
    roll: -13.0,
};

/// Orthonormal tangent-plane basis and Z depth of a projected eye on the unit sphere (`N`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectedEyeBasis {
    pub x: f64,
    pub y: f64,
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub depth: f64,
}

/// Single eye rest geometry (`w`, `h` in unit scale, `tilt` in degrees, `open` in `0..=1`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EyeSpec {
    pub w: f64,
    pub h: f64,
    pub tilt: f64,
    pub open: f64,
}

impl EyeSpec {
    #[inline]
    pub const fn new(w: f64, h: f64, tilt: f64, open: f64) -> Self {
        Self { w, h, tilt, open }
    }
}

/// Complete expression definition (`Se` entry in `BloubBot-RRilvzPQ.js:L219-L381`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpressionDef {
    pub id: BloubExpression,
    pub gaze: GazeAngles,
    pub split: f64,
    pub eyes: [EyeSpec; 2],
}

/// Precomputed `(x, y)` face shift in unit scale keeping eyes inside non-circular silhouettes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DecalageOffset {
    pub x: f64,
    pub y: f64,
}

/// Final rendered 2D affine eye capsule state in viewBox coordinates (`[-158, 158]`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ComposedEyeTransform {
    pub w: f64,
    pub h: f64,
    pub open: f64,
    pub rest_open: f64,
    pub open_squash: f64,
    pub alpha: f64,
    pub culled: bool,
    pub depth: f64,
    pub matrix: [f64; 6],
}

const fn sym_eyes(w: f64, h: f64, tilt: f64, open: f64) -> [EyeSpec; 2] {
    [
        EyeSpec::new(w, h, tilt, open),
        EyeSpec::new(w, h, -tilt, open),
    ]
}

/// Canonical 16 `EXPRESSIONS` table (`BloubBot-RRilvzPQ.js:L219-L381`).
pub const EXPRESSIONS: [ExpressionDef; 16] = [
    ExpressionDef {
        id: BloubExpression::Neutre,
        gaze: REST_GAZE,
        split: 15.46,
        eyes: sym_eyes(0.186, 0.412, 0.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Attentif,
        gaze: GazeAngles {
            yaw: 4.0,
            pitch: 5.0,
            roll: -4.0,
        },
        split: 16.0,
        eyes: sym_eyes(0.21, 0.44, 0.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Surpris,
        gaze: GazeAngles {
            yaw: 3.0,
            pitch: -3.0,
            roll: 0.0,
        },
        split: 19.0,
        eyes: sym_eyes(0.45, 0.47, 0.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Excite,
        gaze: GazeAngles {
            yaw: 6.0,
            pitch: -14.0,
            roll: 0.0,
        },
        split: 19.5,
        eyes: sym_eyes(0.4, 0.56, -10.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Heureux,
        gaze: GazeAngles {
            yaw: 5.0,
            pitch: 9.0,
            roll: 0.0,
        },
        split: 17.0,
        eyes: sym_eyes(0.27, 0.17, 14.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Hilare,
        gaze: GazeAngles {
            yaw: 4.0,
            pitch: 14.0,
            roll: 0.0,
        },
        split: 18.0,
        eyes: sym_eyes(0.34, 0.13, 20.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Colere,
        gaze: GazeAngles {
            yaw: 3.0,
            pitch: 7.0,
            roll: 0.0,
        },
        split: 17.0,
        eyes: sym_eyes(0.34, 0.15, 30.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Triste,
        gaze: GazeAngles {
            yaw: 3.0,
            pitch: -13.0,
            roll: 0.0,
        },
        split: 16.0,
        eyes: sym_eyes(0.22, 0.4, -28.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Effraye,
        gaze: GazeAngles {
            yaw: 2.0,
            pitch: -20.0,
            roll: 0.0,
        },
        split: 20.5,
        eyes: sym_eyes(0.4, 0.6, 0.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Mefiant,
        gaze: GazeAngles {
            yaw: 12.0,
            pitch: 6.0,
            roll: -6.0,
        },
        split: 16.0,
        eyes: [
            EyeSpec::new(0.21, 0.4, 0.0, 1.0),
            EyeSpec::new(0.22, 0.15, 0.0, 1.0),
        ],
    },
    ExpressionDef {
        id: BloubExpression::Confus,
        gaze: GazeAngles {
            yaw: -14.0,
            pitch: 3.0,
            roll: 8.0,
        },
        split: 16.5,
        eyes: [
            EyeSpec::new(0.2, 0.44, -18.0, 1.0),
            EyeSpec::new(0.28, 0.17, 14.0, 1.0),
        ],
    },
    ExpressionDef {
        id: BloubExpression::Curieux,
        gaze: GazeAngles {
            yaw: 16.0,
            pitch: -9.0,
            roll: -15.0,
        },
        split: 16.5,
        eyes: [
            EyeSpec::new(0.24, 0.46, -8.0, 1.0),
            EyeSpec::new(0.2, 0.38, -8.0, 1.0),
        ],
    },
    ExpressionDef {
        id: BloubExpression::Fier,
        gaze: GazeAngles {
            yaw: 5.0,
            pitch: 17.0,
            roll: 0.0,
        },
        split: 17.0,
        eyes: sym_eyes(0.3, 0.15, 18.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Timide,
        gaze: GazeAngles {
            yaw: -19.0,
            pitch: -14.0,
            roll: -7.0,
        },
        split: 14.0,
        eyes: sym_eyes(0.17, 0.3, 0.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Blase,
        gaze: GazeAngles {
            yaw: -22.0,
            pitch: 2.0,
            roll: 0.0,
        },
        split: 16.0,
        eyes: sym_eyes(0.3, 0.12, 0.0, 1.0),
    },
    ExpressionDef {
        id: BloubExpression::Somnolent,
        gaze: GazeAngles {
            yaw: 6.0,
            pitch: -9.0,
            roll: -3.0,
        },
        split: 16.0,
        eyes: sym_eyes(0.2, 0.42, 0.0, 0.42),
    },
];

#[inline]
pub(crate) const fn expression_index(expr: BloubExpression) -> usize {
    match expr {
        BloubExpression::Neutre => 0,
        BloubExpression::Attentif => 1,
        BloubExpression::Surpris => 2,
        BloubExpression::Excite => 3,
        BloubExpression::Heureux => 4,
        BloubExpression::Hilare => 5,
        BloubExpression::Colere => 6,
        BloubExpression::Triste => 7,
        BloubExpression::Effraye => 8,
        BloubExpression::Mefiant => 9,
        BloubExpression::Confus => 10,
        BloubExpression::Curieux => 11,
        BloubExpression::Fier => 12,
        BloubExpression::Timide => 13,
        BloubExpression::Blase => 14,
        BloubExpression::Somnolent => 15,
    }
}

/// Looks up the canonical `ExpressionDef` for `expr`.
#[inline]
pub fn expression_def(expr: BloubExpression) -> ExpressionDef {
    EXPRESSIONS[expression_index(expr)]
}

/// Linearly blends two expressions (`Ee(a, b, t)`), taking `id = b.id`.
pub fn blend_expression(a: &ExpressionDef, b: &ExpressionDef, t: f64) -> ExpressionDef {
    let lerp = |u: f64, v: f64| u + (v - u) * t;
    let blend_eye = |ea: EyeSpec, eb: EyeSpec| EyeSpec {
        w: lerp(ea.w, eb.w),
        h: lerp(ea.h, eb.h),
        tilt: lerp(ea.tilt, eb.tilt),
        open: lerp(ea.open, eb.open),
    };
    ExpressionDef {
        id: b.id,
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
    }
}

#[inline]
fn rotate_basis_pair(u: [f64; 3], v: [f64; 3], rad: f64) -> ([f64; 3], [f64; 3]) {
    let (s, c) = rad.sin_cos();
    (
        [
            u[0] * c + v[0] * s,
            u[1] * c + v[1] * s,
            u[2] * c + v[2] * s,
        ],
        [
            v[0] * c - u[0] * s,
            v[1] * c - u[1] * s,
            v[2] * c - u[2] * s,
        ],
    )
}

/// Exact 3D spherical eye projection (`N(gaze, scale, split)`): `Yaw -> Pitch -> Roll -> ±split`.
pub fn project_eyes_3d(gaze: GazeAngles, scale: f64, split: f64) -> [ProjectedEyeBasis; 2] {
    let mut r = [0.0, 0.0, 1.0];
    let mut i = [1.0, 0.0, 0.0];
    let mut a = [0.0, 1.0, 0.0];

    (r, i) = rotate_basis_pair(r, i, gaze.yaw.to_radians());
    (a, r) = rotate_basis_pair(a, r, gaze.pitch.to_radians());
    (i, a) = rotate_basis_pair(i, a, gaze.roll.to_radians());

    let project_side = |side: f64| -> ProjectedEyeBasis {
        let (o, s) = rotate_basis_pair(r, i, (split * side).to_radians());
        ProjectedEyeBasis {
            x: o[0] * scale,
            y: o[1] * scale,
            a: s[0],
            b: s[1],
            c: a[0],
            d: a[1],
            depth: o[2],
        }
    };

    [project_side(-1.0), project_side(1.0)]
}

/// Vertical eyelid squash curve (`I(open) = 0.06 + 0.94 * clamp01(open)`).
#[inline]
pub fn blink_scale(open: f64) -> f64 {
    BLINK_CLOSED_SCALE + (1.0 - BLINK_CLOSED_SCALE) * open.clamp(0.0, 1.0)
}

/// Evaluates horizon culling (`depth <= 0.02`) and smooth alpha fade (`depth / 0.12`).
#[inline]
pub fn horizon_eye_visibility(depth: f64, eye_alpha: f64) -> (bool, f64) {
    if eye_alpha <= 0.01 || depth <= HORIZON_CULL_DEPTH {
        (true, 0.0)
    } else {
        (
            false,
            eye_alpha * (depth / HORIZON_FADE_DEPTH).clamp(0.0, 1.0),
        )
    }
}

/// Composes final 2D affine eye capsule transforms (`BloubBot-RRilvzPQ.js:L1750-L1766`).
#[allow(clippy::too_many_arguments)]
pub fn compose_eye_transforms(
    sil: &Silhouette,
    gaze: GazeAngles,
    split: f64,
    eyes: &[EyeSpec; 2],
    eye_alpha: f64,
    effective_lid: f64,
    off_x_total: f64,
    off_y_total: f64,
    decalage: DecalageOffset,
    scale: f64,
    eye_scale: f64,
) -> [ComposedEyeTransform; 2] {
    let basis = project_eyes_3d(gaze, scale, split);
    let safe_eye_scale = if eye_scale.is_finite() && eye_scale > 0.0 {
        eye_scale
    } else {
        1.0
    };

    std::array::from_fn(|idx| {
        let r = basis[idx];
        let spec = eyes[idx];
        let rest_open = blink_scale(spec.open);
        let open_squash = blink_scale(effective_lid.min(spec.open));
        let (culled, alpha) = horizon_eye_visibility(r.depth, eye_alpha);

        let radial = radius_at_angle(&sil.radii, r.y.atan2(r.x) - sil.rot);
        let (ls, lc) = spec.tilt.to_radians().sin_cos();
        let m_a = r.a * lc + r.c * ls;
        let m_b = (r.b * lc + r.d * ls) * open_squash;
        let m_c = -r.a * ls + r.c * lc;
        let m_d = (-r.b * ls + r.d * lc) * open_squash;
        let tx = r.x * radial + (off_x_total + decalage.x) * scale;
        let ty = r.y * radial + (off_y_total + decalage.y) * scale;

        ComposedEyeTransform {
            w: (spec.w * safe_eye_scale * scale).max(0.01),
            h: (spec.h * safe_eye_scale * scale).max(0.01),
            open: spec.open,
            rest_open,
            open_squash,
            alpha,
            culled,
            depth: r.depth,
            matrix: [m_a, m_b, m_c, m_d, tx, ty],
        }
    })
}

/// Local eye capsule SVG path `Ie(W, H)` where `W` and `H` are in viewBox units (`eye.w`, `eye.h`).
pub fn eye_capsule_svg_path(w: f64, h: f64) -> String {
    let hw = w.max(0.01) * 0.5;
    let hh = h.max(0.01) * 0.5;
    let cr = hw.min(hh);
    let t = fmt_svg_num;
    format!(
        "M{} {}A{} {} 0 0 1 {} {}L{} {}A{} {} 0 0 1 {} {}L{} {}A{} {} 0 0 1 {} {}L{} {}A{} {} 0 0 1 {} {}Z",
        t(-hw),
        t(-hh + cr),
        t(cr),
        t(cr),
        t(-hw + cr),
        t(-hh),
        t(hw - cr),
        t(-hh),
        t(cr),
        t(cr),
        t(hw),
        t(-hh + cr),
        t(hw),
        t(hh - cr),
        t(cr),
        t(cr),
        t(hw - cr),
        t(hh),
        t(-hw + cr),
        t(hh),
        t(cr),
        t(cr),
        t(-hw),
        t(hh - cr),
    )
}

/// Formats a 6-element affine matrix as SVG `matrix(a,b,c,d,tx,ty)`.
pub fn eye_matrix_svg_attr(matrix: &[f64; 6]) -> String {
    let t = fmt_svg_num;
    format!(
        "matrix({},{},{},{},{},{})",
        t(matrix[0]),
        t(matrix[1]),
        t(matrix[2]),
        t(matrix[3]),
        t(matrix[4]),
        t(matrix[5]),
    )
}

/// Emits an affine-transformed eye capsule into a GPUI `PathBuilder`.
/// Note: GPUI `0.2.2` `PathBuilder::cubic_bezier_to(to, ctrl1, ctrl2)` takes `to` FIRST.
pub fn emit_eye_capsule_gpui(
    builder: &mut PathBuilder,
    eye: &ComposedEyeTransform,
    center: Point<Pixels>,
    px_per_vb: f32,
) {
    let hw = eye.w.max(0.01) * 0.5;
    let hh = eye.h.max(0.01) * 0.5;
    let cr = hw.min(hh);
    let k = 0.55228475 * cr;
    let m = eye.matrix;
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let tf = |lx: f64, ly: f64| -> Point<Pixels> {
        let vx = m[0] * lx + m[2] * ly + m[4];
        let vy = m[1] * lx + m[3] * ly + m[5];
        point(
            px(cx + (vx as f32) * px_per_vb),
            px(cy + (vy as f32) * px_per_vb),
        )
    };

    builder.move_to(tf(-hw, -hh + cr));
    builder.cubic_bezier_to(
        tf(-hw + cr, -hh),
        tf(-hw, -hh + cr - k),
        tf(-hw + cr - k, -hh),
    );
    if hw - cr > 1e-6 {
        builder.line_to(tf(hw - cr, -hh));
    }
    builder.cubic_bezier_to(tf(hw, -hh + cr), tf(hw - cr + k, -hh), tf(hw, -hh + cr - k));
    if hh - cr > 1e-6 {
        builder.line_to(tf(hw, hh - cr));
    }
    builder.cubic_bezier_to(tf(hw - cr, hh), tf(hw, hh - cr + k), tf(hw - cr + k, hh));
    if hw - cr > 1e-6 {
        builder.line_to(tf(-hw + cr, hh));
    }
    builder.cubic_bezier_to(tf(-hw, hh - cr), tf(-hw + cr - k, hh), tf(-hw, hh - cr + k));
    builder.close();
}

/// Builds a filled GPUI `Path<Pixels>` for an affine-transformed eye capsule.
pub fn eye_capsule_gpui_path(
    eye: &ComposedEyeTransform,
    center: Point<Pixels>,
    px_per_vb: f32,
) -> Option<Path<Pixels>> {
    if eye.culled || eye.alpha <= 0.001 || !px_per_vb.is_finite() || px_per_vb <= 0.0 {
        return None;
    }
    let mut builder = PathBuilder::fill();
    emit_eye_capsule_gpui(&mut builder, eye, center, px_per_vb);
    builder.build().ok()
}

#[derive(Clone, Copy)]
struct EyeFootprint {
    x: f64,
    y: f64,
    ax: f64,
    ay: f64,
    r: f64,
    m: [f64; 4],
}

struct FitCase {
    empreintes: Vec<EyeFootprint>,
    reference: Vec<EyeFootprint>,
    contour: [(f64, f64); 64],
    cal_contour: [(f64, f64); 64],
}

#[inline]
fn point_in_polygon(px: f64, py: f64, poly: &[(f64, f64); 64]) -> bool {
    let mut inside = false;
    let mut j = 63usize;
    for i in 0..64 {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn compute_footprints(
    gaze: GazeAngles,
    split: f64,
    eyes: &[EyeSpec; 2],
    sil_rot: f64,
    radii: &[f64; 64],
) -> Vec<EyeFootprint> {
    let projected = project_eyes_3d(gaze, RAYON, split);
    let mut out = Vec::with_capacity(2);
    for k in 0..2 {
        let o = projected[k];
        if o.depth <= HORIZON_CULL_DEPTH {
            continue;
        }
        let spec = eyes[k];
        let (ls, lc) = spec.tilt.to_radians().sin_cos();
        let d = o.a * lc + o.c * ls;
        let f = o.b * lc + o.d * ls;
        let p = -o.a * ls + o.c * lc;
        let m = -o.b * ls + o.d * lc;
        let hw = (spec.w * RAYON).max(0.01) * 0.5;
        let hh = (spec.h * RAYON).max(0.01) * 0.5;
        let cr = hw.min(hh);
        let tall = hh > hw;
        let seg = if tall { hh - cr } else { hw - cr };
        let b = radius_at_angle(radii, o.y.atan2(o.x) - sil_rot);
        out.push(EyeFootprint {
            x: o.x * b,
            y: o.y * b,
            ax: (if tall { p } else { d }) * seg,
            ay: (if tall { m } else { f }) * seg,
            r: cr,
            m: [d, f, p, m],
        });
    }
    out
}

fn contour_margin(
    contour: &[(f64, f64); 64],
    footprints: &[EyeFootprint],
    dx: f64,
    dy: f64,
    drift_pad: f64,
) -> f64 {
    if footprints.is_empty() {
        return 1e9;
    }
    let mut best_margin = f64::INFINITY;
    for s in footprints {
        let tx = s.x + dx;
        let ty = s.y + dy;
        let x1 = tx - s.ax;
        let y1 = ty - s.ay;
        let x2 = tx + s.ax;
        let y2 = ty + s.ay;
        let sdx = x2 - x1;
        let sdy = y2 - y1;
        let l2 = sdx * sdx + sdy * sdy;
        let in_poly = point_in_polygon(tx, ty, contour)
            && point_in_polygon(x1, y1, contour)
            && point_in_polygon(x2, y2, contour);

        let mut min_d = f64::INFINITY;
        let mut best_u = (1.0, 0.0);
        for &(vx, vy) in contour {
            let t = if l2 > 1e-12 {
                (((vx - x1) * sdx + (vy - y1) * sdy) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let cx = x1 + t * sdx;
            let cy = y1 + t * sdy;
            let ddx = cx - vx;
            let ddy = cy - vy;
            let dist = ddx.hypot(ddy);
            if dist < min_d {
                min_d = dist;
                best_u = if dist > 1e-9 {
                    (ddx / dist, ddy / dist)
                } else {
                    (1.0, 0.0)
                };
            }
        }
        let (ux, uy) = best_u;
        let [u, d_m, f, p] = s.m;
        let m_cap = s.r * (u * ux + d_m * uy).hypot(f * ux + p * uy) + drift_pad;
        let signed_d = if in_poly { min_d } else { -min_d };
        best_margin = best_margin.min(signed_d - m_cap);
    }
    best_margin
}

fn solve_decalage_cases(cases: &[FitCase]) -> DecalageOffset {
    if cases.is_empty() {
        return DecalageOffset::default();
    }
    let drift_pad = 0.006_f64.hypot(0.007_f64) * RAYON;
    let marge_at = |dx: f64, dy: f64| -> f64 {
        cases
            .iter()
            .map(|c| contour_margin(&c.contour, &c.empreintes, dx, dy, drift_pad))
            .fold(f64::INFINITY, f64::min)
    };
    let mut ref_min = cases
        .iter()
        .map(|c| contour_margin(&c.cal_contour, &c.reference, 0.0, 0.0, drift_pad))
        .fold(f64::INFINITY, f64::min);

    let emp0 = &cases[0].empreintes;
    let (rx, ry) = if emp0.is_empty() {
        (0.0, 0.0)
    } else {
        let n = emp0.len() as f64;
        (
            -emp0.iter().map(|e| e.x).sum::<f64>() / n,
            -emp0.iter().map(|e| e.y).sum::<f64>() / n,
        )
    };
    let search_r = 35.0_f64.max(rx.hypot(ry) * 1.25);
    ref_min = ref_min.min(marge_at(rx, ry));

    let s0 = marge_at(0.0, 0.0);
    if s0 >= ref_min && s0 >= 0.0 {
        return DecalageOffset { x: 0.0, y: 0.0 };
    }

    let target = ref_min.max(0.0);
    let mut best_dist = f64::INFINITY;
    let mut lx = 0.0;
    let mut ly = 0.0;
    let mut best_m = s0;
    let mut fx = 0.0;
    let mut fy = 0.0;

    for k in 0..12 {
        let theta = (TAU * (k as f64)) / 12.0;
        let (sr, cr) = theta.sin_cos();
        if marge_at(cr * search_r, sr * search_r) < target {
            for frac in [0.3, 0.6, 1.0] {
                let m = marge_at(cr * search_r * frac, sr * search_r * frac);
                if m > best_m {
                    best_m = m;
                    fx = cr * search_r * frac;
                    fy = sr * search_r * frac;
                }
            }
            continue;
        }
        let mut lo = 0.0;
        let mut hi = search_r;
        for _ in 0..8 {
            let mid = 0.5 * (lo + hi);
            if marge_at(cr * mid, sr * mid) >= target {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        if hi < best_dist {
            best_dist = hi;
            lx = cr * hi;
            ly = sr * hi;
        }
    }

    let (hx, hy) = if best_dist.is_infinite() {
        (fx, fy)
    } else {
        (lx, ly)
    };
    let quant = |v: f64| -> f64 {
        let q = ((v / RAYON) * 1e6).round() / 1e6;
        if q == 0.0 {
            0.0
        } else {
            q
        }
    };
    DecalageOffset {
        x: quant(hx),
        y: quant(hy),
    }
}

fn build_face_cases(
    shape_radii: &[f64; 64],
    gaze: GazeAngles,
    split: f64,
    eyes: &[EyeSpec; 2],
) -> Vec<FitCase> {
    let base_sil = Silhouette::from_radii(*shape_radii);
    let cal_sil = Silhouette::circle(1.0);
    let contour = project_silhouette_points(&base_sil, RAYON);
    let cal_contour = project_silhouette_points(&cal_sil, RAYON);
    let cal_radii = [1.0; 64];

    let mut cases = Vec::with_capacity(4);
    for dyaw in [-WANDER_YAW_BOUND, WANDER_YAW_BOUND] {
        for dpitch in [-WANDER_PITCH_BOUND, WANDER_PITCH_BOUND] {
            let corner_gaze = GazeAngles {
                yaw: gaze.yaw + dyaw,
                pitch: gaze.pitch + dpitch,
                roll: gaze.roll,
            };
            cases.push(FitCase {
                empreintes: compute_footprints(corner_gaze, split, eyes, 0.0, shape_radii),
                reference: compute_footprints(corner_gaze, split, eyes, 0.0, &cal_radii),
                contour,
                cal_contour,
            });
        }
    }
    cases
}

fn base_state_face(
    state: BloubParametricState,
    expr: Option<BloubExpression>,
) -> Option<(GazeAngles, f64, [EyeSpec; 2])> {
    match state {
        BloubParametricState::Idle | BloubParametricState::Swirl => {
            if let Some(ex) = expr {
                let def = expression_def(ex);
                Some((def.gaze, def.split, def.eyes))
            } else {
                Some((
                    REST_GAZE,
                    EYE_SPLIT_DEFAULT,
                    sym_eyes(EYE_W_DEFAULT, EYE_H_DEFAULT, 0.0, 1.0),
                ))
            }
        }
        BloubParametricState::Wink => Some((
            GazeAngles {
                yaw: -5.37,
                pitch: 4.55,
                roll: 6.7,
            },
            16.25,
            [
                EyeSpec::new(0.236, 0.464, 0.0, 1.0),
                EyeSpec::new(0.447, 0.089, 0.0, 1.0),
            ],
        )),
        BloubParametricState::Wide => Some((
            GazeAngles {
                yaw: 6.92,
                pitch: -21.96,
                roll: 11.6,
            },
            18.43,
            sym_eyes(0.356, 0.875, 0.0, 1.0),
        )),
        BloubParametricState::Notify => Some((
            GazeAngles {
                yaw: -21.94,
                pitch: -5.82,
                roll: -12.2,
            },
            18.89,
            sym_eyes(0.505, 0.498, 0.0, 1.0),
        )),
        _ => None,
    }
}

struct DecalageTable {
    idle_swirl: [[DecalageOffset; 17]; 14],
    wink: [DecalageOffset; 14],
    wide: [DecalageOffset; 14],
    notify: [DecalageOffset; 14],
}

static DECALAGE_TABLE: LazyLock<DecalageTable> = LazyLock::new(|| {
    let mut idle_swirl = [[DecalageOffset::default(); 17]; 14];
    let mut wink = [DecalageOffset::default(); 14];
    let mut wide = [DecalageOffset::default(); 14];
    let mut notify = [DecalageOffset::default(); 14];

    for (s_idx, shape) in EngineShapeId::ALL.iter().copied().enumerate() {
        let radii = engine_shape_radii(shape);
        if let Some((gz, sp, ey)) = base_state_face(BloubParametricState::Idle, None) {
            idle_swirl[s_idx][0] = solve_decalage_cases(&build_face_cases(&radii, gz, sp, &ey));
        }
        for (e_idx, expr) in BloubExpression::ALL.iter().copied().enumerate() {
            let def = expression_def(expr);
            idle_swirl[s_idx][e_idx + 1] =
                solve_decalage_cases(&build_face_cases(&radii, def.gaze, def.split, &def.eyes));
        }
        if let Some((gz, sp, ey)) = base_state_face(BloubParametricState::Wink, None) {
            wink[s_idx] = solve_decalage_cases(&build_face_cases(&radii, gz, sp, &ey));
        }
        if let Some((gz, sp, ey)) = base_state_face(BloubParametricState::Wide, None) {
            wide[s_idx] = solve_decalage_cases(&build_face_cases(&radii, gz, sp, &ey));
        }
        if let Some((gz, sp, ey)) = base_state_face(BloubParametricState::Notify, None) {
            notify[s_idx] = solve_decalage_cases(&build_face_cases(&radii, gz, sp, &ey));
        }
    }

    DecalageTable {
        idle_swirl,
        wink,
        wide,
        notify,
    }
});

/// Precomputed O(1) `decalage` lookup (`BloubBot-RRilvzPQ.js:L1503-L1524`).
pub fn decalage_for(
    shape: EngineShapeId,
    state: BloubParametricState,
    expr: Option<BloubExpression>,
) -> DecalageOffset {
    let table = &*DECALAGE_TABLE;
    let s_idx = engine_shape_index(shape);
    match state {
        BloubParametricState::Idle | BloubParametricState::Swirl => {
            let e_slot = expr.map_or(0, |ex| expression_index(ex) + 1);
            table.idle_swirl[s_idx][e_slot]
        }
        BloubParametricState::Wink => table.wink[s_idx],
        BloubParametricState::Wide => table.wide[s_idx],
        BloubParametricState::Notify => table.notify[s_idx],
        _ => DecalageOffset::default(),
    }
}

/// Computes `decalage` for an uncatalogued custom face (`PlushFeltRenderer` `ASLEEP_FACE`/`FAILED_FACE`).
pub fn decalage_for_custom_face(
    shape: EngineShapeId,
    state: BloubParametricState,
    face: &ExpressionDef,
) -> DecalageOffset {
    match state {
        BloubParametricState::Idle | BloubParametricState::Swirl => {
            let radii = engine_shape_radii(shape);
            solve_decalage_cases(&build_face_cases(&radii, face.gaze, face.split, &face.eyes))
        }
        _ => decalage_for(shape, state, None),
    }
}

fn local_capsule_perimeter_points(w: f64, h: f64) -> [(f64, f64); 24] {
    let hw = w.max(0.01) * 0.5;
    let hh = h.max(0.01) * 0.5;
    let cr = hw.min(hh);
    let sx = (hw - cr).max(0.0);
    let sy = (hh - cr).max(0.0);
    let mut pts = [(0.0, 0.0); 24];
    for (idx, slot) in pts.iter_mut().enumerate() {
        let quad = idx / 6;
        let step = (idx % 6) as f64 / 5.0;
        let base = (quad as f64) * std::f64::consts::FRAC_PI_2;
        let ang = base + step * std::f64::consts::FRAC_PI_2;
        let cx = if ang.cos() >= 0.0 { sx } else { -sx };
        let cy = if ang.sin() >= 0.0 { sy } else { -sy };
        *slot = (cx + cr * ang.cos(), cy + cr * ang.sin());
    }
    pts
}

fn point_in_dense_poly(px: f64, py: f64, poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) && px < ((xj - xi) * (py - yi)) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Verifies that every visible eye capsule lies strictly inside the rendered Catmull-Rom silhouette.
pub fn verify_eyes_zero_clipping(sil: &Silhouette, eyes: &[ComposedEyeTransform; 2]) -> bool {
    if eyes.iter().all(|e| e.culled || e.alpha <= 0.01) {
        return true;
    }
    let anchors = project_silhouette_points(sil, RAYON);
    let mut dense = Vec::with_capacity(256);
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
            let x =
                u * u * u * o.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * s.0;
            let y =
                u * u * u * o.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * s.1;
            dense.push((x, y));
        }
    }

    for eye in eyes {
        if eye.culled || eye.alpha <= 0.01 {
            continue;
        }
        let m = eye.matrix;
        let samples = local_capsule_perimeter_points(eye.w, eye.h);
        for (lx, ly) in samples {
            let vx = m[0] * lx + m[2] * ly + m[4];
            let vy = m[1] * lx + m[3] * ly + m[5];
            if !point_in_dense_poly(vx, vy, &dense) {
                let near_edge = dense
                    .iter()
                    .any(|&(bx, by)| (vx - bx).hypot(vy - by) <= 0.75);
                if !near_edge {
                    return false;
                }
            }
        }
    }
    true
}

/// Refines `eyes` via minimum inward centroid translation whenever extreme live pointer gaze
/// exceeds the prebaked rest `decalage` envelope on narrow silhouettes (`triangle`, `losange`).
pub fn fit_eyes_zero_clipping(sil: &Silhouette, eyes: &mut [ComposedEyeTransform; 2]) -> bool {
    if verify_eyes_zero_clipping(sil, eyes) {
        return true;
    }
    let anchors = project_silhouette_points(sil, RAYON);
    let mut cx_body = 0.0_f64;
    let mut cy_body = 0.0_f64;
    for &(ax, ay) in &anchors {
        cx_body += ax;
        cy_body += ay;
    }
    cx_body /= anchors.len() as f64;
    cy_body /= anchors.len() as f64;

    let mut vis_count = 0.0_f64;
    let mut ex_mean = 0.0_f64;
    let mut ey_mean = 0.0_f64;
    for eye in eyes.iter() {
        if !eye.culled && eye.alpha > 0.01 {
            ex_mean += eye.matrix[4];
            ey_mean += eye.matrix[5];
            vis_count += 1.0;
        }
    }
    if vis_count <= 0.0 {
        return true;
    }
    ex_mean /= vis_count;
    ey_mean /= vis_count;

    let pull_x = cx_body - ex_mean;
    let pull_y = cy_body - ey_mean;
    let base_eyes = *eyes;
    let shifted = |t: f64| -> [ComposedEyeTransform; 2] {
        let mut cand = base_eyes;
        for slot in &mut cand {
            slot.matrix[4] += pull_x * t;
            slot.matrix[5] += pull_y * t;
        }
        cand
    };

    let mut best: Option<(f64, [ComposedEyeTransform; 2])> = None;
    for k in 0..13 {
        let (dx, dy) = if k == 0 {
            (pull_x, pull_y)
        } else {
            let theta = (TAU * ((k - 1) as f64)) / 12.0;
            let mag = pull_x.hypot(pull_y).max(55.0);
            (theta.cos() * mag, theta.sin() * mag)
        };
        let eval_dir = |u: f64| -> [ComposedEyeTransform; 2] {
            let mut cand = base_eyes;
            for slot in &mut cand {
                slot.matrix[4] += dx * u;
                slot.matrix[5] += dy * u;
            }
            cand
        };
        if !verify_eyes_zero_clipping(sil, &eval_dir(1.0)) {
            continue;
        }
        let mut lo = 0.0_f64;
        let mut hi = 1.0_f64;
        for _ in 0..14 {
            let mid = 0.5 * (lo + hi);
            if verify_eyes_zero_clipping(sil, &eval_dir(mid)) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let dist = (dx * hi).hypot(dy * hi);
        let fitted = eval_dir(hi);
        if best.as_ref().is_none_or(|(b_dist, _)| dist < *b_dist) {
            best = Some((dist, fitted));
        }
    }

    if let Some((_, fitted)) = best {
        *eyes = fitted;
        true
    } else {
        let fallback = shifted(1.0);
        let ok = verify_eyes_zero_clipping(sil, &fallback);
        if ok {
            *eyes = fallback;
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizon_cull_and_fade_match_upstream_thresholds() {
        let (culled_002, alpha_002) = horizon_eye_visibility(0.02, 1.0);
        assert!(culled_002);
        assert_eq!(alpha_002, 0.0);

        let (culled_006, alpha_006) = horizon_eye_visibility(0.06, 1.0);
        assert!(!culled_006);
        assert!((alpha_006 - 0.5).abs() < 1e-12);

        assert!((blink_scale(0.0) - 0.06).abs() < 1e-12);
        assert!((blink_scale(1.0) - 1.0).abs() < 1e-12);
        assert!((blink_scale(0.42) - 0.4548).abs() < 1e-12);
    }

    #[test]
    fn decalage_keeps_all_shapes_and_expressions_inside_silhouette() {
        assert_eq!(
            decalage_for(
                EngineShapeId::Cercle,
                BloubParametricState::Idle,
                Some(BloubExpression::Neutre)
            ),
            DecalageOffset { x: 0.0, y: 0.0 }
        );
        for shape in EngineShapeId::ALL {
            let sil = Silhouette::from_radii(engine_shape_radii(shape));
            for expr in BloubExpression::ALL {
                let def = expression_def(expr);
                let dec = decalage_for(shape, BloubParametricState::Idle, Some(expr));
                let eyes = compose_eye_transforms(
                    &sil, def.gaze, def.split, &def.eyes, 1.0, 1.0, 0.0, 0.0, dec, RAYON, 1.0,
                );
                assert!(
                    verify_eyes_zero_clipping(&sil, &eyes),
                    "eye clipped for shape={shape:?} expr={expr:?} dec={dec:?}"
                );
            }
        }
    }
}
