//! Native Rust BGRA8 pixel shader kernels for `CodeSceneScript` (`cs1`).
//!
//! All 5 shaders write directly into `PixelBuffer` in BGRA8 order, check the
//! wall-clock watchdog every 16 rows, and charge virtual VM ops proportional
//! to pixel work so shader spam cannot bypass `VmBudgets`.

use std::f64::consts::TAU;
use std::time::{Duration, Instant};

use crate::model::{RgbaColor, RuntimeError};
use crate::runtime::pixel::PixelBuffer;

/// Execution guard passed into shader loops for virtual-op charging & row-level timeout.
pub struct ShaderWatchdog<'a> {
    pub ops_used: &'a mut u64,
    pub op_limit: u64,
    pub started_at: Instant,
    pub wall_limit: Duration,
}

impl<'a> ShaderWatchdog<'a> {
    pub fn charge_and_check(&mut self, ops: u64) -> Result<(), RuntimeError> {
        *self.ops_used = self.ops_used.saturating_add(ops);
        if *self.ops_used > self.op_limit {
            return Err(RuntimeError::InstructionBudgetExceeded {
                used: *self.ops_used,
                limit: self.op_limit,
            });
        }
        let elapsed = self.started_at.elapsed();
        if elapsed >= self.wall_limit {
            return Err(RuntimeError::FrameTimeout {
                elapsed_ms: elapsed.as_millis() as u64,
                limit_ms: self.wall_limit.as_millis() as u64,
            });
        }
        Ok(())
    }

    pub fn check_wall_clock(&self) -> Result<(), RuntimeError> {
        let elapsed = self.started_at.elapsed();
        if elapsed >= self.wall_limit {
            return Err(RuntimeError::FrameTimeout {
                elapsed_ms: elapsed.as_millis() as u64,
                limit_ms: self.wall_limit.as_millis() as u64,
            });
        }
        Ok(())
    }
}

fn palette_color(v: f64, pal: u32) -> [u8; 4] {
    let p = v * TAU;
    let (r, g, b) = match pal % 4 {
        0 => (
            // Aurora Cyan / Violet
            (0.45 + 0.45 * (p + 0.0).cos()).clamp(0.0, 1.0),
            (0.55 + 0.45 * (p + 1.8).cos()).clamp(0.0, 1.0),
            (0.65 + 0.35 * (p + 3.4).cos()).clamp(0.0, 1.0),
        ),
        1 => (
            // Cyber Magenta / Electric Blue
            (0.62 + 0.38 * (p + 0.3).cos()).clamp(0.0, 1.0),
            (0.25 + 0.35 * (p + 2.4).cos()).clamp(0.0, 1.0),
            (0.65 + 0.35 * (p + 4.4).cos()).clamp(0.0, 1.0),
        ),
        2 => (
            // Solar Gold / Crimson
            (0.65 + 0.35 * (p + 0.0).cos()).clamp(0.0, 1.0),
            (0.42 + 0.42 * (p + 1.1).cos()).clamp(0.0, 1.0),
            (0.18 + 0.25 * (p + 2.7).cos()).clamp(0.0, 1.0),
        ),
        _ => (
            // Bioluminescent Emerald / Teal
            (0.18 + 0.32 * (p + 2.2).cos()).clamp(0.0, 1.0),
            (0.60 + 0.40 * (p + 0.4).cos()).clamp(0.0, 1.0),
            (0.55 + 0.40 * (p + 1.6).cos()).clamp(0.0, 1.0),
        ),
    };
    RgbaColor::new(
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8,
        255,
    )
    .to_bgra_bytes()
}

fn build_palette_lut(pal: u32) -> [[u8; 4]; 256] {
    let mut lut = [[0u8; 4]; 256];
    let mut i = 0usize;
    while i < 256 {
        lut[i] = palette_color((i as f64) * (1.0 / 256.0), pal);
        i += 1;
    }
    lut
}

