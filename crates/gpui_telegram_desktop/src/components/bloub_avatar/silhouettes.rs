//! 1:1 64-ray polar silhouettes, measured profiles, and closed Catmull-Rom Bezier paths
//! for `@norbert_bodziony/bloub@0.7.0` (`BloubBot-RRilvzPQ.js:L21, L401-L893, L982-L1005`).

use std::f64::consts::{PI, TAU};
use std::sync::LazyLock;

use gpui::{point, px, Path, PathBuilder, Pixels, Point};
use openbot_contracts::{BloubShape, EngineShapeId};

/// Canonical radial unit scale (`scale = 100.0`).
pub const RAYON: f64 = 100.0;
/// Half viewBox extent (`158.0`, spanning `-158..158`).
pub const DEMI_VIEWBOX: f64 = 158.0;
/// Full viewBox extent (`316.0`).
pub const VIEW_BOX: f64 = 316.0;
/// Canonical SVG `viewBox` attribute string.
pub const VIEW_BOX_STR: &str = "-158 -158 316 316";
/// Default eye/backing paper hex color.
pub const DEFAULT_PAPER: &str = "#f9f9f9";
/// Canonical `notify` badge accent blue.
pub const NOTIF_BLUE: &str = "#2496e8";
/// Polar sample count around `[0, 2*PI)`.
pub const SILHOUETTE_RAYS: usize = 64;

/// Exclamation bar vertical center offset (`$e = -0.1875`).
pub const EXCLAIM_CY: f64 = -0.1875;
/// Play-state triangle centroid offset (`at = 0.213`).
pub const PLAY_TRIANGLE_OFFSET: f64 = 0.213;
/// Thinking-state three-dot X offsets (`O`).
pub const DOT_X: [f64; 3] = [-0.557, -0.013, 0.532];
/// Thinking-state dot base radius (`k = 0.165`).
pub const DOT_R: f64 = 0.165;
/// Thinking-state dot pulse peak factor (`ie = 1.25`).
pub const DOT_PEAK: f64 = 1.25;
/// Comet-state contracted dot radius (`ue = 0.129`).
pub const COMET_DOT: f64 = 0.129;

/// Verbatim 64-ray measured egg profile (`profiles.ts`).
pub const PROFILE_EGG: [f64; 64] = [
    0.8369, 0.8424, 0.8497, 0.8585, 0.8674, 0.8775, 0.8878, 0.8983, 0.9089, 0.9185, 0.9288, 0.9374,
    0.9445, 0.9504, 0.9543, 0.9559, 0.9555, 0.9519, 0.9466, 0.9389, 0.9302, 0.9193, 0.9085, 0.8969,
    0.8852, 0.8734, 0.8625, 0.8513, 0.8411, 0.8325, 0.8243, 0.8179, 0.8137, 0.8112, 0.8102, 0.8128,
    0.8178, 0.8262, 0.8374, 0.8518, 0.8702, 0.8922, 0.9169, 0.9446, 0.9741, 1.0023, 1.0267, 1.0433,
    1.0481, 1.0393, 1.0216, 0.997, 0.9697, 0.9418, 0.9169, 0.8949, 0.876, 0.8604, 0.849, 0.8394,
    0.8337, 0.8314, 0.8305, 0.8326,
];

/// Verbatim 64-ray measured hexagon state profile (`profiles.ts`).
pub const PROFILE_HEXAGON: [f64; 64] = [
    0.921, 0.9282, 0.9441, 0.9706, 0.9984, 1.0059, 0.9896, 0.9562, 0.929, 0.9124, 0.9047, 0.9058,
    0.9157, 0.9349, 0.9642, 0.9873, 0.9882, 0.9665, 0.9336, 0.9105, 0.8968, 0.8918, 0.8955, 0.908,
    0.9293, 0.9611, 0.982, 0.9812, 0.959, 0.9282, 0.9089, 0.8978, 0.8964, 0.9026, 0.9189, 0.9439,
    0.9778, 0.999, 0.9964, 0.9713, 0.9439, 0.9274, 0.9196, 0.9206, 0.9308, 0.9502, 0.9799, 1.0121,
    1.0226, 1.0071, 0.9752, 0.951, 0.9366, 0.9316, 0.9351, 0.9485, 0.9711, 1.0026, 1.0213, 1.0155,
    0.9863, 0.9547, 0.9347, 0.9232,
];

