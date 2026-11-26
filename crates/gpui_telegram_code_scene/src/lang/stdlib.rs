//! Standard library (68 builtins + `PI`/`TAU`) for `CodeSceneScript` (`cs1`).
//!
//! Supports both direct calls (`clear(...)`, `circle(...)`) and method syntax
//! (`canvas.clear(...)`, `list.push(...)`, `map.keys()`, `str.len()`).

use std::f64::consts::PI;
use std::sync::Arc;

use crate::lang::heap::{HeapObj, SplitMix64, Val, VmHeap};
use crate::lang::limits::VmBudgets;
use crate::lang::shaders::{
    ShaderWatchdog, run_shader_mandelbrot, run_shader_plasma, run_shader_starfield,
    run_shader_synthwave, run_shader_voronoi,
};
use crate::model::{AudioPulse, RgbaColor, RuntimeError, ToneWave, sanitize_f64};
use crate::runtime::frame::{
    DrawCmd, MAX_POLY_POINTS, TextAlign, clamp_coord, clamp_font_size, clamp_non_neg_size,
    clamp_radius, clamp_stroke, sanitize_text,
};
use crate::runtime::pixel::PixelBuffer;

/// Host interface implemented by `runtime::session` for canvas, pixel, and audio emissions.
pub trait VmHost {
    fn canvas_size(&self) -> (u32, u32);
    fn canvas_fps(&self) -> u32;
    fn canvas_duration(&self) -> Option<f64>;
    fn push_draw_cmd(&mut self, cmd: DrawCmd);
    fn record_invalid_draw(&mut self);
    fn push_audio_pulse(&mut self, pulse: AudioPulse);
    fn pixel_init(&mut self, w: f64, h: f64) -> Result<(), RuntimeError>;
    fn pixel_ensure_default(&mut self) -> Result<&mut PixelBuffer, RuntimeError>;
    fn pixel_buffer_mut(&mut self) -> Option<&mut PixelBuffer>;
}

/// All 68 canonical stdlib builtins resolved in O(1) at compile time.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BuiltinId {
    // Math (18)
    Sin, Cos, Tan, Atan2, Sqrt, Abs, Floor, Ceil, Round,
    Min, Max, Clamp, Lerp, Smoothstep, Pow, Exp, Fract, Hypot,
    // Easing & Timeline (5)
    EaseInOut, EaseOutCubic, EaseOutBounce, EaseOutElastic, Keyframe,
    // Noise & RNG (5)
    Noise2d, Fbm2d, Rand, RandRange, Hash2,
    // Color (5)
    Rgb, Rgba, Hsl, Hsla, LerpColor,
    // 2D Vector Canvas & Audio (16)
    Clear, ClearGrad, Rect, RoundRect, StrokeRect,
    Circle, StrokeCircle, GlowCircle, Line, Polyline, Polygon,
    Arc, Bezier, Star, Text, Tone,
    // Pixel & Shader Kernels (8)
    PixelInit, PixelSet, PixelRect,
    ShaderPlasma, ShaderMandelbrot, ShaderVoronoi, ShaderStarfield, ShaderSynthwave,
    // Collision (4)
    Dist, AabbHit, CircleHit, CircleRectHit,
    // Utilities (7)
    Len, Push, Pop, Str, Num, Keys, HasKey,
}