pub fn run_shader_plasma(
    buf: &mut PixelBuffer,
    t: f64,
    scale: f64,
    pal: f64,
    watchdog: &mut ShaderWatchdog<'_>,
) -> Result<(), RuntimeError> {
    let w = buf.width() as usize;
    let h = buf.height() as usize;
    let cost = ((w * h) / 16).max(64) as u64;
    watchdog.charge_and_check(cost)?;

    let t = if t.is_finite() { t } else { 0.0 };
    let scale = if scale.is_finite() {
        scale.clamp(0.5, 16.0)
    } else {
        4.0
    };
    let pal_idx = if pal.is_finite() && pal >= 0.0 {
        (pal as u32) % 4
    } else {
        0
    };
    let inv_w = scale / (w.max(1) as f64);
    let inv_h = scale / (h.max(1) as f64);
    let lut = build_palette_lut(pal_idx);
    let t_off = (t * 0.08).rem_euclid(1.0);
    let t_12 = t * 1.2;
    let t_09 = t * 0.9;
    let t_07 = t * 0.7;
    let t_16 = t * 1.6;
    let half_scale = 0.5 * scale;
    let bgra = buf.bgra_bytes_mut();

    for y in 0..h {
        if (y & 31) == 31 {
            watchdog.check_wall_clock()?;
        }
        let fy = (y as f64) * inv_h;
        let wy = (fy * 1.15 - t_09).sin();
        let dy = fy - half_scale;
        let dy2 = dy * dy + 0.001;
        let row_base = y * w * 4;
        let mut x = 0usize;
        while x < w {
            let fx = (x as f64) * inv_w;
            let dx = fx - half_scale;
            let v1 = (fx + t_12).sin();
            let v2 = ((fx + fy) * 0.8 + t_07).sin();
            let v3 = ((dx * dx + dy2).sqrt() * 2.2 - t_16).sin();
            // v is strictly in [0.0, 2.0) because (v1+wy+v2+v3)*0.125+0.5 in [0,1] and t_off in [0,1)
            let v = (v1 + wy + v2 + v3) * 0.125 + 0.5 + t_off;
            let lut_idx = ((v * 256.0) as usize) & 255;
            let px = lut[lut_idx];
            let idx = row_base + x * 4;
            bgra[idx] = px[0];
            bgra[idx + 1] = px[1];
            bgra[idx + 2] = px[2];
            bgra[idx + 3] = 255;
            x += 1;
        }
    }
    buf.mark_dirty();
    Ok(())
}

pub fn run_shader_mandelbrot(
    buf: &mut PixelBuffer,
    cx: f64,
    cy: f64,
    zoom: f64,
    iter: f64,
    watchdog: &mut ShaderWatchdog<'_>,
) -> Result<(), RuntimeError> {
    let w = buf.width() as usize;
    let h = buf.height() as usize;
    let max_iter = if iter.is_finite() {
        (iter.round() as i32).clamp(4, 64) as u32
    } else {
        28
    };
    let cost = (((w * h) / 16) * ((max_iter as usize).max(8) / 8)).max(128) as u64;
    watchdog.charge_and_check(cost)?;

    let cx = if cx.is_finite() { cx.clamp(-4.0, 4.0) } else { -0.7436 };
    let cy = if cy.is_finite() { cy.clamp(-4.0, 4.0) } else { 0.1318 };
    let zoom = if zoom.is_finite() { zoom.clamp(0.1, 1e6) } else { 1.0 };
    let aspect = (w as f64) / (h.max(1) as f64);
    let span_y = 2.6 / zoom;
    let span_x = span_y * aspect;
    let inv_w = 1.0 / (w.max(1) as f64);
    let inv_h = 1.0 / (h.max(1) as f64);
    let lut = build_palette_lut(1);
    let lut_scale = (2.4 * 256.0) / (max_iter as f64);
    let bgra = buf.bgra_bytes_mut();

    let mut y = 0usize;
    while y < h {
        if (y & 31) == 31 {
            watchdog.check_wall_clock()?;
        }
        let c_im = cy + ((y as f64) * inv_h - 0.5) * span_y;
        let c_im2 = c_im * c_im;
        let row_base = y * w * 4;
        let mut x = 0usize;
        while x < w {
            let c_re = cx + ((x as f64) * inv_w - 0.5) * span_x;
            let xm = c_re - 0.25;
            let q = xm * xm + c_im2;
            let xp = c_re + 1.0;
            let px = if q * (q + xm) <= 0.25 * c_im2 || xp * xp + c_im2 <= 0.0625 {
                [24u8, 10, 14, 255]
            } else {
                let mut z_re = 0.0f64;
                let mut z_im = 0.0f64;
                let mut it = 0u32;
                while it < max_iter {
                    let zr2 = z_re * z_re;
                    let zi2 = z_im * z_im;
                    if zr2 + zi2 > 4.0 {
                        break;
                    }
                    z_im = 2.0 * z_re * z_im + c_im;
                    z_re = zr2 - zi2 + c_re;
                    it += 1;
                }
                if it >= max_iter {
                    [20u8, 8, 12, 255]
                } else {
                    let lut_idx = (((it as f64) * lut_scale + 38.0) as usize) & 255;
                    lut[lut_idx]
                }
            };
            let idx = row_base + x * 4;
            bgra[idx] = px[0];
            bgra[idx + 1] = px[1];
            bgra[idx + 2] = px[2];
            bgra[idx + 3] = 255;
            x += 1;
        }
        y += 1;
    }
    buf.mark_dirty();
    Ok(())
}