/// Verbatim 64-ray measured play/orbit triangle state profile (`profiles.ts`).
pub const PROFILE_TRIANGLE: [f64; 64] = [
    0.7819, 0.8211, 0.8747, 0.944, 1.0223, 1.096, 1.1401, 1.134, 1.0808, 1.0047, 0.9265, 0.8603,
    0.8104, 0.773, 0.745, 0.7273, 0.7151, 0.7118, 0.7148, 0.7245, 0.7427, 0.768, 0.8037, 0.8518,
    0.9148, 0.9876, 1.0583, 1.1073, 1.1109, 1.0667, 0.994, 0.9164, 0.8482, 0.7948, 0.7555, 0.7261,
    0.7056, 0.6925, 0.6859, 0.6869, 0.6938, 0.7084, 0.7305, 0.7615, 0.804, 0.8595, 0.9311, 1.0092,
    1.0791, 1.1171, 1.1054, 1.0501, 0.9779, 0.905, 0.845, 0.799, 0.7656, 0.7413, 0.7258, 0.716,
    0.7146, 0.7204, 0.733, 0.7528,
];

struct PolarBasis {
    cos: [f64; 64],
    sin: [f64; 64],
    angles: [f64; 64],
}

static POLAR_BASIS: LazyLock<PolarBasis> = LazyLock::new(|| {
    let mut cos = [0.0; 64];
    let mut sin = [0.0; 64];
    let mut angles = [0.0; 64];
    for i in 0..64 {
        let theta = (TAU * (i as f64)) / 64.0;
        angles[i] = theta;
        cos[i] = theta.cos();
        sin[i] = theta.sin();
    }
    PolarBasis { cos, sin, angles }
});

/// Exclamation state bar 64-ray radii (`et`).
pub static EXCLAIM_BAR_RADII: LazyLock<[f64; 64]> = LazyLock::new(|| {
    let stadium = tangent_stadium(0.0, -0.505, 0.132, 0.0, 0.13, 0.075, 96);
    ray_polygon(&stadium, 0.0, EXCLAIM_CY)
});

/// Alert state bar 64-ray radii (`tt`).
pub static ALERT_BAR_RADII: LazyLock<[f64; 64]> = LazyLock::new(|| {
    let stadium = tangent_stadium(0.0, -0.2535, 0.1345, 0.0, 0.2535, 0.1345, 96);
    ray_polygon(&stadium, 0.0, 0.0)
});

/// Alert state teardrop dot SVG path at unit scale (`it`).
pub static ALERT_DROP_PATH: LazyLock<String> =
    LazyLock::new(|| polygon_svg_path(&alert_drop_points(), 1.0));

/// 64-ray polar silhouette with 2D rigid/anisotropic pose parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Silhouette {
    pub radii: [f64; 64],
    pub rot: f64,
    pub cx: f64,
    pub cy: f64,
    pub sx: f64,
    pub sy: f64,
}

impl Default for Silhouette {
    fn default() -> Self {
        Self {
            radii: [1.0; 64],
            rot: 0.0,
            cx: 0.0,
            cy: 0.0,
            sx: 1.0,
            sy: 1.0,
        }
    }
}

impl Silhouette {
    #[inline]
    pub fn circle(radius: f64) -> Self {
        Self {
            radii: [radius; 64],
            ..Self::default()
        }
    }

    #[inline]
    pub fn from_radii(radii: [f64; 64]) -> Self {
        Self {
            radii,
            ..Self::default()
        }
    }
}

/// Exact ECMAScript `Math.round(x * 100) / 100` (`T(x)`), normalizing `-0.0` to `0.0`.
#[inline]
pub fn js_round2(x: f64) -> f64 {
    if !x.is_finite() {
        return 0.0;
    }
    let scaled = x * 100.0;
    let fl = scaled.floor();
    let rounded = if scaled - fl >= 0.5 { fl + 1.0 } else { fl };
    let out = rounded / 100.0;
    if out == 0.0 {
        0.0
    } else {
        out
    }
}

/// Formats `x` with `js_round2` matching JS `Number.prototype.toString()` (no trailing `.0`).
#[inline]
pub fn fmt_svg_num(x: f64) -> String {
    format!("{}", js_round2(x))
}

/// Wraps an angle in radians into `[-PI, PI]`.
#[inline]
pub fn wrap_pi(mut angle: f64) -> f64 {
    while angle > PI {
        angle -= TAU;
    }
    while angle < -PI {
        angle += TAU;
    }
    angle
}

