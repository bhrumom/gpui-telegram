//! Douyin vertical snap-scroll physics, wheel inertia debounce, and double-tap heart bursts (`CS-F13`).

use std::time::{Duration, Instant};

use gpui::{ScrollDelta, px};

/// Direction of a snapped vertical feed transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeSnapDirection {
    Next,
    Prev,
}

/// Cooldown after a wheel-triggered snap so trackpad inertia advances strictly 1 card.
const WHEEL_SNAP_COOLDOWN: Duration = Duration::from_millis(280);
/// Idle reset window for accumulated trackpad pixels.
const WHEEL_ACCUM_RESET: Duration = Duration::from_millis(180);
/// Duration of the ease-out cubic vertical slide-in / snap-back animation.
const SNAP_ANIM_DURATION_MS: f32 = 220.0;
/// Lifetime of a cursor-anchored double-tap heart burst animation.
pub const HEART_BURST_DURATION_MS: f32 = 720.0;

/// Smooth ease-out cubic curve (`1 - (1 - t)^3`).
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let clamped = t.clamp(0.0, 1.0);
    let inv = 1.0 - clamped;
    1.0 - inv * inv * inv
}

/// Stateful controller for Douyin-style wheel inertia, vertical drag-to-swipe, and snap transitions.
#[derive(Debug, Clone)]
pub struct SwipeGestureController {
    pub dragging: bool,
    pub drag_start_y: f32,
    pub drag_offset_y: f32,
    wheel_accum_y: f32,
    last_wheel_at: Option<Instant>,
    wheel_cooldown_until: Option<Instant>,
    snap_anim_from_y: f32,
    snap_anim_started_at: Option<Instant>,
}

impl Default for SwipeGestureController {
    fn default() -> Self {
        Self::new()
    }
}

impl SwipeGestureController {
    pub fn new() -> Self {
        Self {
            dragging: false,
            drag_start_y: 0.0,
            drag_offset_y: 0.0,
            wheel_accum_y: 0.0,
            last_wheel_at: None,
            wheel_cooldown_until: None,
            snap_anim_from_y: 0.0,
            snap_anim_started_at: None,
        }
    }

    /// Processes a GPUI `ScrollWheelEvent` delta (`Pixels` or `Lines`) with inertia debounce.
    /// Returns `Some(SwipeSnapDirection)` when the snap threshold is crossed.
    pub fn on_scroll_wheel(
        &mut self,
        delta: ScrollDelta,
        viewport_h: f32,
    ) -> Option<SwipeSnapDirection> {
        self.on_scroll_wheel_at(delta, viewport_h, Instant::now())
    }

    pub fn on_scroll_wheel_at(
        &mut self,
        delta: ScrollDelta,
        viewport_h: f32,
        now: Instant,
    ) -> Option<SwipeSnapDirection> {
        if self
            .wheel_cooldown_until
            .is_some_and(|until| now < until)
        {
            return None;
        }
        if self
            .last_wheel_at
            .is_some_and(|prev| now.saturating_duration_since(prev) > WHEEL_ACCUM_RESET)
        {
            self.wheel_accum_y = 0.0;
        }
        self.last_wheel_at = Some(now);

        let dy_px = match delta {
            ScrollDelta::Pixels(pt) => f32::from(pt.y),
            ScrollDelta::Lines(pt) => pt.y * 42.0,
        };
        if !dy_px.is_finite() || dy_px.abs() < 0.05 {
            return None;
        }
        self.wheel_accum_y += dy_px;

        let threshold = (viewport_h * 0.09).clamp(38.0, 76.0);
        if self.wheel_accum_y <= -threshold {
            self.wheel_accum_y = 0.0;
            self.wheel_cooldown_until = Some(now + WHEEL_SNAP_COOLDOWN);
            Some(SwipeSnapDirection::Next)
        } else if self.wheel_accum_y >= threshold {
            self.wheel_accum_y = 0.0;
            self.wheel_cooldown_until = Some(now + WHEEL_SNAP_COOLDOWN);
            Some(SwipeSnapDirection::Prev)
        } else {
            None
        }
    }

    pub fn on_drag_start(&mut self, y_px: f32) {
        if !y_px.is_finite() {
            return;
        }
        self.dragging = true;
        self.drag_start_y = y_px;
        self.drag_offset_y = 0.0;
        self.snap_anim_started_at = None;
    }