pub fn run_shader_voronoi(
    buf: &mut PixelBuffer,
    t: f64,
    cells: f64,
    pal: f64,
    watchdog: &mut ShaderWatchdog<'_>,
) -> Result<(), RuntimeError> {
    let w = buf.width() as usize;
    let h = buf.height() as usize;
    let cell_count = if cells.is_finite() {
        (cells.round() as i32).clamp(3, 24) as usize
    } else {
        12
    };
    let cost = (((w * h) / 16) * (cell_count / 4).max(1)).max(96) as u64;
    watchdog.charge_and_check(cost)?;

    let t = if t.is_finite() { t } else { 0.0 };
    let pal_idx = if pal.is_finite() && pal >= 0.0 {
        (pal as u32) % 4
    } else {
        3
    };

    // Precompute deterministic orbiting Voronoi sites in aspect-corrected f32 space
    let aspect = (w as f32) / (h.max(1) as f32);
    let mut sx_arr = [0.0f32; 24];
    let mut sy_arr = [0.0f32; 24];
    let mut i = 0usize;
    while i < cell_count {
        let fi = (i + 1) as f64;
        let phase_x = fi * 2.3999632 + t * (0.42 + 0.05 * (fi % 5.0));
        let phase_y = fi * 1.6180339 + t * (0.37 + 0.04 * (fi % 4.0));
        sx_arr[i] = (0.5 + 0.42 * phase_x.sin() * (0.65 + 0.35 * (fi * 0.7).cos())) as f32 * aspect;
        sy_arr[i] = (0.5 + 0.42 * phase_y.cos() * (0.65 + 0.35 * (fi * 1.1).sin())) as f32;
        i += 1;
    }

    let lut = build_palette_lut(pal_idx);
    let inv_w = aspect / (w.max(1) as f32);
    let inv_h = 1.0f32 / (h.max(1) as f32);
    let inv_cells = 1.0f32 / (cell_count as f32);
    let t_shift = (t * 0.05).rem_euclid(1.0) as f32;
    let bgra = buf.bgra_bytes_mut();

    let mut y = 0usize;
    while y < h {
        if (y & 31) == 31 {
            watchdog.check_wall_clock()?;
        }
        let ny = (y as f32) * inv_h;
        let row_base = y * w * 4;
        let mut x = 0usize;
        while x < w {
            let nx = (x as f32) * inv_w;
            let mut d1 = f32::MAX;
            let mut d2 = f32::MAX;
            let mut closest = 0usize;
            let mut k = 0usize;
            while k < cell_count {
                let dx = nx - sx_arr[k];
                let dy = ny - sy_arr[k];
                let dist2 = dx * dx + dy * dy;
                if dist2 < d1 {
                    d2 = d1;
                    d1 = dist2;
                    closest = k;
                } else if dist2 < d2 {
                    d2 = dist2;
                }
                k += 1;
            }
            let edge = ((d2 - d1) * 14.0).clamp(0.0, 1.0);
            let inv_e = 1.0 - edge;
            let glow = inv_e * inv_e * inv_e;
            let d1_c = if d1 < 4.0 { d1 } else { 4.0 };
            // base_v is strictly in [0.0, 10.8) because t_shift was wrapped via rem_euclid(1.0)
            let base_v = (closest as f32) * inv_cells + d1_c * 2.2 + t_shift;
            let px = lut[((base_v * 256.0) as usize) & 255];
            let boost = (glow * 140.0) as u16;
            let idx = row_base + x * 4;
            bgra[idx] = (px[0] as u16 + boost).min(255) as u8;
            bgra[idx + 1] = (px[1] as u16 + boost).min(255) as u8;
            bgra[idx + 2] = (px[2] as u16 + (boost * 3 / 4)).min(255) as u8;
            bgra[idx + 3] = 255;
            x += 1;
        }
        y += 1;
    }
    buf.mark_dirty();
    Ok(())
}