/// Scales `radii` so its maximum element equals `target_max` (`q(radii, target_max)`).
pub fn normalize_radii(mut radii: [f64; 64], target_max: f64) -> [f64; 64] {
    let max_r = radii.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if max_r <= 0.0 || !max_r.is_finite() {
        return radii;
    }
    let scale = target_max / max_r;
    for r in &mut radii {
        *r *= scale;
    }
    radii
}

/// Ray-polygon intersection from `(ox, oy)` across all 64 polar directions (`W(poly, ox, oy)`).
pub fn ray_polygon(poly: &[(f64, f64)], ox: f64, oy: f64) -> [f64; 64] {
    let mut out = [0.0; 64];
    let m = poly.len();
    if m < 2 {
        return out;
    }
    let basis = &*POLAR_BASIS;
    for a in 0..64 {
        let bx = basis.cos[a];
        let vy = basis.sin[a];
        let mut best = 0.0;
        for j in 0..m {
            let (px, py) = poly[j];
            let (nx, ny) = poly[(j + 1) % m];
            let dx = nx - px;
            let dy = ny - py;
            let det = bx * dy - vy * dx;
            if det.abs() < 1e-9 {
                continue;
            }
            let qx = px - ox;
            let qy = py - oy;
            let h = (qx * dy - qy * dx) / det;
            let g = (qx * vy - qy * bx) / det;
            if h > best && (0.0..=1.0).contains(&g) {
                best = h;
            }
        }
        out[a] = best;
    }
    out
}

/// Tangent stadium / cone-capsule polygon between `(x1, y1, r1)` and `(x2, y2, r2)` (`G`).
pub fn tangent_stadium(
    x1: f64,
    y1: f64,
    r1: f64,
    x2: f64,
    y2: f64,
    r2: f64,
    steps: usize,
) -> Vec<(f64, f64)> {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let raw_l = dx.hypot(dy);
    let l = if raw_l == 0.0 { 1e-6 } else { raw_l };
    let u = dy.atan2(dx);
    let d = ((r1 - r2) / l).clamp(-1.0, 1.0).acos();
    let half = (steps / 2).max(1);
    let mut pts = Vec::with_capacity((half + 1) * 2);
    for k in 0..=half {
        let frac = (k as f64) / (half as f64);
        let alpha = u + d + (TAU - 2.0 * d) * frac;
        pts.push((x1 + r1 * alpha.cos(), y1 + r1 * alpha.sin()));
    }
    for k in 0..=half {
        let frac = (k as f64) / (half as f64);
        let alpha = u - d + (2.0 * d) * frac;
        pts.push((x2 + r2 * alpha.cos(), y2 + r2 * alpha.sin()));
    }
    pts
}

/// Superellipse polar profile (`Me(exp, rx, ry)`).
pub fn superellipse_radii(exp: f64, rx: f64, ry: f64) -> [f64; 64] {
    let basis = &*POLAR_BASIS;
    let mut out = [0.0; 64];
    for i in 0..64 {
        let term = (basis.cos[i] / rx).abs().powf(exp) + (basis.sin[i] / ry).abs().powf(exp);
        out[i] = term.powf(-1.0 / exp);
    }
    out
}

/// Ray-circle union from origin across `(cx, cy, r)` circles (`K`).
pub fn ray_circle_union(circles: &[(f64, f64, f64)]) -> [f64; 64] {
    let basis = &*POLAR_BASIS;
    let mut out = [0.0; 64];
    for a in 0..64 {
        let bx = basis.cos[a];
        let vy = basis.sin[a];
        let mut best = 0.0;
        for &(cx, cy, cr) in circles {
            let e = bx * cx + vy * cy;
            let disc = e * e - (cx * cx + cy * cy - cr * cr);
            if disc >= 0.0 {
                let cand = e + disc.sqrt();
                if cand > best {
                    best = cand;
                }
            }
        }
        out[a] = best;
    }
    out
}