impl BuiltinId {
    pub const ALL: [Self; 68] = [
        Self::Sin, Self::Cos, Self::Tan, Self::Atan2, Self::Sqrt, Self::Abs, Self::Floor, Self::Ceil, Self::Round,
        Self::Min, Self::Max, Self::Clamp, Self::Lerp, Self::Smoothstep, Self::Pow, Self::Exp, Self::Fract, Self::Hypot,
        Self::EaseInOut, Self::EaseOutCubic, Self::EaseOutBounce, Self::EaseOutElastic, Self::Keyframe,
        Self::Noise2d, Self::Fbm2d, Self::Rand, Self::RandRange, Self::Hash2,
        Self::Rgb, Self::Rgba, Self::Hsl, Self::Hsla, Self::LerpColor,
        Self::Clear, Self::ClearGrad, Self::Rect, Self::RoundRect, Self::StrokeRect,
        Self::Circle, Self::StrokeCircle, Self::GlowCircle, Self::Line, Self::Polyline, Self::Polygon,
        Self::Arc, Self::Bezier, Self::Star, Self::Text, Self::Tone,
        Self::PixelInit, Self::PixelSet, Self::PixelRect,
        Self::ShaderPlasma, Self::ShaderMandelbrot, Self::ShaderVoronoi, Self::ShaderStarfield, Self::ShaderSynthwave,
        Self::Dist, Self::AabbHit, Self::CircleHit, Self::CircleRectHit,
        Self::Len, Self::Push, Self::Pop, Self::Str, Self::Num, Self::Keys, Self::HasKey,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "sin" => Self::Sin, "cos" => Self::Cos, "tan" => Self::Tan,
            "atan2" => Self::Atan2, "sqrt" => Self::Sqrt, "abs" => Self::Abs,
            "floor" => Self::Floor, "ceil" => Self::Ceil, "round" => Self::Round,
            "min" => Self::Min, "max" => Self::Max, "clamp" => Self::Clamp,
            "lerp" => Self::Lerp, "smoothstep" => Self::Smoothstep, "pow" => Self::Pow,
            "exp" => Self::Exp, "fract" => Self::Fract, "hypot" => Self::Hypot,
            "ease_in_out" => Self::EaseInOut, "ease_out_cubic" => Self::EaseOutCubic,
            "ease_out_bounce" => Self::EaseOutBounce, "ease_out_elastic" => Self::EaseOutElastic,
            "keyframe" => Self::Keyframe,
            "noise2d" => Self::Noise2d, "fbm2d" => Self::Fbm2d,
            "rand" => Self::Rand, "rand_range" => Self::RandRange, "hash2" => Self::Hash2,
            "rgb" => Self::Rgb, "rgba" => Self::Rgba, "hsl" => Self::Hsl,
            "hsla" => Self::Hsla, "lerp_color" => Self::LerpColor,
            "clear" => Self::Clear, "clear_grad" => Self::ClearGrad,
            "rect" => Self::Rect, "round_rect" => Self::RoundRect,
            "stroke_rect" => Self::StrokeRect, "circle" => Self::Circle,
            "stroke_circle" => Self::StrokeCircle, "glow_circle" => Self::GlowCircle,
            "line" => Self::Line, "polyline" => Self::Polyline, "polygon" => Self::Polygon,
            "arc" => Self::Arc, "bezier" => Self::Bezier, "star" => Self::Star,
            "text" => Self::Text, "tone" => Self::Tone,
            "pixel_init" => Self::PixelInit, "pixel_set" => Self::PixelSet,
            "pixel_rect" => Self::PixelRect, "shader_plasma" => Self::ShaderPlasma,
            "shader_mandelbrot" => Self::ShaderMandelbrot, "shader_voronoi" => Self::ShaderVoronoi,
            "shader_starfield" => Self::ShaderStarfield, "shader_synthwave" => Self::ShaderSynthwave,
            "dist" => Self::Dist, "aabb_hit" => Self::AabbHit,
            "circle_hit" => Self::CircleHit, "circle_rect_hit" => Self::CircleRectHit,
            "len" => Self::Len, "push" => Self::Push, "pop" => Self::Pop,
            "str" => Self::Str, "num" => Self::Num, "keys" => Self::Keys,
            "has_key" => Self::HasKey,
            _ => return None,
        })
    }

    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Sin => "sin", Self::Cos => "cos", Self::Tan => "tan",
            Self::Atan2 => "atan2", Self::Sqrt => "sqrt", Self::Abs => "abs",
            Self::Floor => "floor", Self::Ceil => "ceil", Self::Round => "round",
            Self::Min => "min", Self::Max => "max", Self::Clamp => "clamp",
            Self::Lerp => "lerp", Self::Smoothstep => "smoothstep", Self::Pow => "pow",
            Self::Exp => "exp", Self::Fract => "fract", Self::Hypot => "hypot",
            Self::EaseInOut => "ease_in_out", Self::EaseOutCubic => "ease_out_cubic",
            Self::EaseOutBounce => "ease_out_bounce", Self::EaseOutElastic => "ease_out_elastic",
            Self::Keyframe => "keyframe",
            Self::Noise2d => "noise2d", Self::Fbm2d => "fbm2d",
            Self::Rand => "rand", Self::RandRange => "rand_range", Self::Hash2 => "hash2",
            Self::Rgb => "rgb", Self::Rgba => "rgba", Self::Hsl => "hsl",
            Self::Hsla => "hsla", Self::LerpColor => "lerp_color",
            Self::Clear => "clear", Self::ClearGrad => "clear_grad",
            Self::Rect => "rect", Self::RoundRect => "round_rect",
            Self::StrokeRect => "stroke_rect", Self::Circle => "circle",
            Self::StrokeCircle => "stroke_circle", Self::GlowCircle => "glow_circle",
            Self::Line => "line", Self::Polyline => "polyline", Self::Polygon => "polygon",
            Self::Arc => "arc", Self::Bezier => "bezier", Self::Star => "star",
            Self::Text => "text", Self::Tone => "tone",
            Self::PixelInit => "pixel_init", Self::PixelSet => "pixel_set",
            Self::PixelRect => "pixel_rect", Self::ShaderPlasma => "shader_plasma",
            Self::ShaderMandelbrot => "shader_mandelbrot", Self::ShaderVoronoi => "shader_voronoi",
            Self::ShaderStarfield => "shader_starfield", Self::ShaderSynthwave => "shader_synthwave",
            Self::Dist => "dist", Self::AabbHit => "aabb_hit",
            Self::CircleHit => "circle_hit", Self::CircleRectHit => "circle_rect_hit",
            Self::Len => "len", Self::Push => "push", Self::Pop => "pop",
            Self::Str => "str", Self::Num => "num", Self::Keys => "keys",
            Self::HasKey => "has_key",
        }
    }

    pub const fn is_canvas_method(self) -> bool {
        matches!(
            self,
            Self::Clear | Self::ClearGrad | Self::Rect | Self::RoundRect | Self::StrokeRect
                | Self::Circle | Self::StrokeCircle | Self::GlowCircle | Self::Line
                | Self::Polyline | Self::Polygon | Self::Arc | Self::Bezier | Self::Star
                | Self::Text | Self::Tone
                | Self::PixelInit | Self::PixelSet | Self::PixelRect
                | Self::ShaderPlasma | Self::ShaderMandelbrot | Self::ShaderVoronoi
                | Self::ShaderStarfield | Self::ShaderSynthwave
        )
    }
}

/// Execution context passed into stdlib builtins.
pub struct BuiltinCtx<'a> {
    pub heap: &'a mut VmHeap,
    pub rng: &'a mut SplitMix64,
    pub budgets: &'a VmBudgets,
    pub host: &'a mut dyn VmHost,
    pub watchdog: &'a mut ShaderWatchdog<'a>,
}

fn check_arity(name: &str, args: &[Val], min: usize, max: usize) -> Result<(), RuntimeError> {
    if args.len() < min || args.len() > max {
        let expected = if min == max {
            min.to_string()
        } else {
            format!("{min}..={max}")
        };
        return Err(RuntimeError::ArityMismatch {
            name: name.to_string(),
            expected,
            got: args.len(),
        });
    }
    Ok(())
}

fn arg_num(args: &[Val], idx: usize) -> f64 {
    args.get(idx).map_or(0.0, Val::as_num)
}

fn arg_str(args: &[Val], idx: usize, heap: &VmHeap, max_bytes: usize) -> Result<String, RuntimeError> {
    match args.get(idx) {
        Some(Val::Str(s)) => Ok(s.to_string()),
        Some(v) => heap.format_val(v, max_bytes),
        None => Ok(String::new()),
    }
}

