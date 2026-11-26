//! 1:1 OpenBot `240Hz` `CrewPile` Soft-Body Physics Engine & GPUI `CrewPileStage` (`OB-F04`).
//! Mirrors `packages/ui/src/features/account/{crew-pile.ts,CrewPileStage.tsx}`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{BloubExpression, EngineShapeId, PlushState};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::bot_engine::Mulberry32;
use super::eyefit::expression_def;
use super::plush_felt::PlushCharacterId;
use super::silhouettes::{Silhouette, engine_shape_radii, silhouette_gpui_path};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrewCharacterCollider {
    pub character: PlushCharacterId,
    pub slug: &'static str,
    pub ink_hex: &'static str,
    pub engine_shape: EngineShapeId,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
}

pub const CHARACTERS: [CrewCharacterCollider; 6] = [
    CrewCharacterCollider {
        character: PlushCharacterId::Yoshi,
        slug: "yoshi",
        ink_hex: "#e94741",
        engine_shape: EngineShapeId::Capsule,
        cy: 0.499,
        w: 0.647,
        h: 0.401,
    },
    CrewCharacterCollider {
        character: PlushCharacterId::Nexo,
        slug: "nexo",
        ink_hex: "#e45206",
        engine_shape: EngineShapeId::Squircle,
        cy: 0.501,
        w: 0.612,
        h: 0.608,
    },
    CrewCharacterCollider {
        character: PlushCharacterId::Malu,
        slug: "malu",
        ink_hex: "#9a55ef",
        engine_shape: EngineShapeId::Cercle,
        cy: 0.500,
        w: 0.626,
        h: 0.622,
    },
    CrewCharacterCollider {
        character: PlushCharacterId::Fred,
        slug: "fred",
        ink_hex: "#efb428",
        engine_shape: EngineShapeId::Hexagone,
        cy: 0.499,
        w: 0.625,
        h: 0.596,
    },
    CrewCharacterCollider {
        character: PlushCharacterId::Vita,
        slug: "vita",
        ink_hex: "#22c377",
        engine_shape: EngineShapeId::Hexagone,
        cy: 0.502,
        w: 0.632,
        h: 0.595,
    },
    CrewCharacterCollider {
        character: PlushCharacterId::Cloe,
        slug: "cloe",
        ink_hex: "#df2986",
        engine_shape: EngineShapeId::Triangle,
        cy: 0.448,
        w: 0.603,
        h: 0.565,
    },
];

pub const CREW_SIZE: f64 = 168.0;
pub const CREW_RADIUS: f64 = (0.25
    * CREW_SIZE
    * ((0.647 + 0.401)
        + (0.612 + 0.608)
        + (0.626 + 0.622)
        + (0.625 + 0.596)
        + (0.632 + 0.595)
        + (0.603 + 0.565)))
    / 6.0;
pub const CREW_AREA: f64 = std::f64::consts::PI * CREW_RADIUS * CREW_RADIUS;

pub const FILL_SHARE: f64 = 0.24;
pub const FILL_WIDTH_SHARE: f64 = 0.42;
pub const FILL_MIN_SIZE: f64 = 120.0;
pub const FILL_MAX_SIZE: f64 = 480.0;
pub const FILL_DENSITY: f64 = 0.78;
pub const FILL_MIN_COUNT: usize = 8;
pub const FILL_MAX_COUNT: usize = 64;