/// Rounded polygon ray-cast from inner corner centers (`Ne`).
pub fn rounded_polygon_radii(verts: &[(f64, f64)], corner_r: f64, arc_steps: usize) -> [f64; 64] {
    let n = verts.len();
    if n < 3 {
        return [0.0; 64];
    }
    let steps = arc_steps.max(1);
    let edge_normal_angle = |(ax, ay): (f64, f64), (bx, by): (f64, f64)| -> f64 {
        let dx = bx - ax;
        let dy = by - ay;
        let len = {
            let h = dx.hypot(dy);
            if h == 0.0 {
                1.0
            } else {
                h
            }
        };
        (-dx / len).atan2(dy / len)
    };
    let mut pts = Vec::with_capacity(n * (steps + 1));
    for o in 0..n {
        let prev = verts[(o + n - 1) % n];
        let cur = verts[o];
        let next = verts[(o + 1) % n];
        let u_prev = edge_normal_angle(prev, cur);
        let u_next = edge_normal_angle(cur, next);
        let du = wrap_pi(u_next - u_prev);
        for s in 0..=steps {
            let alpha = u_prev + du * ((s as f64) / (steps as f64));
            pts.push((
                cur.0 + corner_r * alpha.cos(),
                cur.1 + corner_r * alpha.sin(),
            ));
        }
    }
    ray_polygon(&pts, 0.0, 0.0)
}

/// Regular `n`-gon with rounded corners (`Pe(n, R, corner_r, rot_deg)`).
pub fn regular_rounded_polygon_radii(
    n: usize,
    radius: f64,
    corner_r: f64,
    rot_deg: f64,
) -> [f64; 64] {
    let rot_rad = rot_deg.to_radians();
    let inner = radius - corner_r;
    let verts: Vec<(f64, f64)> = (0..n)
        .map(|k| {
            let a = rot_rad + (TAU * (k as f64)) / (n as f64);
            (inner * a.cos(), inner * a.sin())
        })
        .collect();
    rounded_polygon_radii(&verts, corner_r, 10)
}

/// Per-ray maximum across multiple 64-ray profiles (`Be`).
pub fn union_radii(slices: &[[f64; 64]]) -> [f64; 64] {
    let mut out = [0.0; 64];
    for slice in slices {
        for i in 0..64 {
            if slice[i] > out[i] {
                out[i] = slice[i];
            }
        }
    }
    out
}

/// Axis-aligned rectangle ray profile (`Ve(x1, y1, x2, y2)`).
pub fn rect_radii(x1: f64, y1: f64, x2: f64, y2: f64) -> [f64; 64] {
    ray_polygon(&[(x1, y1), (x2, y1), (x2, y2), (x1, y2)], 0.0, 0.0)
}

/// Arch of `count` circles for `fantome` / `meduse` / `poulpe` (`He`).
pub fn arch_circles(
    count: usize,
    half_span: f64,
    y0: f64,
    r: f64,
    arch: f64,
) -> Vec<(f64, f64, f64)> {
    if count <= 1 {
        return vec![(0.0, y0, r)];
    }
    (0..count)
        .map(|o| {
            let s = ((o as f64) / ((count - 1) as f64)) * 2.0 - 1.0;
            (s * half_span, y0 - arch * s * s, r)
        })
        .collect()
}

/// Closed linear polygon SVG path (`Fe(pts, scale)` -> `M...L...Z`).
pub fn polygon_svg_path(pts: &[(f64, f64)], scale: f64) -> String {
    let Some(&(x0, y0)) = pts.first() else {
        return String::new();
    };
    let mut out = format!("M{} {}", fmt_svg_num(x0 * scale), fmt_svg_num(y0 * scale));
    for &(x, y) in &pts[1..] {
        out.push('L');
        out.push_str(&fmt_svg_num(x * scale));
        out.push(' ');
        out.push_str(&fmt_svg_num(y * scale));
    }
    out.push('Z');
    out
}

/// Returns the 98-point tangent cone-capsule polygon for the `alert` drop dot at unit scale.
pub fn alert_drop_points() -> Vec<(f64, f64)> {
    tangent_stadium(0.0, 0.0, 0.118, 0.0, 0.172, 0.012, 96)
}

/// Exclamation bar silhouette (`nt()`).
pub fn exclaim_bar_silhouette() -> Silhouette {
    Silhouette {
        radii: *EXCLAIM_BAR_RADII,
        cy: EXCLAIM_CY,
        ..Silhouette::default()
    }
}

/// Alert bar silhouette (`rt(overrides)`).
pub fn alert_bar_silhouette(rot: f64, cx: f64, cy: f64) -> Silhouette {
    Silhouette {
        radii: *ALERT_BAR_RADII,
        rot,
        cx,
        cy,
        sx: 1.0,
        sy: 1.0,
    }
}