    pub fn on_drag_move(&mut self, y_px: f32) -> f32 {
        if !self.dragging || !y_px.is_finite() {
            return 0.0;
        }
        // Subtle rubber-band resistance beyond 220px
        let raw = y_px - self.drag_start_y;
        let clamped = if raw.abs() <= 220.0 {
            raw
        } else {
            raw.signum() * (220.0 + (raw.abs() - 220.0) * 0.35)
        };
        self.drag_offset_y = clamped;
        clamped
    }

    /// Ends a vertical drag, returning `Some(direction)` if the snap threshold was crossed,
    /// or smoothly animating back to `0.0` otherwise.
    pub fn on_drag_end(&mut self, viewport_h: f32) -> Option<SwipeSnapDirection> {
        if !self.dragging {
            return None;
        }
        self.dragging = false;
        let offset = self.drag_offset_y;
        self.drag_offset_y = 0.0;
        let threshold = (viewport_h * 0.15).clamp(56.0, 128.0);
        if offset <= -threshold {
            Some(SwipeSnapDirection::Next)
        } else if offset >= threshold {
            Some(SwipeSnapDirection::Prev)
        } else {
            if offset.abs() > 1.0 {
                self.snap_anim_from_y = offset;
                self.snap_anim_started_at = Some(Instant::now());
            }
            None
        }
    }

    /// Triggers a smooth ease-out cubic vertical slide-in transition when a new card becomes active.
    pub fn trigger_card_transition(&mut self, dir: SwipeSnapDirection, viewport_h: f32) {
        self.dragging = false;
        self.drag_offset_y = 0.0;
        let distance = (viewport_h * 0.28).clamp(90.0, 240.0);
        self.snap_anim_from_y = match dir {
            SwipeSnapDirection::Next => distance,
            SwipeSnapDirection::Prev => -distance,
        };
        self.snap_anim_started_at = Some(Instant::now());
    }

    /// Returns `(visual_offset_y_px, is_animating)`.
    pub fn current_visual_offset_y(&mut self) -> (f32, bool) {
        if self.dragging {
            return (self.drag_offset_y, true);
        }
        let Some(started) = self.snap_anim_started_at else {
            return (0.0, false);
        };
        let elapsed_ms = Instant::now().saturating_duration_since(started).as_secs_f32() * 1000.0;
        if elapsed_ms >= SNAP_ANIM_DURATION_MS {
            self.snap_anim_started_at = None;
            self.snap_anim_from_y = 0.0;
            return (0.0, false);
        }
        let progress = ease_out_cubic(elapsed_ms / SNAP_ANIM_DURATION_MS);
        let offset = self.snap_anim_from_y * (1.0 - progress);
        (offset, true)
    }

    pub fn drag_travel_px(&self) -> f32 {
        self.drag_offset_y.abs()
    }
}

/// Cursor-anchored double-tap heart burst particle rendered above the Douyin stage.
#[derive(Debug, Clone)]
pub struct HeartBurst {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub created_at: Instant,
    pub tilt_deg: f32,
}

impl HeartBurst {
    pub fn new(id: u64, x: f32, y: f32) -> Self {
        let tilt = match id % 3 {
            0 => -12.0,
            1 => 10.0,
            _ => -4.0,
        };
        Self {
            id,
            x: if x.is_finite() { x } else { 180.0 },
            y: if y.is_finite() { y } else { 240.0 },
            created_at: Instant::now(),
            tilt_deg: tilt,
        }
    }

    /// Returns `(size_px, top_offset_px, opacity)` or `None` once expired.
    pub fn sample(&self, now: Instant) -> Option<(gpui::Pixels, f32, f32)> {
        let elapsed_ms = now.saturating_duration_since(self.created_at).as_secs_f32() * 1000.0;
        if elapsed_ms >= HEART_BURST_DURATION_MS {
            return None;
        }
        let p = (elapsed_ms / HEART_BURST_DURATION_MS).clamp(0.0, 1.0);
        let scale = if p < 0.22 {
            0.55 + (p / 0.22) * 0.72
        } else if p < 0.42 {
            1.27 - ((p - 0.22) / 0.20) * 0.22
        } else {
            1.05 + ((p - 0.42) / 0.58) * 0.35
        };
        let rise_y = -58.0 * ease_out_cubic(p);
        let opacity = if p < 0.55 {
            1.0
        } else {
            (1.0 - (p - 0.55) / 0.45).clamp(0.0, 1.0)
        };
        Some((px(62.0 * scale), self.y + rise_y, opacity))
    }
}
