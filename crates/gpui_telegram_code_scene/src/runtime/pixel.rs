//! Bounded BGRA8 pixel buffer for native shader kernels and `pixel_*` builtins.
//!
//! Pixel bytes are stored in direct `[B, G, R, A]` row-major order so GPUI
//! `RenderImage` can consume `PixelFrame::bgra` with zero UI-thread channel swap.

use crate::model::{RgbaColor, RuntimeError};

pub const MAX_PIXEL_WIDTH: u32 = 240;
pub const MAX_PIXEL_HEIGHT: u32 = 320;
pub const MAX_PIXEL_AREA: usize = (MAX_PIXEL_WIDTH as usize) * (MAX_PIXEL_HEIGHT as usize);
pub const DEFAULT_PIXEL_WIDTH: u32 = 120;
pub const DEFAULT_PIXEL_HEIGHT: u32 = 160;

/// Immutable BGRA8 raster snapshot attached to `FrameOutput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub revision: u64,
}

impl PixelFrame {
    pub fn has_visible_pixel(&self) -> bool {
        self.bgra.chunks_exact(4).any(|px| px[3] > 0)
    }
}

/// Mutable BGRA8 pixel buffer owned by `VmInstance`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelBuffer {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    dirty: bool,
    revision: u64,
}

impl PixelBuffer {
    pub fn validate_dims(
        w_raw: f64,
        h_raw: f64,
        max_w: u32,
        max_h: u32,
        max_area: usize,
    ) -> Result<(u32, u32), RuntimeError> {
        if !w_raw.is_finite() || !h_raw.is_finite() {
            return Err(RuntimeError::PixelBudgetExceeded {
                detail: "pixel_init width/height must be finite".into(),
            });
        }
        let w_floor = w_raw.floor();
        let h_floor = h_raw.floor();
        if w_floor < 1.0 || h_floor < 1.0 || w_floor > (max_w as f64) || h_floor > (max_h as f64) {
            return Err(RuntimeError::PixelBudgetExceeded {
                detail: format!(
                    "pixel_init({w_raw}, {h_raw}) exceeds bounds 1..={max_w} x 1..={max_h}"
                ),
            });
        }
        let width = w_floor as u32;
        let height = h_floor as u32;
        let area = (width as usize).saturating_mul(height as usize);
        if area == 0 || area > max_area {
            return Err(RuntimeError::PixelBudgetExceeded {
                detail: format!("pixel area {width}x{height}={area} exceeds max {max_area}"),
            });
        }
        Ok((width, height))
    }

    pub fn new_checked(
        w_raw: f64,
        h_raw: f64,
        max_w: u32,
        max_h: u32,
        max_area: usize,
        prev_revision: u64,
    ) -> Result<Self, RuntimeError> {
        let (width, height) = Self::validate_dims(w_raw, h_raw, max_w, max_h, max_area)?;
        let area = (width as usize).saturating_mul(height as usize);
        Ok(Self {
            width,
            height,
            bgra: vec![0u8; area * 4],
            dirty: true,
            revision: prev_revision.wrapping_add(1).max(1),
        })
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1).max(1);
    }

    /// Rebases `revision` strictly above both `self.revision` and `prev` and marks the buffer dirty
    /// so continuous playback across stateful Video loop wraps never rolls back or collides revisions.
    pub fn rebase_revision_above(&mut self, prev: u64) {
        self.revision = self.revision.max(prev).saturating_add(1).max(1);
        self.dirty = true;
    }

    pub fn bgra_bytes(&self) -> &[u8] {
        &self.bgra
    }

    pub fn bgra_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bgra
    }

    /// Writes a single pixel at integer `(x, y)` in BGRA8 order, clipping safely.
    pub fn set_rgba(&mut self, x: f64, y: f64, color: RgbaColor) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let xi = x.floor() as i64;
        let yi = y.floor() as i64;
        if xi < 0 || yi < 0 || xi >= self.width as i64 || yi >= self.height as i64 {
            return;
        }
        let idx = ((yi as usize) * (self.width as usize) + (xi as usize)) * 4;
        let px = color.to_bgra_bytes();
        self.bgra[idx..idx + 4].copy_from_slice(&px);
        self.mark_dirty();
    }

    /// Computes the clipped inclusive-exclusive pixel bounds `(x0, y0, x1, y1)` for a rectangle,
    /// returning `None` if non-finite, non-positive, or completely outside the buffer.
    pub fn clipped_rect(&self, x: f64, y: f64, w: f64, h: f64) -> Option<(usize, usize, usize, usize)> {
        if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() {
            return None;
        }
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let x0 = (x.floor() as i64).clamp(0, self.width as i64) as usize;
        let y0 = (y.floor() as i64).clamp(0, self.height as i64) as usize;
        let x1 = ((x + w).ceil() as i64).clamp(0, self.width as i64) as usize;
        let y1 = ((y + h).ceil() as i64).clamp(0, self.height as i64) as usize;
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        Some((x0, y0, x1, y1))
    }

    /// Fills a pre-clipped rectangle in BGRA8 order, invoking `check_row` every 16 rows
    /// so large fills abort cleanly on wall-clock timeout.
    pub fn fill_clipped_rect_checked(
        &mut self,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        color: RgbaColor,
        mut check_row: impl FnMut() -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let px = color.to_bgra_bytes();
        let stride = self.width as usize;
        for row in y0..y1 {
            if ((row - y0) & 15) == 15 {
                check_row()?;
            }
            let start = (row * stride + x0) * 4;
            let end = (row * stride + x1) * 4;
            for dst in self.bgra[start..end].chunks_exact_mut(4) {
                dst.copy_from_slice(&px);
            }
        }
        self.mark_dirty();
        Ok(())
    }

    /// Fills a clipped rectangle in BGRA8 order.
    pub fn fill_rect_rgba(&mut self, x: f64, y: f64, w: f64, h: f64, color: RgbaColor) {
        if let Some((x0, y0, x1, y1)) = self.clipped_rect(x, y, w, h) {
            let _ = self.fill_clipped_rect_checked(x0, y0, x1, y1, color, || Ok(()));
        }
    }

    pub fn has_visible_pixel(&self) -> bool {
        self.bgra.chunks_exact(4).any(|px| px[3] > 0)
    }

    /// Returns a self-contained snapshot without clearing `dirty`.
    pub fn snapshot(&self) -> PixelFrame {
        PixelFrame {
            width: self.width,
            height: self.height,
            bgra: self.bgra.clone(),
            revision: self.revision,
        }
    }

    /// Returns a snapshot only if mutated since the previous `take_if_dirty`, resetting `dirty`.
    pub fn take_if_dirty(&mut self) -> Option<PixelFrame> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(self.snapshot())
    }
}