pub fn run_shader_starfield(
    buf: &mut PixelBuffer,
    t: f64,
    speed: f64,
    density: f64,
    watchdog: &mut ShaderWatchdog<'_>,
) -> Result<(), RuntimeError> {
    let w = buf.width() as usize;
    let h = buf.height() as usize;
    let count = if density.is_finite() {
        (density.round() as i32).clamp(16, 256) as usize
    } else {
        120
    };
    let cost = (((w * h) / 32) + count * 2).max(64) as u64;
    watchdog.charge_and_check(cost)?;

    let t = if t.is_finite() { t } else { 0.0 };
    let speed = if speed.is_finite() {
        speed.clamp(0.05, 8.0)
    } else {
        1.0
    };
    let bgra = buf.bgra_bytes_mut();

    // Deep-space nebula gradient backdrop
    for y in 0..h {
        if (y & 31) == 31 {
            watchdog.check_wall_clock()?;
        }
        let fy = (y as f64) / (h.max(1) as f64);
        let r = (6.0 + 18.0 * (1.0 - fy) * (0.5 + 0.5 * (t * 0.4).sin())) as u8;
        let g = (8.0 + 14.0 * fy) as u8;
        let b = (22.0 + 38.0 * (0.5 + 0.5 * (fy * 3.14).sin())) as u8;
        let row_base = y * w * 4;
        for x in 0..w {
            let idx = row_base + x * 4;
            bgra[idx] = b;
            bgra[idx + 1] = g;
            bgra[idx + 2] = r;
            bgra[idx + 3] = 255;
        }
    }

    let cx = (w as f64) * 0.5;
    let cy = (h as f64) * 0.5;
    let scale = (w.min(h) as f64) * 0.55;

    for i in 0..count {
        let mut hval = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xBF58_476D_1CE4_E5B9;
        hval = (hval ^ (hval >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let sx = (((hval & 0xFFFF) as f64) / 65535.0) * 2.0 - 1.0;
        let sy = ((((hval >> 16) & 0xFFFF) as f64) / 65535.0) * 2.0 - 1.0;
        let z0 = ((((hval >> 32) & 0xFFFF) as f64) / 65535.0).max(0.05);
        let z = (z0 - (t * speed * 0.45)).rem_euclid(1.0).max(0.04);
        let px = cx + (sx / z) * scale;
        let py = cy + (sy / z) * scale;
        let bright = ((1.0 - z).clamp(0.15, 1.0) * 255.0) as u8;
        let tint_b = bright;
        let tint_g = ((bright as u16 * (215 + (i % 40) as u16)) / 255).min(255) as u8;
        let tint_r = ((bright as u16 * (190 + ((i * 7) % 65) as u16)) / 255).min(255) as u8;
        let streak = ((speed * (1.0 - z) * 4.0) as i32).clamp(1, 6);
        for s in 0..streak {
            let frac = (s as f64) * 0.025;
            let z_prev = (z + frac).min(1.0);
            let x_i = (cx + (sx / z_prev) * scale).round() as i32;
            let y_i = (cy + (sy / z_prev) * scale).round() as i32;
            if x_i >= 0 && y_i >= 0 && (x_i as usize) < w && (y_i as usize) < h {
                let fade = (streak - s) as u16;
                let idx = ((y_i as usize) * w + (x_i as usize)) * 4;
                bgra[idx] = bgra[idx].max(((tint_b as u16 * fade) / streak as u16) as u8);
                bgra[idx + 1] = bgra[idx + 1].max(((tint_g as u16 * fade) / streak as u16) as u8);
                bgra[idx + 2] = bgra[idx + 2].max(((tint_r as u16 * fade) / streak as u16) as u8);
                bgra[idx + 3] = 255;
            }
        }
        let xi = px.round() as i32;
        let yi = py.round() as i32;
        if z < 0.35 && xi >= 0 && yi >= 0 && (xi as usize + 1) < w && (yi as usize + 1) < h {
            let idx = ((yi as usize) * w + (xi as usize + 1)) * 4;
            bgra[idx] = tint_b;
            bgra[idx + 1] = tint_g;
            bgra[idx + 2] = tint_r;
            bgra[idx + 3] = 255;
        }
    }
    buf.mark_dirty();
    Ok(())
}

pub fn run_shader_synthwave(
    buf: &mut PixelBuffer,
    t: f64,
    speed: f64,
    hue: f64,
    watchdog: &mut ShaderWatchdog<'_>,
) -> Result<(), RuntimeError> {
    let w = buf.width() as usize;
    let h = buf.height() as usize;
    let cost = ((w * h) / 16).max(64) as u64;
    watchdog.charge_and_check(cost)?;

    let t = if t.is_finite() { t } else { 0.0 };
    let speed = if speed.is_finite() {
        speed.clamp(0.1, 10.0)
    } else {
        1.5
    };
    let hue_shift = if hue.is_finite() {
        hue.rem_euclid(360.0) / 360.0
    } else {
        0.0
    };
    let horizon = ((h as f32) * 0.56) as usize;
    let sun_cx = (w as f32) * 0.5;
    let sun_cy = (h as f32) * 0.37;
    let sun_r = (w.min(h) as f32) * 0.25;
    let sun_r2 = sun_r * sun_r;
    let inv_w = 2.0f32 / (w.max(1) as f32);
    let hue_cos = (0.5 + 0.5 * (hue_shift * TAU).cos()) as f32;
    let lut = build_palette_lut(1);
    let t_f32 = t.rem_euclid(10_000.0) as f32;
    let spd_f32 = speed as f32;
    let bgra = buf.bgra_bytes_mut();

    let mut y = 0usize;
    while y < h {
        if (y & 31) == 31 {
            watchdog.check_wall_clock()?;
        }
        let row_base = y * w * 4;
        if y < horizon {
            let sky_t = (y as f32) / (horizon.max(1) as f32);
            let sky_t2 = sky_t * sky_t;
            let sky_r = (20.0 + 180.0 * sky_t * sky_t.sqrt()) as u8;
            let sky_g = (10.0 + 48.0 * sky_t2) as u8;
            let sky_b = (52.0 + 115.0 * (1.0 - (sky_t - 0.5).abs() * 1.4).max(0.0)) as u8;
            let dy = (y as f32) - sun_cy;
            let dy2 = dy * dy;
            let sy = (((y as f32) - (sun_cy - sun_r)) / (2.0 * sun_r)).clamp(0.0, 1.0);
            let stripe_raw = sy * 14.0 - t_f32 * 0.8;
            let stripe_frac = stripe_raw - stripe_raw.floor();
            let sun_row_lit = !(sy > 0.45 && stripe_frac > 0.72);
            let sun_g = (228.0 - 158.0 * sy).clamp(40.0, 255.0) as u8;
            let sun_b = (96.0 + 110.0 * sy * hue_cos).clamp(40.0, 255.0) as u8;
            let mut x = 0usize;
            while x < w {
                let dx = (x as f32) - sun_cx;
                let idx = row_base + x * 4;
                if sun_row_lit && dx * dx + dy2 <= sun_r2 {
                    bgra[idx] = sun_b;
                    bgra[idx + 1] = sun_g;
                    bgra[idx + 2] = 255;
                } else {
                    bgra[idx] = sky_b;
                    bgra[idx + 1] = sky_g;
                    bgra[idx + 2] = sky_r;
                }
                bgra[idx + 3] = 255;
                x += 1;
            }
        } else {
            let depth = ((y - horizon) as f32 + 1.0) / ((h - horizon).max(1) as f32);
            let z = 1.0 / (depth + 0.04);
            let z_scroll = z + t_f32 * spd_f32 * 3.0;
            let z_line = ((z_scroll - z_scroll.floor()) - 0.5).abs();
            let hz_u = 1.0 - (z_line / (0.045 + depth * 0.04)).clamp(0.0, 1.0);
            let hz_glow = hz_u * hz_u;
            let lut_idx = ((((hue_shift as f32) + depth * 0.3).rem_euclid(1.0) * 256.0) as usize) & 255;
            let glow_col = lut[lut_idx];
            let gc_b = glow_col[0] as f32;
            let gc_g = glow_col[1] as f32;
            let gc_r = glow_col[2] as f32;
            let base_r = 14.0 + 28.0 * (1.0 - depth);
            let base_g = 6.0 + 10.0 * (1.0 - depth);
            let base_b = 34.0 + 48.0 * (1.0 - depth);
            let inv_v_width = 1.0 / (0.04 + depth * 0.035);
            let z_step = z * 0.65;
            let mut x = 0usize;
            while x < w {
                let nx = (x as f32) * inv_w - 1.0;
                let gx_raw = nx * z_step;
                let gx = gx_raw - gx_raw.floor();
                let x_line = (gx - 0.5).abs();
                let v_u = 1.0 - (x_line * inv_v_width).clamp(0.0, 1.0);
                let v_glow = v_u * v_u;
                let grid = if hz_glow > v_glow { hz_glow } else { v_glow };
                let idx = row_base + x * 4;
                bgra[idx] = (base_b + grid * gc_b).min(255.0) as u8;
                bgra[idx + 1] = (base_g + grid * gc_g).min(255.0) as u8;
                bgra[idx + 2] = (base_r + grid * gc_r).min(255.0) as u8;
                bgra[idx + 3] = 255;
                x += 1;
            }
        }
        y += 1;
    }
    buf.mark_dirty();
    Ok(())
}