pub fn dispatch_builtin(
    builtin: BuiltinId,
    args: &[Val],
    ctx: &mut BuiltinCtx<'_>,
) -> Result<Val, RuntimeError> {
    let name = builtin.canonical_name();
    match builtin {
        // --- Math (18) ---
        BuiltinId::Sin => { check_arity(name, args, 1, 1)?; Ok(Val::Num(sanitize_f64(arg_num(args, 0).sin()))) }
        BuiltinId::Cos => { check_arity(name, args, 1, 1)?; Ok(Val::Num(sanitize_f64(arg_num(args, 0).cos()))) }
        BuiltinId::Tan => {
            check_arity(name, args, 1, 1)?;
            let v = sanitize_f64(arg_num(args, 0).tan()).clamp(-1e6, 1e6);
            Ok(Val::Num(v))
        }
        BuiltinId::Atan2 => { check_arity(name, args, 2, 2)?; Ok(Val::Num(sanitize_f64(arg_num(args, 0).atan2(arg_num(args, 1))))) }
        BuiltinId::Sqrt => {
            check_arity(name, args, 1, 1)?;
            let x = arg_num(args, 0);
            Ok(Val::Num(if x.is_finite() && x > 0.0 { x.sqrt() } else { 0.0 }))
        }
        BuiltinId::Abs => { check_arity(name, args, 1, 1)?; Ok(Val::Num(sanitize_f64(arg_num(args, 0).abs()))) }
        BuiltinId::Floor => { check_arity(name, args, 1, 1)?; Ok(Val::Num(sanitize_f64(arg_num(args, 0).floor()))) }
        BuiltinId::Ceil => { check_arity(name, args, 1, 1)?; Ok(Val::Num(sanitize_f64(arg_num(args, 0).ceil()))) }
        BuiltinId::Round => { check_arity(name, args, 1, 1)?; Ok(Val::Num(sanitize_f64(arg_num(args, 0).round()))) }
        BuiltinId::Min => {
            check_arity(name, args, 2, 2)?;
            let a = sanitize_f64(arg_num(args, 0));
            let b = sanitize_f64(arg_num(args, 1));
            Ok(Val::Num(a.min(b)))
        }
        BuiltinId::Max => {
            check_arity(name, args, 2, 2)?;
            let a = sanitize_f64(arg_num(args, 0));
            let b = sanitize_f64(arg_num(args, 1));
            Ok(Val::Num(a.max(b)))
        }
        BuiltinId::Clamp => {
            check_arity(name, args, 3, 3)?;
            let x = sanitize_f64(arg_num(args, 0));
            let a = sanitize_f64(arg_num(args, 1));
            let b = sanitize_f64(arg_num(args, 2));
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            Ok(Val::Num(x.clamp(lo, hi)))
        }
        BuiltinId::Lerp => {
            check_arity(name, args, 3, 3)?;
            let a = sanitize_f64(arg_num(args, 0));
            let b = sanitize_f64(arg_num(args, 1));
            let t = sanitize_f64(arg_num(args, 2));
            Ok(Val::Num(sanitize_f64(a + (b - a) * t)))
        }
        BuiltinId::Smoothstep => {
            check_arity(name, args, 3, 3)?;
            let e0 = sanitize_f64(arg_num(args, 0));
            let e1 = sanitize_f64(arg_num(args, 1));
            let x = sanitize_f64(arg_num(args, 2));
            if e0 < e1 {
                if x <= e0 {
                    return Ok(Val::Num(0.0));
                }
                if x >= e1 {
                    return Ok(Val::Num(1.0));
                }
            } else if e0 > e1 {
                if x >= e0 {
                    return Ok(Val::Num(0.0));
                }
                if x <= e1 {
                    return Ok(Val::Num(1.0));
                }
            } else {
                return Ok(Val::Num(if x >= e1 { 1.0 } else { 0.0 }));
            }
            let diff = e1 - e0;
            if diff.abs() <= f64::EPSILON {
                return Ok(Val::Num(if x >= e1 { 1.0 } else { 0.0 }));
            }
            let raw_t = if diff.is_finite() {
                (x - e0) / diff
            } else {
                (x * 0.5 - e0 * 0.5) / (e1 * 0.5 - e0 * 0.5)
            };
            let t = sanitize_f64(raw_t).clamp(0.0, 1.0);
            Ok(Val::Num(sanitize_f64(t * t * (3.0 - 2.0 * t))))
        }
        BuiltinId::Pow => {
            check_arity(name, args, 2, 2)?;
            let a = sanitize_f64(arg_num(args, 0));
            let b = sanitize_f64(arg_num(args, 1));
            Ok(Val::Num(sanitize_f64(a.powf(b))))
        }
        BuiltinId::Exp => {
            check_arity(name, args, 1, 1)?;
            let x = sanitize_f64(arg_num(args, 0)).clamp(-700.0, 700.0);
            Ok(Val::Num(sanitize_f64(x.exp())))
        }
        BuiltinId::Fract => {
            check_arity(name, args, 1, 1)?;
            let x = sanitize_f64(arg_num(args, 0));
            Ok(Val::Num(x - x.floor()))
        }
        BuiltinId::Hypot => {
            check_arity(name, args, 2, 2)?;
            Ok(Val::Num(sanitize_f64(sanitize_f64(arg_num(args, 0)).hypot(sanitize_f64(arg_num(args, 1))))))
        }

        // --- Easing & Timeline (5) ---
        BuiltinId::EaseInOut => { check_arity(name, args, 1, 1)?; Ok(Val::Num(ease_in_out(arg_num(args, 0)))) }
        BuiltinId::EaseOutCubic => { check_arity(name, args, 1, 1)?; Ok(Val::Num(ease_out_cubic(arg_num(args, 0)))) }
        BuiltinId::EaseOutBounce => { check_arity(name, args, 1, 1)?; Ok(Val::Num(ease_out_bounce(arg_num(args, 0)))) }
        BuiltinId::EaseOutElastic => { check_arity(name, args, 1, 1)?; Ok(Val::Num(ease_out_elastic(arg_num(args, 0)))) }
        BuiltinId::Keyframe => {
            check_arity(name, args, 2, 2)?;
            Ok(Val::Num(eval_keyframe(arg_num(args, 0), &args[1], ctx.heap)))
        }

        // --- Noise & RNG (5) ---
        BuiltinId::Noise2d => { check_arity(name, args, 2, 2)?; Ok(Val::Num(noise2d(arg_num(args, 0), arg_num(args, 1)))) }
        BuiltinId::Fbm2d => {
            check_arity(name, args, 2, 3)?;
            let oct = if args.len() == 3 { arg_num(args, 2) } else { 4.0 };
            Ok(Val::Num(fbm2d(arg_num(args, 0), arg_num(args, 1), oct)))
        }
        BuiltinId::Rand => { check_arity(name, args, 0, 0)?; Ok(Val::Num(ctx.rng.next_f64())) }
        BuiltinId::RandRange => {
            check_arity(name, args, 2, 2)?;
            let a = sanitize_f64(arg_num(args, 0));
            let b = sanitize_f64(arg_num(args, 1));
            Ok(Val::Num(sanitize_f64(a + (b - a) * ctx.rng.next_f64())))
        }
        BuiltinId::Hash2 => { check_arity(name, args, 2, 2)?; Ok(Val::Num(hash2(arg_num(args, 0), arg_num(args, 1)))) }

        // --- Color (5) ---
        BuiltinId::Rgb => {
            check_arity(name, args, 3, 3)?;
            let c = pack_rgba(arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), 255.0, false);
            Ok(Val::Num(c.to_u32_rgba() as f64))
        }
        BuiltinId::Rgba => {
            check_arity(name, args, 4, 4)?;
            let c = pack_rgba(arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), arg_num(args, 3), true);
            Ok(Val::Num(c.to_u32_rgba() as f64))
        }
        BuiltinId::Hsl => {
            check_arity(name, args, 3, 3)?;
            let c = pack_hsla(arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), 1.0);
            Ok(Val::Num(c.to_u32_rgba() as f64))
        }
        BuiltinId::Hsla => {
            check_arity(name, args, 4, 4)?;
            let c = pack_hsla(arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), arg_num(args, 3));
            Ok(Val::Num(c.to_u32_rgba() as f64))
        }
        BuiltinId::LerpColor => {
            check_arity(name, args, 3, 3)?;
            let c1 = RgbaColor::from_script_num(arg_num(args, 0));
            let c2 = RgbaColor::from_script_num(arg_num(args, 1));
            let t = sanitize_f64(arg_num(args, 2)).clamp(0.0, 1.0);
            let mix = |a: u8, b: u8| ((a as f64) + ((b as f64) - (a as f64)) * t).round().clamp(0.0, 255.0) as u8;
            let out = RgbaColor::new(mix(c1.r, c2.r), mix(c1.g, c2.g), mix(c1.b, c2.b), mix(c1.a, c2.a));
            Ok(Val::Num(out.to_u32_rgba() as f64))
        }

        // --- Canvas, Pixels, Shaders, Collision, Utilities ---
        _ => dispatch_canvas_and_util(builtin, args, ctx),
    }
}