/// Play/Orbit rounded triangle state silhouette (`ot(rot)`).
pub fn play_triangle_silhouette(rot: f64) -> Silhouette {
    Silhouette {
        radii: PROFILE_TRIANGLE,
        rot,
        cx: -PLAY_TRIANGLE_OFFSET * rot.sin(),
        cy: PLAY_TRIANGLE_OFFSET * rot.cos(),
        sx: 1.0,
        sy: 1.0,
    }
}

fn compute_engine_shape_radii(shape: EngineShapeId) -> [f64; 64] {
    match shape {
        EngineShapeId::Cercle => [1.0; 64],
        EngineShapeId::Galet => {
            let basis = &*POLAR_BASIS;
            let mut raw = [0.0; 64];
            for i in 0..64 {
                let a = basis.angles[i];
                raw[i] = 1.0 + 0.075 * (2.0 * a + 0.5).cos() + 0.035 * (3.0 * a + 2.1).cos();
            }
            normalize_radii(raw, 1.02)
        }
        EngineShapeId::Squircle => normalize_radii(superellipse_radii(4.2, 1.0, 1.0), 1.15),
        EngineShapeId::Capsule => ray_polygon(
            &tangent_stadium(-0.42, 0.0, 0.62, 0.42, 0.0, 0.62, 96),
            0.0,
            0.0,
        ),
        EngineShapeId::Triangle => regular_rounded_polygon_radii(3, 1.12, 0.34, -90.0),
        EngineShapeId::Hexagone => regular_rounded_polygon_radii(6, 1.04, 0.26, 0.0),
        EngineShapeId::Nuage => normalize_radii(
            ray_circle_union(&[
                (-0.44, 0.2, 0.54),
                (0.46, 0.2, 0.5),
                (0.02, 0.3, 0.6),
                (-0.24, -0.3, 0.48),
                (0.3, -0.24, 0.44),
            ]),
            1.02,
        ),
        EngineShapeId::Goutte => normalize_radii(
            ray_polygon(
                &tangent_stadium(0.0, 0.28, 0.66, 0.0, -0.96, 0.05, 96),
                0.0,
                0.0,
            ),
            1.04,
        ),
        EngineShapeId::Carre => regular_rounded_polygon_radii(4, 1.2, 0.24, 45.0),
        EngineShapeId::Losange => regular_rounded_polygon_radii(4, 1.14, 0.3, -90.0),
        EngineShapeId::Pentagone => regular_rounded_polygon_radii(5, 1.08, 0.28, -90.0),
        EngineShapeId::Fantome => normalize_radii(
            union_radii(&[
                ray_circle_union(&[(0.0, -0.12, 0.8)]),
                rect_radii(-0.8, -0.12, 0.8, 0.62),
                ray_circle_union(&arch_circles(3, 0.54, 0.66, 0.27, 0.0)),
            ]),
            1.1,
        ),
        EngineShapeId::Meduse => {
            let dome: Vec<(f64, f64)> = (0..=40)
                .map(|t| {
                    let a = PI + ((t as f64) / 40.0) * PI;
                    (a.cos() * 0.9, a.sin() * 0.86 + 0.18)
                })
                .collect();
            normalize_radii(
                union_radii(&[
                    ray_polygon(&dome, 0.0, 0.0),
                    rect_radii(-0.9, 0.0, 0.9, 0.3),
                    ray_circle_union(&arch_circles(6, 0.72, 0.38, 0.17, 0.08)),
                ]),
                1.1,
            )
        }
        EngineShapeId::Poulpe => normalize_radii(
            union_radii(&[
                ray_circle_union(&[(0.0, -0.18, 0.78)]),
                rect_radii(-0.66, -0.1, 0.66, 0.5),
                ray_circle_union(&arch_circles(4, 0.78, 0.6, 0.22, 0.16)),
            ]),
            1.1,
        ),
    }
}

static ENGINE_SHAPES_TABLE: LazyLock<[[f64; 64]; 14]> = LazyLock::new(|| {
    let mut table = [[0.0; 64]; 14];
    for (idx, shape) in EngineShapeId::ALL.iter().copied().enumerate() {
        table[idx] = compute_engine_shape_radii(shape);
    }
    table
});