pub const DEFAULT_ROOM: CrewRoom = CrewRoom {
    width: 960.0,
    height: 640.0,
};
pub const FLOOR_INSET: f64 = 8.0;
pub const SPAWN_Y: f64 = -520.0;
pub const SPAWN_SPEED: f64 = 1400.0;
pub const GRAVITY: f64 = 5200.0;
pub const STEP: f64 = 1.0 / 240.0;
pub const ITERATIONS: usize = 4;
pub const FLOOR_BOUNCE: f64 = 0.32;
pub const BODY_BOUNCE: f64 = 0.22;
pub const BOUNCE_SPEED: f64 = 90.0;
pub const SLOP: f64 = 0.5;
pub const SLEEP_SPEED: f64 = 24.0;
pub const SLEEP_SPRING: f64 = 0.6;
pub const SLEEP_AFTER: f64 = 0.3;
pub const SLEEP_WAKE: f64 = 80.0;
pub const SLEEP_REACH: f64 = 3.0;
pub const REACTING_SHARE: f64 = 0.25;
pub const REACTING_MIN: usize = 4;
pub const FRICTION: f64 = 0.5;
pub const AIR_DRAG: f64 = 0.4;
pub const SQUASH_STIFFNESS: f64 = 1100.0;
pub const SQUASH_DAMPING: f64 = 21.0;
pub const SQUASH_KICK: f64 = 0.0011;
pub const SQUASH_MIN: f64 = -0.16;
pub const SQUASH_MAX: f64 = 0.28;
pub const TILT_STIFFNESS: f64 = 420.0;
pub const TILT_DAMPING: f64 = 12.0;
pub const TILT_KICK: f64 = 0.006;
pub const TILT_MAX: f64 = 0.3;
pub const BREATHE_PERIOD: f64 = 3.2;
pub const BREATHE_AMOUNT: f64 = 0.012;
pub const LANDING_SPEED: f64 = 500.0;
pub const STARTLE_SPEED: f64 = 700.0;
pub const SETTLE_AFTER: f64 = 0.55;
pub const SETTLE_SPEED: f64 = 28.0;
pub const SETTLE_LIMIT: f64 = 3.0;
pub const RUSH_RATE: f64 = 2.2;
pub const RUSH_TIME: f64 = 0.8;
pub const DROP_SAMPLES: usize = 24;
pub const EXIT_STAGGER: f64 = 0.05;
pub const EXIT_SPREAD: f64 = 0.8;
pub const EXIT_AFTER: f64 = 0.7;
pub const FILM_FRAME: f64 = 1.0 / 60.0;
pub const FILM_LIMIT: f64 = 30.0;
pub const INTRO_HOLD_S: f64 = 3600.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrewPilePhase {
    Intro,
    Live,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrewFaceState {
    Idle,
    Wide,
}

impl CrewFaceState {
    pub const fn to_plush_state(self) -> PlushState {
        match self {
            Self::Idle => PlushState::Idle,
            Self::Wide => PlushState::Wide,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrewFace {
    pub state: CrewFaceState,
    pub happy: bool,
}

impl CrewFace {
    pub const fn expression(self) -> BloubExpression {
        if self.happy {
            BloubExpression::Heureux
        } else {
            BloubExpression::Neutre
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrewRoom {
    pub width: f64,
    pub height: f64,
}

impl Default for CrewRoom {
    fn default() -> Self {
        DEFAULT_ROOM
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrewPit {
    pub width: f64,
    pub height: f64,
    pub floor: f64,
    pub unit: f64,
}

#[inline]
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

#[inline]
fn between(rng: &mut Mulberry32, min: f64, max: f64) -> f64 {
    min + rng.next_f64() * (max - min)
}

pub fn pit_for(room: CrewRoom, unit: Option<f64>) -> CrewPit {
    let shown = ((room.width * room.height).max(0.0).sqrt() * FILL_SHARE)
        .min(room.width * FILL_WIDTH_SHARE)
        .clamp(FILL_MIN_SIZE, FILL_MAX_SIZE);
    let scale = unit.unwrap_or(shown / CREW_SIZE).max(1e-6);
    let width = (room.width / scale).max(CREW_SIZE);
    let height = (room.height / scale).max(CREW_SIZE);
    CrewPit {
        width,
        height,
        floor: height - FLOOR_INSET,
        unit: scale,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CrewMember {
    pub id: String,
    pub character: PlushCharacterId,
    pub slug: &'static str,
    pub ink_hex: &'static str,
    pub engine_shape: EngineShapeId,
    pub size: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
    pub drop_x: f64,
    pub drop_at: f64,
    pub spawn_lift: f64,
    pub drop_vx: f64,
    pub drop_tilt: f64,
    pub drop_spin: f64,
    pub rest_tilt: f64,
    pub reacts: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CrewBody {
    pub member: CrewMember,
    pub radius: f64,
    pub mass: f64,
    pub active: bool,
    pub x: f64,
    pub y: f64,
    pub last_x: f64,
    pub last_y: f64,
    pub last_tilt: f64,
    pub vx: f64,
    pub vy: f64,
    pub squash: f64,
    pub squash_speed: f64,
    pub tilt: f64,
    pub tilt_speed: f64,
    pub impact: f64,
    pub breathe_phase: f64,
    pub pinned: bool,
    pub asleep: bool,
    pub still_for: f64,
    pub face: CrewFace,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrewPose {
    pub x: f64,
    pub y: f64,
    pub tilt: f64,
    pub scale: f64,
    pub squash_x: f64,
    pub squash_y: f64,
}

impl CrewPose {
    pub fn plush_transform(&self) -> String {
        format!(
            "translate3d({:.2}px, {:.2}px, 0) rotate({:.4}rad) scale({:.4})",
            self.x, self.y, self.tilt, self.scale
        )
    }

    pub fn squash_transform(&self) -> String {
        format!("scale({:.4}, {:.4})", self.squash_x, self.squash_y)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewKeyframe {
    pub offset: f64,
    pub transform: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrewFaceCue {
    pub at: f64,
    pub index: usize,
    pub face: CrewFace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewReel {
    pub length: f64,
    pub plushes: Vec<Vec<CrewKeyframe>>,
    pub squashes: Vec<Vec<CrewKeyframe>>,
    pub faces: Vec<CrewFaceCue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrewRestPose {
    pub plush: String,
    pub squash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewBreatheSpec {
    pub from: f64,
    pub period: f64,
    pub amount: f64,
    pub phases: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrewRushSpec {
    pub rate: f64,
    pub time: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewFilm {
    pub faces: Vec<CrewFace>,
    pub intro: CrewReel,
    pub exit: CrewReel,
    pub rest: Vec<CrewRestPose>,
    pub breathe: CrewBreatheSpec,
    pub rush: CrewRushSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrewHop {
    pub delay_ms: f64,
    pub length_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CrewReducedMotionSummary {
    pub max_squash: f64,
    pub max_tilt: f64,
    pub all_faces_idle: bool,
}

fn create_crew(rng: &mut Mulberry32, pit: CrewPit) -> Vec<CrewMember> {
    let raw_count = js_round((FILL_DENSITY * pit.width * pit.floor) / CREW_AREA);
    let count = (raw_count as usize).clamp(FILL_MIN_COUNT, FILL_MAX_COUNT);
    let drop_window = 0.12 + (count as f64) * 0.05;
    let mut crew = Vec::with_capacity(count);
    for index in 0..count {
        let char_idx = ((rng.next_f64() * (CHARACTERS.len() as f64)).floor() as usize)
            .min(CHARACTERS.len() - 1);
        let body = CHARACTERS[char_idx];
        let size = js_round(CREW_SIZE * between(rng, 0.86, 1.12));
        let side = (pit.width / 2.0 - size * 0.42).max(0.0);
        let drop_x = between(rng, -side, side);
        let drop_at = (((index as f64) + rng.next_f64() * 0.8) / (count as f64)) * drop_window;
        let spawn_lift = rng.next_f64() * 220.0;
        let drop_vx = between(rng, -280.0, 280.0);
        let drop_tilt = between(rng, -0.5, 0.5);
        let drop_spin = between(rng, -7.0, 7.0);
        let rest_tilt = between(rng, -0.14, 0.14);
        crew.push(CrewMember {
            id: format!("{}-{index}", body.slug),
            character: body.character,
            slug: body.slug,
            ink_hex: body.ink_hex,
            engine_shape: body.engine_shape,
            size,
            cy: body.cy,
            w: body.w,
            h: body.h,
            drop_x,
            drop_at,
            spawn_lift,
            drop_vx,
            drop_tilt,
            drop_spin,
            rest_tilt,
            reacts: false,
        });
    }
    crew.sort_by(|a, b| a.drop_at.total_cmp(&b.drop_at));

    let reacting = (js_round((count as f64) * REACTING_SHARE) as usize)
        .max(REACTING_MIN)
        .min(count);
    let mut keyed: Vec<(usize, f64)> = (0..count).map(|idx| (idx, rng.next_f64())).collect();
    keyed.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (idx, _) in keyed.into_iter().take(reacting) {
        if let Some(member) = crew.get_mut(idx) {
            member.reacts = true;
        }
    }
    crew
}

fn create_body(member: CrewMember, index: usize, reduced: bool, pit: CrewPit) -> CrewBody {
    let radius = 0.25 * (member.w + member.h) * member.size;
    let x = pit.width / 2.0 + member.drop_x;
    let y = SPAWN_Y - member.spawn_lift;
    let drop_tilt = member.drop_tilt;
    let drop_vx = member.drop_vx;
    let drop_spin = member.drop_spin;
    let initial_state = if reduced || !member.reacts {
        CrewFaceState::Idle
    } else {
        CrewFaceState::Wide
    };
    CrewBody {
        member,
        radius,
        mass: radius * radius,
        active: false,
        x,
        y,
        last_x: x,
        last_y: y,
        last_tilt: drop_tilt,
        vx: drop_vx,
        vy: SPAWN_SPEED,
        squash: 0.0,
        squash_speed: 0.0,
        tilt: drop_tilt,
        tilt_speed: drop_spin,
        impact: 0.0,
        breathe_phase: (index as f64) * 0.9,
        pinned: false,
        asleep: false,
        still_for: 0.0,
        face: CrewFace {
            state: initial_state,
            happy: false,
        },
    }
}

#[inline]
fn wake(body: &mut CrewBody) {
    body.asleep = false;
    body.still_for = 0.0;
}

#[inline]
fn motion_squared(body: &CrewBody) -> f64 {
    let dx = body.x - body.last_x;
    let dy = body.y - body.last_y;
    (dx * dx + dy * dy) / (STEP * STEP)
}

fn collide_floor(body: &mut CrewBody, pit: CrewPit) {
    let depth = body.y + body.radius - pit.floor;
    if depth <= 0.0 {
        return;
    }
    body.y -= depth;
    if body.vy <= 0.0 {
        return;
    }
    body.impact = body.impact.max(body.vy);
    body.vy = if body.vy > BOUNCE_SPEED {
        -FLOOR_BOUNCE * body.vy
    } else {
        0.0
    };
    let tangent = body.vx;
    body.vx -= FRICTION * tangent;
    body.tilt_speed += tangent * TILT_KICK * 0.5;
}

fn collide_walls(body: &mut CrewBody, pit: CrewPit) {
    if body.x < body.radius {
        body.x = body.radius;
        body.vx = body.vx.abs() * FLOOR_BOUNCE;
    } else if body.x > pit.width - body.radius {
        body.x = pit.width - body.radius;
        body.vx = -body.vx.abs() * FLOOR_BOUNCE;
    }
}

fn collide_pair(a: &mut CrewBody, b: &mut CrewBody) {
    if a.asleep && b.asleep {
        return;
    }
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let reach = a.radius + b.radius + SLEEP_REACH;
    if dx.abs() > reach || dy.abs() > reach {
        return;
    }
    let distance_sq = dx * dx + dy * dy;
    if distance_sq >= reach * reach {
        return;
    }
    let wake_speed_sq = SLEEP_WAKE * SLEEP_WAKE;
    if a.asleep && motion_squared(b) > wake_speed_sq {
        wake(a);
    } else if b.asleep && motion_squared(a) > wake_speed_sq {
        wake(b);
    }
    let raw_dist = distance_sq.sqrt();
    let distance = if raw_dist == 0.0 { 0.001 } else { raw_dist };
    let overlap = a.radius + b.radius - distance;
    if overlap <= 0.0 {
        return;
    }
    let nx = dx / distance;
    let ny = dy / distance;
    let inv_a = if a.pinned || a.asleep {
        0.0
    } else {
        1.0 / a.mass
    };
    let inv_b = if b.pinned || b.asleep {
        0.0
    } else {
        1.0 / b.mass
    };
    if inv_a + inv_b == 0.0 {
        return;
    }
    let share = inv_a / (inv_a + inv_b);
    let push = (overlap - SLOP).max(0.0);
    a.x -= nx * push * share;
    a.y -= ny * push * share;
    b.x += nx * push * (1.0 - share);
    b.y += ny * push * (1.0 - share);

    let rvx = b.vx - a.vx;
    let rvy = b.vy - a.vy;
    let normal = rvx * nx + rvy * ny;
    if normal >= 0.0 {
        return;
    }
    a.impact = a.impact.max(-normal * share);
    b.impact = b.impact.max(-normal * (1.0 - share));
    let bounce = if -normal > BOUNCE_SPEED {
        BODY_BOUNCE
    } else {
        0.0
    };
    let impulse = (-(1.0 + bounce) * normal) / (inv_a + inv_b);
    a.vx -= impulse * nx * inv_a;
    a.vy -= impulse * ny * inv_a;
    b.vx += impulse * nx * inv_b;
    b.vy += impulse * ny * inv_b;

    let tangent = rvx * -ny + rvy * nx;
    a.vx += FRICTION * tangent * -ny * share;
    a.vy += FRICTION * tangent * nx * share;
    b.vx -= FRICTION * tangent * -ny * (1.0 - share);
    b.vy -= FRICTION * tangent * nx * (1.0 - share);
    a.tilt_speed -= tangent * TILT_KICK * share;
    b.tilt_speed += tangent * TILT_KICK * (1.0 - share);
}

fn settle_body(body: &mut CrewBody) {
    let still = motion_squared(body) < SLEEP_SPEED * SLEEP_SPEED
        && body.squash_speed.abs() < SLEEP_SPRING
        && (body.tilt - body.last_tilt).abs() < SLEEP_SPRING * STEP;
    body.still_for = if still { body.still_for + STEP } else { 0.0 };
    if body.still_for < SLEEP_AFTER {
        return;
    }
    body.asleep = true;
    body.vx = 0.0;
    body.vy = 0.0;
    body.squash_speed = 0.0;
    body.tilt_speed = 0.0;
}

fn step_bodies(bodies: &mut [CrewBody], pit: CrewPit, collide: bool) {
    for body in bodies.iter_mut() {
        body.last_x = body.x;
        body.last_y = body.y;
        body.last_tilt = body.tilt;
        if !body.active || body.pinned {
            continue;
        }
        body.impact = 0.0;
        if body.asleep {
            continue;
        }
        body.vy += GRAVITY * STEP;
        body.vx *= 1.0 - AIR_DRAG * STEP;
        body.vy *= 1.0 - AIR_DRAG * STEP;
        body.x += body.vx * STEP;
        body.y += body.vy * STEP;
    }

    if collide {
        for _ in 0..ITERATIONS {
            for body in bodies.iter_mut() {
                if body.active && !body.pinned && !body.asleep {
                    collide_floor(body, pit);
                    collide_walls(body, pit);
                }
            }
            let len = bodies.len();
            for a in 0..len {
                if !bodies[a].active {
                    continue;
                }
                let (left, right) = bodies.split_at_mut(a + 1);
                let first = &mut left[a];
                for second in right.iter_mut() {
                    if second.active {
                        collide_pair(first, second);
                    }
                }
            }
        }
    }

    for body in bodies.iter_mut() {
        if !body.active || body.asleep {
            continue;
        }
        body.squash_speed += body.impact * SQUASH_KICK;
        body.squash_speed +=
            (-SQUASH_STIFFNESS * body.squash - SQUASH_DAMPING * body.squash_speed) * STEP;
        body.squash = (body.squash + body.squash_speed * STEP).clamp(SQUASH_MIN, SQUASH_MAX);
        body.tilt_speed += (-TILT_STIFFNESS * (body.tilt - body.member.rest_tilt)
            - TILT_DAMPING * body.tilt_speed)
            * STEP;
        body.tilt = (body.tilt + body.tilt_speed * STEP).clamp(-TILT_MAX, TILT_MAX);
        if collide && !body.pinned {
            let fall = (body.y - body.last_y) / STEP;
            if body.vy > 0.0 {
                body.vy = body.vy.min(fall.max(0.0));
            }
            settle_body(body);
        }
    }
}

#[derive(Debug, Clone)]
struct TrackRecorder {
    frames: Vec<(f64, String)>,
    held: Option<(f64, String)>,
}

impl TrackRecorder {
    fn new() -> Self {
        Self {
            frames: Vec::new(),
            held: None,
        }
    }

    fn add(&mut self, at: f64, value: String) {
        if self.frames.last().is_some_and(|(_, prev)| *prev == value) {
            self.held = Some((at, value));
            return;
        }
        if let Some(held) = self.held.take() {
            self.frames.push(held);
        }
        self.frames.push((at, value));
    }

    fn keyframes(&self, length: f64) -> Vec<CrewKeyframe> {
        let mut all = self.frames.clone();
        if let Some((last_at, last_val)) = all.last().cloned()
            && last_at < length
        {
            all.push((length, last_val));
        }
        all.into_iter()
            .map(|(at, transform)| CrewKeyframe {
                offset: if length > 0.0 {
                    (at / length).min(1.0)
                } else {
                    1.0
                },
                transform,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct ReelRecorder {
    plushes: Vec<TrackRecorder>,
    squashes: Vec<TrackRecorder>,
    faces: Vec<CrewFaceCue>,
    shown: Vec<CrewFace>,
}

impl ReelRecorder {
    fn new(bodies: &[CrewBody]) -> Self {
        Self {
            plushes: (0..bodies.len()).map(|_| TrackRecorder::new()).collect(),
            squashes: (0..bodies.len()).map(|_| TrackRecorder::new()).collect(),
            faces: Vec::new(),
            shown: bodies.iter().map(|b| b.face).collect(),
        }
    }

    fn add(&mut self, at: f64, poses: &[CrewPose], bodies: &[CrewBody]) {
        for (idx, pose) in poses.iter().enumerate() {
            if let Some(track) = self.plushes.get_mut(idx) {
                track.add(at, pose.plush_transform());
            }
            if let Some(track) = self.squashes.get_mut(idx) {
                track.add(at, pose.squash_transform());
            }
        }
        self.cue(at, bodies);
    }

    fn cue(&mut self, at: f64, bodies: &[CrewBody]) {
        for (idx, body) in bodies.iter().enumerate() {
            if let Some(shown) = self.shown.get_mut(idx)
                && body.face != *shown
            {
                *shown = body.face;
                self.faces.push(CrewFaceCue {
                    at,
                    index: idx,
                    face: body.face,
                });
            }
        }
    }

    fn into_reel(self, length: f64) -> CrewReel {
        CrewReel {
            length,
            plushes: self
                .plushes
                .into_iter()
                .map(|t| t.keyframes(length))
                .collect(),
            squashes: self
                .squashes
                .into_iter()
                .map(|t| t.keyframes(length))
                .collect(),
            faces: self.faces,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ScheduledCrewEvent {
    SetFaceState {
        at: f64,
        body_index: usize,
        state: CrewFaceState,
    },
    LaunchExit {
        at: f64,
        body_index: usize,
        reach: f64,
    },
}

impl ScheduledCrewEvent {
    fn at(self) -> f64 {
        match self {
            Self::SetFaceState { at, .. } | Self::LaunchExit { at, .. } => at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CrewPile {
    pub crew: Vec<CrewMember>,
    pub bodies: Vec<CrewBody>,
    pub reduced: bool,
    pub pit: CrewPit,
    phase: CrewPilePhase,
    seed: u32,
    room: CrewRoom,
    recorded: Option<CrewFilm>,
    time: f64,
    carry: f64,
    exit_started_at: f64,
    exit_delay_ms: f64,
    rush_rate: f64,
    rush_until_exit: bool,
    events: Vec<ScheduledCrewEvent>,
}

impl CrewPile {
    pub fn new(seed: u32, reduced: bool, room: CrewRoom) -> Self {
        let mut pile = Self {
            crew: Vec::new(),
            bodies: Vec::new(),
            reduced,
            pit: pit_for(room, None),
            phase: if reduced {
                CrewPilePhase::Live
            } else {
                CrewPilePhase::Intro
            },
            seed,
            room,
            recorded: None,
            time: 0.0,
            carry: 0.0,
            exit_started_at: -1.0,
            exit_delay_ms: 0.0,
            rush_rate: 1.0,
            rush_until_exit: false,
            events: Vec::new(),
        };
        pile.build();
        pile
    }

    pub fn new_default(seed: u32, reduced: bool) -> Self {
        Self::new(seed, reduced, DEFAULT_ROOM)
    }

    pub fn phase(&self) -> CrewPilePhase {
        self.phase
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    fn build(&mut self) {
        self.time = 0.0;
        self.carry = 0.0;
        self.exit_started_at = -1.0;
        self.exit_delay_ms = 0.0;
        self.rush_rate = 1.0;
        self.rush_until_exit = false;
        self.events.clear();
        let mut rng = Mulberry32::new(self.seed);
        self.crew = create_crew(&mut rng, self.pit);
        self.bodies = self
            .crew
            .iter()
            .cloned()
            .enumerate()
            .map(|(idx, member)| create_body(member, idx, self.reduced, self.pit))
            .collect();

        if self.reduced {
            let end = self.last_drop_at() + 1.5;
            while self.time < end {
                self.advance(STEP * 60.0);
            }
            for body in &mut self.bodies {
                body.squash = 0.0;
                body.squash_speed = 0.0;
                body.tilt = 0.0;
                body.tilt_speed = 0.0;
                body.last_x = body.x;
                body.last_y = body.y;
                body.last_tilt = 0.0;
                body.face = CrewFace {
                    state: CrewFaceState::Idle,
                    happy: false,
                };
            }
            self.events.clear();
            self.phase = CrewPilePhase::Live;
        }
    }

    pub fn fit(&mut self, room: CrewRoom) -> bool {
        if self.recorded.is_some() || room.width <= 0.0 || room.height <= 0.0 {
            return false;
        }
        if (room.width - self.room.width).abs() < 1.0
            && (room.height - self.room.height).abs() < 1.0
        {
            return false;
        }
        self.room = room;
        self.pit = pit_for(room, None);
        self.build();
        true
    }

    pub fn step(&mut self, dt_sec: f64) {
        if dt_sec <= 0.0 || self.reduced {
            return;
        }
        let effective_dt = if self.rush_until_exit {
            dt_sec * self.rush_rate
        } else {
            dt_sec
        };
        self.advance(effective_dt);
        if self.phase == CrewPilePhase::Intro && self.settled() {
            self.phase = CrewPilePhase::Live;
        }
        if self.rush_until_exit && self.phase == CrewPilePhase::Live {
            self.rush_until_exit = false;
            self.rush_rate = 1.0;
            self.start_exit_now();
        }
    }

    pub fn start_faces(&self) -> Vec<CrewFace> {
        if let Some(film) = &self.recorded {
            film.faces.clone()
        } else {
            self.bodies.iter().map(|b| b.face).collect()
        }
    }

    pub fn reduced_motion_summary(&self) -> CrewReducedMotionSummary {
        let max_squash = self
            .bodies
            .iter()
            .map(|b| b.squash.abs())
            .fold(0.0_f64, f64::max);
        let max_tilt = self
            .bodies
            .iter()
            .map(|b| b.tilt.abs())
            .fold(0.0_f64, f64::max);
        let all_faces_idle = self
            .bodies
            .iter()
            .all(|b| b.face.state == CrewFaceState::Idle && !b.face.happy);
        CrewReducedMotionSummary {
            max_squash,
            max_tilt,
            all_faces_idle,
        }
    }

    fn last_drop_at(&self) -> f64 {
        self.crew.last().map_or(0.0, |m| m.drop_at)
    }

    fn spacing(&self, step: f64, total: f64) -> f64 {
        let count = self.crew.len().max(1) as f64;
        step.min(total / count)
    }

    pub fn exit_length(&self) -> f64 {
        let n = self.crew.len().saturating_sub(1) as f64;
        n * self.spacing(EXIT_STAGGER, EXIT_SPREAD) + EXIT_AFTER
    }

    pub fn is_finished(&self) -> bool {
        self.phase == CrewPilePhase::Exit && self.time > self.exit_started_at + self.exit_length()
    }
}

impl CrewPile {
    pub fn poses(&self) -> Vec<CrewPose> {
        let blend = if self.reduced {
            1.0
        } else {
            (self.carry / STEP).clamp(0.0, 1.0)
        };
        self.bodies
            .iter()
            .map(|body| {
                let x = body.last_x + (body.x - body.last_x) * blend;
                let y = body.last_y + (body.y - body.last_y) * blend;
                let tilt = if self.reduced {
                    0.0
                } else {
                    body.last_tilt + (body.tilt - body.last_tilt) * blend
                };
                let squash = if self.reduced { 0.0 } else { body.squash };
                CrewPose {
                    x: x - body.member.size / 2.0,
                    y: y - body.member.size * body.member.cy,
                    tilt,
                    scale: 1.0,
                    squash_x: 1.0 + squash * 0.6,
                    squash_y: 1.0 - squash,
                }
            })
            .collect()
    }

    pub fn poses_at(&self, elapsed_sec: f64) -> Vec<CrewPose> {
        let base = self.poses();
        if self.reduced || elapsed_sec < self.last_drop_at() {
            return base;
        }
        base.into_iter()
            .enumerate()
            .map(|(index, mut pose)| {
                let phase = self.bodies.get(index).map_or(0.0, |b| b.breathe_phase);
                let angle = (std::f64::consts::TAU * elapsed_sec) / BREATHE_PERIOD + phase;
                let breath = BREATHE_AMOUNT * angle.sin();
                pose.squash_x *= 1.0 - breath / 2.0;
                pose.squash_y *= 1.0 + breath;
                pose
            })
            .collect()
    }

    pub fn film(&mut self) -> CrewFilm {
        if let Some(existing) = &self.recorded {
            return existing.clone();
        }
        let faces: Vec<CrewFace> = self.bodies.iter().map(|b| b.face).collect();
        let mut intro = ReelRecorder::new(&self.bodies);
        if !self.reduced {
            intro.add(0.0, &self.poses(), &self.bodies);
            while self.phase == CrewPilePhase::Intro && self.time < FILM_LIMIT {
                self.advance(FILM_FRAME);
                if self.settled() {
                    self.phase = CrewPilePhase::Live;
                }
                intro.add(self.time, &self.poses(), &self.bodies);
            }
        }
        let intro_length = self.time;
        let rest: Vec<CrewRestPose> = self
            .poses()
            .iter()
            .map(|pose| CrewRestPose {
                plush: pose.plush_transform(),
                squash: pose.squash_transform(),
            })
            .collect();

        let mut pending = std::mem::take(&mut self.events);
        pending.sort_by(|a, b| a.at().total_cmp(&b.at()));
        for ev in pending {
            let at = ev.at();
            self.run_scheduled_event(ev);
            intro.cue(at, &self.bodies);
        }

        let mut exit = ReelRecorder::new(&self.bodies);
        let exit_from = self.time;
        if !self.reduced {
            self.phase = CrewPilePhase::Live;
            self.start_exit_now();
            exit.add(0.0, &self.poses(), &self.bodies);
            while !self.is_finished() && (self.time - exit_from) < FILM_LIMIT {
                self.advance(FILM_FRAME);
                exit.add(self.time - exit_from, &self.poses(), &self.bodies);
            }
        }

        let recorded = CrewFilm {
            faces,
            intro: intro.into_reel(intro_length),
            exit: exit.into_reel(self.time - exit_from),
            rest,
            breathe: CrewBreatheSpec {
                from: if self.reduced {
                    0.0
                } else {
                    self.last_drop_at().min(intro_length)
                },
                period: BREATHE_PERIOD,
                amount: BREATHE_AMOUNT,
                phases: self.bodies.iter().map(|b| b.breathe_phase).collect(),
            },
            rush: CrewRushSpec {
                rate: RUSH_RATE,
                time: RUSH_TIME,
            },
        };
        self.recorded = Some(recorded.clone());
        recorded
    }

    pub fn start_exit_now(&mut self) {
        self.phase = CrewPilePhase::Exit;
        self.exit_started_at = self.time;
        let stagger = self.spacing(EXIT_STAGGER, EXIT_SPREAD);
        let reach = (self.pit.width / 620.0).max(1.0);

        let mut order: Vec<(usize, f64)> = self
            .bodies
            .iter()
            .enumerate()
            .map(|(idx, b)| (idx, b.y))
            .collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));

        let mut rank_for = vec![0usize; self.bodies.len()];
        for (rank, (body_idx, _)) in order.into_iter().enumerate() {
            if let Some(slot) = rank_for.get_mut(body_idx) {
                *slot = rank;
            }
        }

        for (index, body) in self.bodies.iter_mut().enumerate() {
            wake(body);
            if body.member.reacts {
                body.face.happy = true;
            }
            body.pinned = true;
            body.vx = 0.0;
            body.vy = 0.0;
            let delay = (rank_for[index] as f64) * stagger;
            self.events.push(ScheduledCrewEvent::LaunchExit {
                at: self.time + delay,
                body_index: index,
                reach,
            });
        }
    }

    pub fn hop_out(&mut self) -> CrewHop {
        if self.reduced {
            return CrewHop {
                delay_ms: 0.0,
                length_ms: 0.0,
            };
        }
        let exit_len_ms = self.exit_length() * 1000.0;
        if self.phase == CrewPilePhase::Exit {
            return CrewHop {
                delay_ms: self.exit_delay_ms,
                length_ms: exit_len_ms,
            };
        }
        for body in &mut self.bodies {
            if body.member.reacts {
                body.face.happy = true;
            }
        }
        if self.phase == CrewPilePhase::Intro && !self.settled() {
            let intro_end = self.last_drop_at() + SETTLE_AFTER;
            let left = (intro_end - self.time).max(0.0);
            if left > 0.0 {
                let rate = RUSH_RATE.max(left / RUSH_TIME);
                self.rush_rate = rate;
                self.rush_until_exit = true;
                self.exit_delay_ms = (left / rate) * 1000.0;
                return CrewHop {
                    delay_ms: self.exit_delay_ms,
                    length_ms: exit_len_ms,
                };
            }
        }
        self.exit_delay_ms = 0.0;
        self.start_exit_now();
        CrewHop {
            delay_ms: 0.0,
            length_ms: exit_len_ms,
        }
    }
}

impl CrewPile {
    pub fn hit_test_body(&self, pit_x: f64, pit_y: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (idx, body) in self.bodies.iter().enumerate() {
            if !body.active {
                continue;
            }
            let dist = (body.x - pit_x).hypot(body.y - pit_y);
            if dist <= body.radius * 1.15 && best.is_none_or(|(_, prev_dist)| dist < prev_dist) {
                best = Some((idx, dist));
            }
        }
        best.map(|(idx, _)| idx)
    }

    pub fn pointer_drag(
        &mut self,
        index: Option<usize>,
        pit_x: f64,
        pit_y: f64,
        dt_sec: f64,
    ) -> Option<usize> {
        if self.reduced {
            return None;
        }
        let target_idx = index.or_else(|| self.hit_test_body(pit_x, pit_y))?;
        let pit = self.pit;
        let body = self.bodies.get_mut(target_idx)?;
        wake(body);
        body.active = true;
        body.pinned = true;
        let clamped_x = pit_x.clamp(body.radius, (pit.width - body.radius).max(body.radius));
        let clamped_y = pit_y.min(pit.floor - body.radius);
        let safe_dt = dt_sec.max(STEP);
        body.vx = ((clamped_x - body.x) / safe_dt).clamp(-3600.0, 3600.0);
        body.vy = ((clamped_y - body.y) / safe_dt).clamp(-3600.0, 3600.0);
        body.last_x = body.x;
        body.last_y = body.y;
        body.x = clamped_x;
        body.y = clamped_y;
        if body.member.reacts && body.vx.hypot(body.vy) > STARTLE_SPEED {
            body.face.state = CrewFaceState::Wide;
        }
        Some(target_idx)
    }

    pub fn pointer_release(&mut self, index: usize, throw_vx: Option<f64>, throw_vy: Option<f64>) {
        if self.reduced {
            return;
        }
        let time_now = self.time;
        let mut schedule_idle = false;
        if let Some(body) = self.bodies.get_mut(index) {
            body.pinned = false;
            wake(body);
            if let Some(vx) = throw_vx {
                body.vx = vx.clamp(-3600.0, 3600.0);
            }
            if let Some(vy) = throw_vy {
                body.vy = vy.clamp(-3600.0, 3600.0);
            }
            body.tilt_speed += body.vx * TILT_KICK * 0.25;
            if body.member.reacts && body.vx.hypot(body.vy) > STARTLE_SPEED {
                body.face.state = CrewFaceState::Wide;
                schedule_idle = true;
            }
        }
        if schedule_idle {
            self.events.push(ScheduledCrewEvent::SetFaceState {
                at: time_now + 0.35,
                body_index: index,
                state: CrewFaceState::Idle,
            });
        }
    }

    pub fn pointer_repel(&mut self, pit_x: f64, pit_y: f64, radius: f64, strength: f64) {
        if self.reduced || radius <= 0.0 || strength <= 0.0 {
            return;
        }
        let time_now = self.time;
        let mut startled = Vec::new();
        for (idx, body) in self.bodies.iter_mut().enumerate() {
            if !body.active || body.pinned {
                continue;
            }
            let dx = body.x - pit_x;
            let dy = body.y - pit_y;
            let reach = radius + body.radius;
            let dist = dx.hypot(dy);
            if dist >= reach {
                continue;
            }
            wake(body);
            let (nx, ny) = if dist > 1e-4 {
                (dx / dist, dy / dist)
            } else {
                (0.0, -1.0)
            };
            let factor = 1.0 - (dist / reach);
            let kick = strength * factor;
            body.vx = (body.vx + nx * kick).clamp(-3600.0, 3600.0);
            body.vy = (body.vy + ny * kick).clamp(-3600.0, 3600.0);
            body.squash_speed += kick * SQUASH_KICK * 0.5;
            if body.member.reacts && kick > STARTLE_SPEED * 0.5 {
                body.face.state = CrewFaceState::Wide;
                startled.push(idx);
            }
        }
        for idx in startled {
            self.events.push(ScheduledCrewEvent::SetFaceState {
                at: time_now + 0.35,
                body_index: idx,
                state: CrewFaceState::Idle,
            });
        }
    }

    fn run_scheduled_event(&mut self, ev: ScheduledCrewEvent) {
        match ev {
            ScheduledCrewEvent::SetFaceState {
                body_index, state, ..
            } => {
                if let Some(body) = self.bodies.get_mut(body_index) {
                    body.face.state = state;
                }
            }
            ScheduledCrewEvent::LaunchExit {
                body_index, reach, ..
            } => {
                let half_w = self.pit.width / 2.0;
                if let Some(body) = self.bodies.get_mut(body_index) {
                    let diff = body.x - half_w;
                    let side = if diff > 0.0 {
                        1.0
                    } else if diff < 0.0 {
                        -1.0
                    } else if body_index % 2 != 0 {
                        1.0
                    } else {
                        -1.0
                    };
                    body.active = true;
                    body.vx = side * (720.0 + (((body_index * 137) % 5) as f64) * 110.0) * reach;
                    body.vy = -(1500.0 + (((body_index * 61) % 4) as f64) * 160.0);
                    body.pinned = false;
                    body.squash_speed -= 6.0;
                    body.tilt_speed += side * 9.0;
                }
            }
        }
    }

    fn advance(&mut self, seconds: f64) {
        let collide = self.phase != CrewPilePhase::Exit;
        let mut remaining = self.carry + seconds;
        while remaining >= STEP {
            remaining -= STEP;
            self.time += STEP;
            for idx in 0..self.bodies.len() {
                if !self.bodies[idx].active
                    && self.phase != CrewPilePhase::Exit
                    && self.time >= self.bodies[idx].member.drop_at
                {
                    self.bodies[idx].active = true;
                    self.fill_hole(idx);
                    let b = &mut self.bodies[idx];
                    b.last_x = b.x;
                    b.last_y = b.y;
                    b.last_tilt = b.tilt;
                }
            }

            step_bodies(&mut self.bodies, self.pit, collide);

            if self.phase == CrewPilePhase::Intro {
                for idx in 0..self.bodies.len() {
                    let b = &self.bodies[idx];
                    if b.pinned || !b.member.reacts || b.impact < LANDING_SPEED {
                        continue;
                    }
                    if b.face.state == CrewFaceState::Wide {
                        self.events.push(ScheduledCrewEvent::SetFaceState {
                            at: self.time + 0.22,
                            body_index: idx,
                            state: CrewFaceState::Idle,
                        });
                    } else if b.impact > STARTLE_SPEED {
                        self.bodies[idx].face.state = CrewFaceState::Wide;
                        self.events.push(ScheduledCrewEvent::SetFaceState {
                            at: self.time + 0.35,
                            body_index: idx,
                            state: CrewFaceState::Idle,
                        });
                    }
                }
            }

            for i in (0..self.events.len()).rev() {
                if self.events[i].at() <= self.time {
                    let ev = self.events.remove(i);
                    self.run_scheduled_event(ev);
                }
            }
        }
        self.carry = remaining;
    }
}

impl CrewPile {
    fn rest_at(&self, falling_idx: usize, x: f64) -> f64 {
        let falling_r = self.bodies[falling_idx].radius;
        let mut rest = self.pit.floor - falling_r;
        for (idx, body) in self.bodies.iter().enumerate() {
            if idx == falling_idx || !body.active {
                continue;
            }
            let reach = body.radius + falling_r;
            let dx = (body.x - x).abs();
            if dx < reach {
                let lift = (reach * reach - dx * dx).max(0.0).sqrt();
                rest = rest.min(body.y - lift);
            }
        }
        rest
    }

    fn fill_hole(&mut self, falling_idx: usize) {
        let current_x = self.bodies[falling_idx].x;
        let r = self.bodies[falling_idx].radius;
        if self.rest_at(falling_idx, current_x) - r < 0.0 {
            self.bodies[falling_idx].x = self.lowest_point(falling_idx);
        }
    }

    fn lowest_point(&self, falling_idx: usize) -> f64 {
        let falling_x = self.bodies[falling_idx].x;
        let falling_r = self.bodies[falling_idx].radius;
        let mut best = falling_x;
        let mut best_rest = f64::NEG_INFINITY;
        let left = falling_r;
        let right = (self.pit.width - falling_r).max(left);
        for sample in 0..=DROP_SAMPLES {
            let x = left + ((right - left) * (sample as f64)) / (DROP_SAMPLES as f64);
            let rest = self.rest_at(falling_idx, x);
            let score = rest - (x - falling_x).abs() * 0.1;
            if score > best_rest {
                best_rest = score;
                best = x;
            }
        }
        best
    }

    pub fn settled(&self) -> bool {
        let last = self.last_drop_at();
        if self.time < last + SETTLE_AFTER {
            return false;
        }
        if self.time > last + SETTLE_LIMIT {
            return true;
        }
        self.bodies.iter().all(|b| b.vx.hypot(b.vy) < SETTLE_SPEED)
    }
}

fn crew_asset_path(rel: &str) -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(rel);
    if manifest_dir.is_file() {
        return Some(manifest_dir);
    }
    let cwd_candidate = PathBuf::from("crates/gpui_telegram_desktop/assets").join(rel);
    if cwd_candidate.is_file() {
        return Some(cwd_candidate);
    }
    None
}

fn load_bgra_render_image(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

fn cached_character_corps(slug: &'static str) -> Option<Arc<RenderImage>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<Arc<RenderImage>>>>> =
        OnceLock::new();
    let map = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(entry) = map.lock().get(slug).cloned() {
        return entry;
    }
    let rel = format!("openbot_plush/bloub/{slug}-corps.webp");
    let loaded = crew_asset_path(&rel)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|raw| load_bgra_render_image(&raw));
    map.lock().insert(slug, loaded.clone());
    loaded
}

fn parse_rgb_u32(hex: &str) -> u32 {
    let clean = hex.trim().trim_start_matches('#');
    if clean.len() >= 6 {
        u32::from_str_radix(&clean[0..6], 16).unwrap_or(0xE45206)
    } else {
        0xE45206
    }
}

#[derive(Debug, Clone, Default)]
pub struct CrewPileStageOptions {
    pub elapsed_sec: f64,
    pub viewport: Option<CrewRoom>,
    pub custom_urls: HashMap<String, String>,
    pub failed_urls: HashSet<String>,
}

pub fn render_crew_pile_stage(
    pile: &CrewPile,
    elapsed_sec: f64,
    failed_urls: Option<&HashSet<String>>,
) -> Div {
    let opts = CrewPileStageOptions {
        elapsed_sec,
        viewport: None,
        custom_urls: HashMap::new(),
        failed_urls: failed_urls.cloned().unwrap_or_default(),
    };
    render_crew_pile_stage_with_options(pile, &opts)
}

pub fn render_crew_pile_stage_with_options(pile: &CrewPile, opts: &CrewPileStageOptions) -> Div {
    let pit = pile.pit;
    let pit_scale = match opts.viewport {
        Some(room) if pit.width > 0.0 && pit.height > 0.0 => pit
            .unit
            .max(room.width / pit.width)
            .max(room.height / pit.height),
        _ => pit.unit,
    }
    .max(0.05) as f32;

    let stage_w = (pit.width as f32) * pit_scale;
    let stage_h = (pit.height as f32) * pit_scale;
    let poses = pile.poses_at(opts.elapsed_sec);

    let mut stage = div()
        .relative()
        .w(px(stage_w))
        .h(px(stage_h))
        .overflow_hidden();

    for (idx, body) in pile.bodies.iter().enumerate() {
        if !pile.reduced && !body.active {
            continue;
        }
        let Some(pose) = poses.get(idx).copied() else {
            continue;
        };
        stage = stage.child(render_crew_member_node(body, pose, pit_scale, opts));
    }
    stage
}

fn can_use_custom_url(url: &str, failed_urls: &HashSet<String>) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || failed_urls.contains(trimmed) {
        return false;
    }
    if let Some(rest) = trimmed.strip_prefix("data:") {
        let Some((_, b64)) = rest.split_once(";base64,") else {
            return false;
        };
        use base64::Engine as _;
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return false;
        };
        return image::guess_format(&raw).is_ok();
    }
    if trimmed.starts_with('/') || trimmed.starts_with("file://") {
        let fs_path = trimmed.strip_prefix("file://").unwrap_or(trimmed);
        return std::path::Path::new(fs_path).is_file();
    }
    trimmed.starts_with("http://") || trimmed.starts_with("https://")
}

fn render_crew_member_node(
    body: &CrewBody,
    pose: CrewPose,
    pit_scale: f32,
    opts: &CrewPileStageOptions,
) -> Div {
    let member = &body.member;
    let base_px = (member.size as f32) * pit_scale;
    let disp_w = (base_px * (pose.squash_x as f32)).max(4.0);
    let disp_h = (base_px * (pose.squash_y as f32)).max(4.0);
    let left_px = (pose.x as f32) * pit_scale + (base_px - disp_w) * 0.5;
    let top_px = (pose.y as f32) * pit_scale + (base_px - disp_h) * (member.cy as f32);

    let custom_valid = opts
        .custom_urls
        .get(&member.id)
        .filter(|u| can_use_custom_url(u, &opts.failed_urls))
        .cloned();

    if let Some(url) = custom_valid {
        return div()
            .absolute()
            .left(px(left_px))
            .top(px(top_px))
            .w(px(disp_w))
            .h(px(disp_h))
            .rounded_full()
            .overflow_hidden()
            .child(img(url).w_full().h_full().object_fit(ObjectFit::Cover));
    }

    let corps_img = cached_character_corps(member.slug);
    let face = body.face;
    let engine_shape = member.engine_shape;
    let ink_rgb = parse_rgb_u32(member.ink_hex);
    let tilt = pose.tilt;
    let sx = pose.squash_x;
    let sy = pose.squash_y;

    div()
        .absolute()
        .left(px(left_px))
        .top(px(top_px))
        .w(px(disp_w))
        .h(px(disp_h))
        .when_some(corps_img.clone(), |el, render_img| {
            el.child(
                img(render_img)
                    .w(px(disp_w))
                    .h(px(disp_h))
                    .object_fit(ObjectFit::Contain),
            )
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    let center = bounds.center();
                    let min_dim = bounds.size.width.min(bounds.size.height);
                    let px_per_vb = f32::from(min_dim) / 316.0;
                    if corps_img.is_none() {
                        let sil = Silhouette {
                            radii: engine_shape_radii(engine_shape),
                            rot: tilt,
                            cx: 0.0,
                            cy: 0.0,
                            sx,
                            sy,
                        };
                        if let Some(path) = silhouette_gpui_path(&sil, center, px_per_vb) {
                            window.paint_path(path, rgb(ink_rgb));
                        }
                    }

                    let expr_def = expression_def(face.expression());
                    let eye_spread = (expr_def.split as f32) * px_per_vb * 1.15;
                    let eye_scale = if face.state == CrewFaceState::Wide {
                        1.28_f32
                    } else {
                        1.0_f32
                    };
                    for (side_idx, spec) in expr_def.eyes.iter().enumerate() {
                        let sign = if side_idx == 0 { -1.0_f32 } else { 1.0_f32 };
                        let ex = f32::from(center.x) + sign * eye_spread;
                        let ey = f32::from(center.y) - 4.0 * px_per_vb;
                        let rx = ((spec.w as f32) * 42.0 * px_per_vb * eye_scale).max(1.5);
                        let ry = ((spec.h as f32) * 42.0 * px_per_vb * eye_scale).max(1.5);
                        if face.happy {
                            let stroke_w = (rx * 0.38).max(1.5);
                            let mut arc = PathBuilder::stroke(px(stroke_w));
                            let p0 = point(px(ex - rx), px(ey + ry * 0.2));
                            let ctrl = point(px(ex), px(ey - ry * 0.95));
                            let p1 = point(px(ex + rx), px(ey + ry * 0.2));
                            arc.move_to(p0);
                            arc.curve_to(p1, ctrl);
                            if let Ok(path) = arc.build() {
                                window.paint_path(path, rgb(0x1C1A1D));
                            }
                        } else {
                            let mut oval = PathBuilder::fill();
                            for step in 0..=16 {
                                let ang = (step as f32 / 16.0) * std::f32::consts::TAU;
                                let pt = point(px(ex + rx * ang.cos()), px(ey + ry * ang.sin()));
                                if step == 0 {
                                    oval.move_to(pt);
                                } else {
                                    oval.line_to(pt);
                                }
                            }
                            oval.close();
                            if let Ok(path) = oval.build() {
                                window.paint_path(path, rgb(0x1C1A1D));
                            }
                        }
                    }
                },
            )
            .absolute()
            .inset_0()
            .w(px(disp_w))
            .h(px(disp_h)),
        )
}