fn dispatch_canvas_and_util(
    builtin: BuiltinId,
    args: &[Val],
    ctx: &mut BuiltinCtx<'_>,
) -> Result<Val, RuntimeError> {
    let name = builtin.canonical_name();
    match builtin {
        // --- 2D Vector Canvas & Audio (16) ---
        BuiltinId::Clear => {
            check_arity(name, args, 1, 1)?;
            let color = RgbaColor::from_script_num(arg_num(args, 0));
            ctx.host.push_draw_cmd(DrawCmd::Clear { color });
            Ok(Val::Null)
        }
        BuiltinId::ClearGrad => {
            check_arity(name, args, 2, 3)?;
            let top = RgbaColor::from_script_num(arg_num(args, 0));
            let bottom = RgbaColor::from_script_num(arg_num(args, 1));
            let angle = if args.len() == 3 { arg_num(args, 2) } else { 180.0 };
            if !angle.is_finite() {
                ctx.host.record_invalid_draw();
            } else {
                ctx.host.push_draw_cmd(DrawCmd::LinearGradientBg {
                    top,
                    bottom,
                    angle_deg: angle.rem_euclid(360.0) as f32,
                });
            }
            Ok(Val::Null)
        }
        BuiltinId::Rect => {
            check_arity(name, args, 5, 5)?;
            if let (Some(x), Some(y), Some(w), Some(h)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_non_neg_size(arg_num(args, 2)),
                clamp_non_neg_size(arg_num(args, 3)),
            ) {
                let color = RgbaColor::from_script_num(arg_num(args, 4));
                ctx.host.push_draw_cmd(DrawCmd::FillRect { x, y, w, h, color });
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::RoundRect => {
            check_arity(name, args, 6, 6)?;
            let r_raw = arg_num(args, 4);
            if let (Some(x), Some(y), Some(w), Some(h)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_non_neg_size(arg_num(args, 2)),
                clamp_non_neg_size(arg_num(args, 3)),
            ) {
                if r_raw.is_finite() {
                    let radius = r_raw.clamp(0.0, (w.min(h) as f64) * 0.5) as f32;
                    let color = RgbaColor::from_script_num(arg_num(args, 5));
                    ctx.host.push_draw_cmd(DrawCmd::RoundRect { x, y, w, h, radius, color });
                } else {
                    ctx.host.record_invalid_draw();
                }
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::StrokeRect => {
            check_arity(name, args, 5, 6)?;
            let sw_raw = if args.len() == 6 { arg_num(args, 5) } else { 2.0 };
            if let (Some(x), Some(y), Some(w), Some(h), Some(sw)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_non_neg_size(arg_num(args, 2)),
                clamp_non_neg_size(arg_num(args, 3)),
                clamp_stroke(sw_raw),
            ) {
                let color = RgbaColor::from_script_num(arg_num(args, 4));
                ctx.host.push_draw_cmd(DrawCmd::StrokeRect { x, y, w, h, stroke_width: sw, color });
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Circle => {
            check_arity(name, args, 4, 4)?;
            if let (Some(cx), Some(cy), Some(radius)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_radius(arg_num(args, 2)),
            ) {
                let color = RgbaColor::from_script_num(arg_num(args, 3));
                ctx.host.push_draw_cmd(DrawCmd::FillCircle { cx, cy, radius, color });
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::StrokeCircle => {
            check_arity(name, args, 4, 5)?;
            let sw_raw = if args.len() == 5 { arg_num(args, 4) } else { 2.0 };
            if let (Some(cx), Some(cy), Some(radius), Some(sw)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_radius(arg_num(args, 2)),
                clamp_stroke(sw_raw),
            ) {
                let color = RgbaColor::from_script_num(arg_num(args, 3));
                ctx.host.push_draw_cmd(DrawCmd::StrokeCircle { cx, cy, radius, stroke_width: sw, color });
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::GlowCircle => {
            check_arity(name, args, 4, 5)?;
            let inten_raw = if args.len() == 5 { arg_num(args, 4) } else { 1.0 };
            if let (Some(cx), Some(cy), Some(radius)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_radius(arg_num(args, 2)),
            ) {
                if inten_raw.is_finite() {
                    let intensity = inten_raw.clamp(0.1, 2.0) as f32;
                    let color = RgbaColor::from_script_num(arg_num(args, 3));
                    ctx.host.push_draw_cmd(DrawCmd::GlowCircle { cx, cy, radius, color, intensity });
                } else {
                    ctx.host.record_invalid_draw();
                }
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Line => {
            check_arity(name, args, 5, 6)?;
            let sw_raw = if args.len() == 6 { arg_num(args, 5) } else { 2.0 };
            if let (Some(x1), Some(y1), Some(x2), Some(y2), Some(sw)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_coord(arg_num(args, 2)),
                clamp_coord(arg_num(args, 3)),
                clamp_stroke(sw_raw),
            ) {
                let color = RgbaColor::from_script_num(arg_num(args, 4));
                ctx.host.push_draw_cmd(DrawCmd::Line { x1, y1, x2, y2, stroke_width: sw, color });
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Polyline => {
            check_arity(name, args, 2, 3)?;
            let sw_raw = if args.len() == 3 { arg_num(args, 2) } else { 2.0 };
            if let (Some(points), Some(sw)) = (extract_points(&args[0], ctx.heap), clamp_stroke(sw_raw)) {
                if points.len() >= 2 {
                    let color = RgbaColor::from_script_num(arg_num(args, 1));
                    ctx.host.push_draw_cmd(DrawCmd::Polyline { points, stroke_width: sw, color });
                }
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Polygon => {
            check_arity(name, args, 2, 2)?;
            if let Some(points) = extract_points(&args[0], ctx.heap) {
                if points.len() >= 3 {
                    let color = RgbaColor::from_script_num(arg_num(args, 1));
                    ctx.host.push_draw_cmd(DrawCmd::FillPolygon { points, color });
                }
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Arc => {
            check_arity(name, args, 6, 7)?;
            let sw_raw = if args.len() == 7 { arg_num(args, 6) } else { 2.0 };
            let a0 = arg_num(args, 3);
            let a1 = arg_num(args, 4);
            if let (Some(cx), Some(cy), Some(radius), Some(sw)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_radius(arg_num(args, 2)),
                clamp_stroke(sw_raw),
            ) {
                if a0.is_finite() && a1.is_finite() {
                    let color = RgbaColor::from_script_num(arg_num(args, 5));
                    ctx.host.push_draw_cmd(DrawCmd::Arc {
                        cx, cy, radius,
                        start_rad: a0.clamp(-100.0, 100.0) as f32,
                        end_rad: a1.clamp(-100.0, 100.0) as f32,
                        stroke_width: sw, color,
                    });
                } else {
                    ctx.host.record_invalid_draw();
                }
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Bezier => {
            check_arity(name, args, 9, 10)?;
            let sw_raw = if args.len() == 10 { arg_num(args, 9) } else { 2.0 };
            if let (Some(x0), Some(y0), Some(cx1), Some(cy1), Some(cx2), Some(cy2), Some(x1), Some(y1), Some(sw)) = (
                clamp_coord(arg_num(args, 0)), clamp_coord(arg_num(args, 1)),
                clamp_coord(arg_num(args, 2)), clamp_coord(arg_num(args, 3)),
                clamp_coord(arg_num(args, 4)), clamp_coord(arg_num(args, 5)),
                clamp_coord(arg_num(args, 6)), clamp_coord(arg_num(args, 7)),
                clamp_stroke(sw_raw),
            ) {
                let color = RgbaColor::from_script_num(arg_num(args, 8));
                ctx.host.push_draw_cmd(DrawCmd::Bezier {
                    x0, y0, cx1, cy1, cx2, cy2, x1, y1, stroke_width: sw, color,
                });
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Star => {
            check_arity(name, args, 6, 7)?;
            let pts_raw = arg_num(args, 4);
            let rot_raw = if args.len() == 7 { arg_num(args, 6) } else { 0.0 };
            if let (Some(cx), Some(cy), Some(outer_r), Some(inner_r)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_radius(arg_num(args, 2)),
                clamp_radius(arg_num(args, 3)),
            ) {
                if pts_raw.is_finite() && rot_raw.is_finite() {
                    let points = (pts_raw.round() as i32).clamp(3, 32) as u8;
                    let color = RgbaColor::from_script_num(arg_num(args, 5));
                    ctx.host.push_draw_cmd(DrawCmd::Star {
                        cx, cy, outer_r, inner_r, points,
                        rotation_rad: rot_raw.clamp(-100.0, 100.0) as f32,
                        color,
                    });
                } else {
                    ctx.host.record_invalid_draw();
                }
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Text => {
            check_arity(name, args, 5, 6)?;
            if let (Some(x), Some(y), Some(size)) = (
                clamp_coord(arg_num(args, 0)),
                clamp_coord(arg_num(args, 1)),
                clamp_font_size(arg_num(args, 3)),
            ) {
                let content = sanitize_text(&arg_str(args, 2, ctx.heap, ctx.budgets.max_str_bytes)?);
                let color = RgbaColor::from_script_num(arg_num(args, 4));
                let align = if args.len() == 6 {
                    TextAlign::parse(&arg_str(args, 5, ctx.heap, 32)?)
                } else {
                    TextAlign::Left
                };
                ctx.host.push_draw_cmd(DrawCmd::Text { x, y, content, size, color, align });
            } else {
                ctx.host.record_invalid_draw();
            }
            Ok(Val::Null)
        }
        BuiltinId::Tone => {
            check_arity(name, args, 2, 4)?;
            let freq = sanitize_f64(arg_num(args, 0)).clamp(40.0, 4000.0) as f32;
            let dur = sanitize_f64(arg_num(args, 1)).clamp(0.02, 2.0) as f32;
            let wave = if args.len() >= 3 {
                ToneWave::parse(&arg_str(args, 2, ctx.heap, 32)?)
            } else {
                ToneWave::Sine
            };
            let gain = if args.len() == 4 {
                sanitize_f64(arg_num(args, 3)).clamp(0.0, 1.0) as f32
            } else {
                0.6
            };
            ctx.host.push_audio_pulse(AudioPulse { freq_hz: freq, duration_sec: dur, wave, gain });
            Ok(Val::Null)
        }
        _ => dispatch_pixel_collision_util(builtin, args, ctx),
    }
}

fn dispatch_pixel_collision_util(
    builtin: BuiltinId,
    args: &[Val],
    ctx: &mut BuiltinCtx<'_>,
) -> Result<Val, RuntimeError> {
    let name = builtin.canonical_name();
    match builtin {
        // --- Pixel & Shader Kernels (8) ---
        BuiltinId::PixelInit => {
            check_arity(name, args, 2, 2)?;
            let w_raw = arg_num(args, 0);
            let h_raw = arg_num(args, 1);
            let (w, h) = PixelBuffer::validate_dims(
                w_raw,
                h_raw,
                ctx.budgets.max_pixel_width,
                ctx.budgets.max_pixel_height,
                ctx.budgets.max_pixel_area,
            )?;
            let cost = (((w as u64) * (h as u64)) / 16).max(64);
            ctx.watchdog.charge_and_check(cost)?;
            ctx.host.pixel_init(w as f64, h as f64)?;
            Ok(Val::Null)
        }
        BuiltinId::PixelSet => {
            check_arity(name, args, 3, 3)?;
            let buf = ctx.host.pixel_ensure_default()?;
            let color = RgbaColor::from_script_num(arg_num(args, 2));
            buf.set_rgba(arg_num(args, 0), arg_num(args, 1), color);
            Ok(Val::Null)
        }
        BuiltinId::PixelRect => {
            check_arity(name, args, 5, 5)?;
            let buf = ctx.host.pixel_ensure_default()?;
            let x = arg_num(args, 0);
            let y = arg_num(args, 1);
            let w = arg_num(args, 2);
            let h = arg_num(args, 3);
            let color = RgbaColor::from_script_num(arg_num(args, 4));
            if let Some((x0, y0, x1, y1)) = buf.clipped_rect(x, y, w, h) {
                let cw = (x1 - x0) as u64;
                let ch = (y1 - y0) as u64;
                let cost = ((cw * ch) / 16).max(8);
                ctx.watchdog.charge_and_check(cost)?;
                let wd = &*ctx.watchdog;
                buf.fill_clipped_rect_checked(x0, y0, x1, y1, color, || wd.check_wall_clock())?;
            } else {
                ctx.watchdog.charge_and_check(8)?;
            }
            Ok(Val::Null)
        }
        BuiltinId::ShaderPlasma => {
            check_arity(name, args, 3, 3)?;
            let buf = ctx.host.pixel_ensure_default()?;
            run_shader_plasma(buf, arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), ctx.watchdog)?;
            Ok(Val::Null)
        }
        BuiltinId::ShaderMandelbrot => {
            check_arity(name, args, 4, 4)?;
            let buf = ctx.host.pixel_ensure_default()?;
            run_shader_mandelbrot(
                buf,
                arg_num(args, 0),
                arg_num(args, 1),
                arg_num(args, 2),
                arg_num(args, 3),
                ctx.watchdog,
            )?;
            Ok(Val::Null)
        }
        BuiltinId::ShaderVoronoi => {
            check_arity(name, args, 3, 3)?;
            let buf = ctx.host.pixel_ensure_default()?;
            run_shader_voronoi(buf, arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), ctx.watchdog)?;
            Ok(Val::Null)
        }
        BuiltinId::ShaderStarfield => {
            check_arity(name, args, 3, 3)?;
            let buf = ctx.host.pixel_ensure_default()?;
            run_shader_starfield(buf, arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), ctx.watchdog)?;
            Ok(Val::Null)
        }
        BuiltinId::ShaderSynthwave => {
            check_arity(name, args, 3, 3)?;
            let buf = ctx.host.pixel_ensure_default()?;
            run_shader_synthwave(buf, arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), ctx.watchdog)?;
            Ok(Val::Null)
        }

        // --- Collision (4) ---
        BuiltinId::Dist => {
            check_arity(name, args, 4, 4)?;
            let dx = arg_num(args, 2) - arg_num(args, 0);
            let dy = arg_num(args, 3) - arg_num(args, 1);
            Ok(Val::Num(sanitize_f64(dx.hypot(dy))))
        }
        BuiltinId::AabbHit => {
            check_arity(name, args, 8, 8)?;
            let (ax, ay, aw, ah) = (arg_num(args, 0), arg_num(args, 1), arg_num(args, 2), arg_num(args, 3));
            let (bx, by, bw, bh) = (arg_num(args, 4), arg_num(args, 5), arg_num(args, 6), arg_num(args, 7));
            if ![ax, ay, aw, ah, bx, by, bw, bh].iter().all(|v| v.is_finite())
                || aw <= 0.0
                || ah <= 0.0
                || bw <= 0.0
                || bh <= 0.0
            {
                return Ok(Val::Bool(false));
            }
            let hit = ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by;
            Ok(Val::Bool(hit))
        }
        BuiltinId::CircleHit => {
            check_arity(name, args, 6, 6)?;
            let (x1, y1, r1) = (arg_num(args, 0), arg_num(args, 1), arg_num(args, 2));
            let (x2, y2, r2) = (arg_num(args, 3), arg_num(args, 4), arg_num(args, 5));
            if ![x1, y1, r1, x2, y2, r2].iter().all(|v| v.is_finite()) || r1 < 0.0 || r2 < 0.0 {
                return Ok(Val::Bool(false));
            }
            let dx = x2 - x1;
            let dy = y2 - y1;
            let rr = r1 + r2;
            Ok(Val::Bool(dx * dx + dy * dy <= rr * rr))
        }
        BuiltinId::CircleRectHit => {
            check_arity(name, args, 7, 7)?;
            let (cx, cy, cr) = (arg_num(args, 0), arg_num(args, 1), arg_num(args, 2));
            let (rx, ry, rw, rh) = (arg_num(args, 3), arg_num(args, 4), arg_num(args, 5), arg_num(args, 6));
            if ![cx, cy, cr, rx, ry, rw, rh].iter().all(|v| v.is_finite()) || cr < 0.0 || rw < 0.0 || rh < 0.0 {
                return Ok(Val::Bool(false));
            }
            let nx = cx.clamp(rx, rx + rw);
            let ny = cy.clamp(ry, ry + rh);
            let dx = cx - nx;
            let dy = cy - ny;
            Ok(Val::Bool(dx * dx + dy * dy <= cr * cr))
        }

        // --- Utilities (7) ---
        BuiltinId::Len => {
            check_arity(name, args, 1, 1)?;
            let n = match &args[0] {
                Val::List(id) => ctx.heap.list_len(*id),
                Val::Map(id) => ctx.heap.map_len(*id),
                Val::Str(s) => s.chars().count(),
                _ => 0,
            };
            Ok(Val::Num(n as f64))
        }
        BuiltinId::Push => {
            check_arity(name, args, 2, 2)?;
            match &args[0] {
                Val::List(id) => {
                    ctx.heap.list_push(*id, args[1].clone(), ctx.budgets)?;
                    Ok(Val::Num(ctx.heap.list_len(*id) as f64))
                }
                _ => Err(RuntimeError::TypeError {
                    detail: "push() first argument must be a list".into(),
                }),
            }
        }
        BuiltinId::Pop => {
            check_arity(name, args, 1, 1)?;
            match &args[0] {
                Val::List(id) => Ok(ctx.heap.list_pop(*id)),
                _ => Err(RuntimeError::TypeError {
                    detail: "pop() argument must be a list".into(),
                }),
            }
        }
        BuiltinId::Str => {
            check_arity(name, args, 1, 1)?;
            let formatted = ctx.heap.format_val(&args[0], ctx.budgets.max_str_bytes)?;
            Ok(Val::Str(Arc::from(formatted)))
        }
        BuiltinId::Num => {
            check_arity(name, args, 1, 1)?;
            Ok(Val::Num(sanitize_f64(args[0].as_num())))
        }
        BuiltinId::Keys => {
            check_arity(name, args, 1, 1)?;
            match &args[0] {
                Val::Map(id) => {
                    let keys = ctx.heap.map_keys(*id);
                    let list_id = ctx.heap.alloc_list(keys, ctx.budgets)?;
                    Ok(Val::List(list_id))
                }
                _ => {
                    let list_id = ctx.heap.alloc_list(Vec::new(), ctx.budgets)?;
                    Ok(Val::List(list_id))
                }
            }
        }
        BuiltinId::HasKey => {
            check_arity(name, args, 2, 2)?;
            match &args[0] {
                Val::Map(id) => {
                    let key = arg_str(args, 1, ctx.heap, ctx.budgets.max_str_bytes)?;
                    Ok(Val::Bool(ctx.heap.map_has_key(*id, &key)))
                }
                _ => Ok(Val::Bool(false)),
            }
        }
        _ => unreachable!("handled in dispatch_builtin"),
    }
}

/// Handles `receiver.method(args...)` for `Val::Canvas` and collections (`list`/`map`/`str`).
pub fn dispatch_method(
    receiver: Val,
    method: &str,
    args: &[Val],
    ctx: &mut BuiltinCtx<'_>,
) -> Result<(Val, Option<BuiltinId>), RuntimeError> {
    match receiver {
        Val::Canvas => {
            if let Some(builtin) = BuiltinId::from_name(method).filter(|b| b.is_canvas_method()) {
                let res = dispatch_builtin(builtin, args, ctx)?;
                return Ok((res, Some(builtin)));
            }
            Err(RuntimeError::UndefinedFunction {
                name: format!("canvas.{method}"),
            })
        }
        Val::List(id) => match method {
            "len" => {
                check_arity("list.len", args, 0, 0)?;
                Ok((Val::Num(ctx.heap.list_len(id) as f64), Some(BuiltinId::Len)))
            }
            "push" => {
                check_arity("list.push", args, 1, 1)?;
                ctx.heap.list_push(id, args[0].clone(), ctx.budgets)?;
                Ok((Val::Num(ctx.heap.list_len(id) as f64), Some(BuiltinId::Push)))
            }
            "pop" => {
                check_arity("list.pop", args, 0, 0)?;
                Ok((ctx.heap.list_pop(id), Some(BuiltinId::Pop)))
            }
            _ => Err(RuntimeError::UndefinedFunction {
                name: format!("list.{method}"),
            }),
        },
        Val::Map(id) => match method {
            "len" => {
                check_arity("map.len", args, 0, 0)?;
                Ok((Val::Num(ctx.heap.map_len(id) as f64), Some(BuiltinId::Len)))
            }
            "keys" => {
                check_arity("map.keys", args, 0, 0)?;
                let keys = ctx.heap.map_keys(id);
                let lid = ctx.heap.alloc_list(keys, ctx.budgets)?;
                Ok((Val::List(lid), Some(BuiltinId::Keys)))
            }
            "has_key" => {
                check_arity("map.has_key", args, 1, 1)?;
                let key = arg_str(args, 0, ctx.heap, ctx.budgets.max_str_bytes)?;
                Ok((Val::Bool(ctx.heap.map_has_key(id, &key)), Some(BuiltinId::HasKey)))
            }
            _ => Err(RuntimeError::UndefinedFunction {
                name: format!("map.{method}"),
            }),
        },
        Val::Str(s) => match method {
            "len" => {
                check_arity("str.len", args, 0, 0)?;
                Ok((Val::Num(s.chars().count() as f64), Some(BuiltinId::Len)))
            }
            _ => Err(RuntimeError::UndefinedFunction {
                name: format!("str.{method}"),
            }),
        },
        _ => Err(RuntimeError::TypeError {
            detail: format!("cannot call method `.{method}()` on this value"),
        }),
    }
}

fn ease_in_out(t: f64) -> f64 {
    let t = sanitize_f64(t).clamp(0.0, 1.0);
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) * 0.5
    }
}

fn ease_out_cubic(t: f64) -> f64 {
    let t = sanitize_f64(t).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn ease_out_bounce(t: f64) -> f64 {
    let mut x = sanitize_f64(t).clamp(0.0, 1.0);
    let n1 = 7.5625;
    let d1 = 2.75;
    if x < 1.0 / d1 {
        n1 * x * x
    } else if x < 2.0 / d1 {
        x -= 1.5 / d1;
        n1 * x * x + 0.75
    } else if x < 2.5 / d1 {
        x -= 2.25 / d1;
        n1 * x * x + 0.9375
    } else {
        x -= 2.625 / d1;
        n1 * x * x + 0.984375
    }
}

fn ease_out_elastic(t: f64) -> f64 {
    let x = sanitize_f64(t).clamp(0.0, 1.0);
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let c4 = (2.0 * PI) / 3.0;
    2.0f64.powf(-10.0 * x) * ((x * 10.0 - 0.75) * c4).sin() + 1.0
}

fn apply_named_ease(u: f64, ease: &str) -> f64 {
    match ease {
        "ease_in_out" | "in_out" | "smooth" => ease_in_out(u),
        "ease_out_cubic" | "out_cubic" | "cubic" => ease_out_cubic(u),
        "ease_out_bounce" | "bounce" => ease_out_bounce(u),
        "ease_out_elastic" | "elastic" => ease_out_elastic(u),
        _ => u.clamp(0.0, 1.0),
    }
}

fn eval_keyframe(t_raw: f64, stops_val: &Val, heap: &VmHeap) -> f64 {
    let t = sanitize_f64(t_raw);
    let Val::List(list_id) = stops_val else {
        return 0.0;
    };
    let Some(HeapObj::List(items)) = heap.get(*list_id) else {
        return 0.0;
    };
    let mut stops: Vec<(f64, f64, String)> = Vec::with_capacity(items.len().min(64));
    for item in items.iter().take(64) {
        match item {
            Val::List(pair_id) => {
                if let Some(HeapObj::List(pair)) = heap.get(*pair_id) {
                    if pair.len() >= 2 {
                        let st = sanitize_f64(pair[0].as_num());
                        let sv = sanitize_f64(pair[1].as_num());
                        let ease = match pair.get(2) {
                            Some(Val::Str(s)) => s.to_string(),
                            _ => String::new(),
                        };
                        stops.push((st, sv, ease));
                    }
                }
            }
            Val::Map(map_id) => {
                if let Some(HeapObj::Map(map)) = heap.get(*map_id) {
                    let st = map.get("t").map_or(0.0, |v| sanitize_f64(v.as_num()));
                    let sv = map.get("v").map_or(0.0, |v| sanitize_f64(v.as_num()));
                    let ease = match map.get("ease") {
                        Some(Val::Str(s)) => s.to_string(),
                        _ => String::new(),
                    };
                    stops.push((st, sv, ease));
                }
            }
            _ => {}
        }
    }
    if stops.is_empty() {
        return 0.0;
    }
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    if t <= stops[0].0 {
        return stops[0].1;
    }
    let last = stops.len() - 1;
    if t >= stops[last].0 {
        return stops[last].1;
    }
    for w in stops.windows(2) {
        let (t0, v0, ref ease0) = w[0];
        let (t1, v1, ref ease1) = w[1];
        if t >= t0 && t <= t1 {
            let span = t1 - t0;
            if span <= f64::EPSILON {
                return v1;
            }
            let raw_u = ((t - t0) / span).clamp(0.0, 1.0);
            let ease_name = if !ease1.is_empty() { ease1 } else { ease0 };
            let u = apply_named_ease(raw_u, ease_name);
            return sanitize_f64(v0 + (v1 - v0) * u);
        }
    }
    stops[last].1
}

fn hash2_u64(ix: i64, iy: i64) -> u64 {
    let mut h = (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (iy as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn hash2(x: f64, y: f64) -> f64 {
    let qx = (sanitize_f64(x) * 1024.0).floor() as i64;
    let qy = (sanitize_f64(y) * 1024.0).floor() as i64;
    let bits = hash2_u64(qx, qy) >> 11;
    (bits as f64) * (1.0 / ((1u64 << 53) as f64))
}

fn grad2(ix: i64, iy: i64, dx: f64, dy: f64) -> f64 {
    const GRADS: [(f64, f64); 8] = [
        (1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0),
        (0.70710678, 0.70710678), (-0.70710678, 0.70710678),
        (0.70710678, -0.70710678), (-0.70710678, -0.70710678),
    ];
    let idx = (hash2_u64(ix, iy) & 7) as usize;
    let (gx, gy) = GRADS[idx];
    gx * dx + gy * dy
}

fn noise2d(x_raw: f64, y_raw: f64) -> f64 {
    let x = sanitize_f64(x_raw).clamp(-1e6, 1e6);
    let y = sanitize_f64(y_raw).clamp(-1e6, 1e6);
    let ix = x.floor() as i64;
    let iy = y.floor() as i64;
    let fx = x - (ix as f64);
    let fy = y - (iy as f64);
    let u = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let v = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let n00 = grad2(ix, iy, fx, fy);
    let n10 = grad2(ix + 1, iy, fx - 1.0, fy);
    let n01 = grad2(ix, iy + 1, fx, fy - 1.0);
    let n11 = grad2(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
    let nx0 = n00 + (n10 - n00) * u;
    let nx1 = n01 + (n11 - n01) * u;
    (nx0 + (nx1 - nx0) * v).clamp(-1.0, 1.0)
}

fn fbm2d(x_raw: f64, y_raw: f64, oct_raw: f64) -> f64 {
    let octaves = if oct_raw.is_finite() {
        (oct_raw.round() as i32).clamp(1, 6) as usize
    } else {
        4
    };
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for _ in 0..octaves {
        sum += amp * noise2d(x_raw * freq, y_raw * freq);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if norm > 0.0 { (sum / norm).clamp(-1.0, 1.0) } else { 0.0 }
}

fn pack_rgba(r: f64, g: f64, b: f64, a: f64, flex_alpha: bool) -> RgbaColor {
    let ch = |v: f64| if v.is_finite() { v.round().clamp(0.0, 255.0) as u8 } else { 0 };
    let alpha = if !a.is_finite() {
        0
    } else if flex_alpha && a <= 1.0 {
        (a.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        a.round().clamp(0.0, 255.0) as u8
    };
    RgbaColor::new(ch(r), ch(g), ch(b), alpha)
}

fn pack_hsla(h_raw: f64, s_raw: f64, l_raw: f64, a_raw: f64) -> RgbaColor {
    let h = sanitize_f64(h_raw).rem_euclid(360.0);
    let norm_unit = |v: f64| {
        let v = sanitize_f64(v);
        if v > 1.0 { (v / 100.0).clamp(0.0, 1.0) } else { v.clamp(0.0, 1.0) }
    };
    let s = norm_unit(s_raw);
    let l = norm_unit(l_raw);
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
    pack_rgba((r1 + m) * 255.0, (g1 + m) * 255.0, (b1 + m) * 255.0, a_raw, true)
}

fn extract_points(val: &Val, heap: &VmHeap) -> Option<Vec<[f32; 2]>> {
    let Val::List(list_id) = val else {
        return Some(Vec::new());
    };
    let Some(HeapObj::List(items)) = heap.get(*list_id) else {
        return Some(Vec::new());
    };
    let mut out = Vec::with_capacity(items.len().min(MAX_POLY_POINTS));
    for item in items.iter().take(MAX_POLY_POINTS) {
        let (x_raw, y_raw) = match item {
            Val::List(pid) => match heap.get(*pid) {
                Some(HeapObj::List(pair)) if pair.len() >= 2 => (pair[0].as_num(), pair[1].as_num()),
                _ => continue,
            },
            Val::Map(mid) => match heap.get(*mid) {
                Some(HeapObj::Map(map)) => (
                    map.get("x").map_or(0.0, Val::as_num),
                    map.get("y").map_or(0.0, Val::as_num),
                ),
                _ => continue,
            },
            _ => continue,
        };
        let x = clamp_coord(x_raw)?;
        let y = clamp_coord(y_raw)?;
        out.push([x, y]);
    }
    Some(out)
}