#[inline]
pub(crate) const fn engine_shape_index(shape: EngineShapeId) -> usize {
    match shape {
        EngineShapeId::Cercle => 0,
        EngineShapeId::Galet => 1,
        EngineShapeId::Squircle => 2,
        EngineShapeId::Capsule => 3,
        EngineShapeId::Triangle => 4,
        EngineShapeId::Hexagone => 5,
        EngineShapeId::Nuage => 6,
        EngineShapeId::Goutte => 7,
        EngineShapeId::Carre => 8,
        EngineShapeId::Losange => 9,
        EngineShapeId::Pentagone => 10,
        EngineShapeId::Fantome => 11,
        EngineShapeId::Meduse => 12,
        EngineShapeId::Poulpe => 13,
    }
}

#[inline]
pub const fn bloub_to_engine_shape(shape: BloubShape) -> EngineShapeId {
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

/// Returns the precomputed 64-ray silhouette profile for any of the 14 `EngineShapeId`s.
#[inline]
pub fn engine_shape_radii(shape: EngineShapeId) -> [f64; 64] {
    ENGINE_SHAPES_TABLE[engine_shape_index(shape)]
}

/// Returns the precomputed 64-ray silhouette profile for any of the 13 `BloubShape`s.
#[inline]
pub fn bloub_shape_radii(shape: BloubShape) -> [f64; 64] {
    engine_shape_radii(bloub_to_engine_shape(shape))
}

/// Parses an engine shape identifier (`"cercle".."poulpe"`, including `"goutte"`).
pub fn parse_engine_shape(raw: &str) -> Option<EngineShapeId> {
    let trimmed = raw.trim();
    EngineShapeId::ALL
        .iter()
        .copied()
        .find(|shape| shape.as_str().eq_ignore_ascii_case(trimmed))
}

/// Continuous radial interpolation at arbitrary `angle_rad` (`je(radii, angle_rad)`).
#[inline]
pub fn radius_at_angle(radii: &[f64; 64], angle_rad: f64) -> f64 {
    let wrapped = (((angle_rad / TAU) % 1.0) + 1.0) % 1.0;
    let u = wrapped * 64.0;
    let i = u.floor() as usize;
    let frac = u - (i as f64);
    let r0 = radii[i % 64];
    let r1 = radii[(i + 1) % 64];
    r0 + (r1 - r0) * frac
}

/// Interpolates two silhouettes (`U(a, b, t)`) using shortest-arc rotation wrapping.
pub fn lerp_silhouette(a: &Silhouette, b: &Silhouette, t: f64) -> Silhouette {
    let mut radii = [0.0; 64];
    for i in 0..64 {
        radii[i] = a.radii[i] + (b.radii[i] - a.radii[i]) * t;
    }
    let da = wrap_pi(b.rot - a.rot);
    Silhouette {
        radii,
        rot: a.rot + da * t,
        cx: a.cx + (b.cx - a.cx) * t,
        cy: a.cy + (b.cy - a.cy) * t,
        sx: a.sx + (b.sx - a.sx) * t,
        sy: a.sy + (b.sy - a.sy) * t,
    }
}

/// Projects the 64 polar rays of `sil` into Cartesian viewBox points (`ke(sil, scale)`).
pub fn project_silhouette_points(sil: &Silhouette, scale: f64) -> [(f64, f64); 64] {
    let basis = &*POLAR_BASIS;
    let (s, c) = sil.rot.sin_cos();
    let mut pts = [(0.0, 0.0); 64];
    for i in 0..64 {
        let r = sil.radii[i];
        let x0 = r * basis.cos[i];
        let y0 = r * basis.sin[i];
        let lx = x0 * c - y0 * s;
        let ly = x0 * s + y0 * c;
        pts[i] = (
            (lx * sil.sx + sil.cx) * scale,
            (ly * sil.sy + sil.cy) * scale,
        );
    }
    pts
}

/// Closed 64-segment Catmull-Rom (`t = 1/6`) cubic Bezier SVG path (`Ae`, `M...C...Z`).
pub fn silhouette_svg_path(sil: &Silhouette, scale: f64) -> String {
    let pts = project_silhouette_points(sil, scale);
    let mut out = String::with_capacity(2048);
    out.push('M');
    out.push_str(&fmt_svg_num(pts[0].0));
    out.push(' ');
    out.push_str(&fmt_svg_num(pts[0].1));
    for r in 0..64 {
        let a = pts[(r + 63) % 64];
        let o = pts[r];
        let s = pts[(r + 1) % 64];
        let c = pts[(r + 2) % 64];
        let c1x = o.0 + (s.0 - a.0) / 6.0;
        let c1y = o.1 + (s.1 - a.1) / 6.0;
        let c2x = s.0 - (c.0 - o.0) / 6.0;
        let c2y = s.1 - (c.1 - o.1) / 6.0;
        out.push('C');
        out.push_str(&fmt_svg_num(c1x));
        out.push(' ');
        out.push_str(&fmt_svg_num(c1y));
        out.push(' ');
        out.push_str(&fmt_svg_num(c2x));
        out.push(' ');
        out.push_str(&fmt_svg_num(c2y));
        out.push(' ');
        out.push_str(&fmt_svg_num(s.0));
        out.push(' ');
        out.push_str(&fmt_svg_num(s.1));
    }
    out.push('Z');
    out
}

/// Emits the closed 64-segment Catmull-Rom cubic Bezier silhouette into a GPUI `PathBuilder`.
/// Note: GPUI `0.2.2` `PathBuilder::cubic_bezier_to(to, ctrl1, ctrl2)` takes `to` FIRST.
pub fn emit_silhouette_gpui(
    builder: &mut PathBuilder,
    sil: &Silhouette,
    center: Point<Pixels>,
    px_per_vb: f32,
) {
    let pts = project_silhouette_points(sil, RAYON);
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let map_pt = |(vx, vy): (f64, f64)| -> Point<Pixels> {
        point(
            px(cx + (vx as f32) * px_per_vb),
            px(cy + (vy as f32) * px_per_vb),
        )
    };
    builder.move_to(map_pt(pts[0]));
    for r in 0..64 {
        let a = pts[(r + 63) % 64];
        let o = pts[r];
        let s = pts[(r + 1) % 64];
        let c = pts[(r + 2) % 64];
        let c1 = (o.0 + (s.0 - a.0) / 6.0, o.1 + (s.1 - a.1) / 6.0);
        let c2 = (s.0 - (c.0 - o.0) / 6.0, s.1 - (c.1 - o.1) / 6.0);
        builder.cubic_bezier_to(map_pt(s), map_pt(c1), map_pt(c2));
    }
    builder.close();
}

/// Builds a filled GPUI `Path<Pixels>` for `sil` centered at `center` with `px_per_vb` scale.
pub fn silhouette_gpui_path(
    sil: &Silhouette,
    center: Point<Pixels>,
    px_per_vb: f32,
) -> Option<Path<Pixels>> {
    if !px_per_vb.is_finite() || px_per_vb <= 0.0 {
        return None;
    }
    let mut builder = PathBuilder::fill();
    emit_silhouette_gpui(&mut builder, sil, center, px_per_vb);
    builder.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_round2_matches_ecmascript_math_round() {
        assert_eq!(js_round2(0.0), 0.0);
        assert_eq!(js_round2(-0.0), 0.0);
        assert_eq!(js_round2(-0.005), 0.0);
        assert_eq!(js_round2(-0.0051), -0.01);
        assert_eq!(js_round2(1.005), 1.0);
        assert_eq!(js_round2(0.125), 0.13);
        assert_eq!(js_round2(-0.125), -0.12);
        assert_eq!(fmt_svg_num(-0.004), "0");
    }

    #[test]
    fn all_14_engine_shapes_match_upstream_invariants() {
        for shape in EngineShapeId::ALL {
            let radii = engine_shape_radii(shape);
            assert_eq!(radii.len(), 64);
            assert!(radii.iter().all(|r| r.is_finite() && *r > 0.2));
            assert_eq!(parse_engine_shape(shape.as_str()), Some(shape));
        }
        let capsule = engine_shape_radii(EngineShapeId::Capsule);
        assert!((capsule[0] - 1.04).abs() < 1e-6);
        let tri = engine_shape_radii(EngineShapeId::Triangle);
        let expected = [0.842931, 0.898078, 0.970952, 1.048094];
        for (got, exp) in tri[..4].iter().zip(expected.iter()) {
            assert!((got - exp).abs() < 1e-5, "got {got}, expected {exp}");
        }
        let svg = silhouette_svg_path(&Silhouette::from_radii(tri), RAYON);
        assert!(svg.starts_with('M') && svg.ends_with('Z'));
        assert_eq!(svg.chars().filter(|&ch| ch == 'C').count(), 64);
    }
}
