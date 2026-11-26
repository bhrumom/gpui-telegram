//! Procedural pixel-art `ChatBackdrop` scenes (`pond`, `jellyfish`, `sunset`, `snowfall`,
//! `fireflies`, `coast`) and `CustomBackdropModel` (`OB-F05` Part B).
//!
//! 1:1 port of:
//! - `packages/brand/src/chat-backdrop-scenes.ts` (`L1-L917`)
//! - `packages/ui/src/features/conversation/chat-backdrop.ts` (`L1-L86`)
//! - `packages/ui/src/features/conversation/ChatBackdrop.tsx` (`L1-L97`)
//! - `packages/ui/src/features/conversation/CustomBackdrop.tsx` (`L1-L92`)
//! - `src/renderer/src/custom-backdrops.ts` (`L1-L166`)
//! - `packages/contracts/src/chat-visual.ts` (`L58-L131`)

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    canvas, div, img, point, px, rgba, Bounds, EntityId, ObjectFit, PathBuilder, Pixels,
    RenderImage, Window,
};
use serde::{Deserialize, Serialize};

use super::tokens::OpenBotColorScheme;

pub const BACKDROP_CELL_PX: u32 = 6;
pub const BACKDROP_FRAME_MS: u64 = 50;
pub const BACKDROP_STILL_TIME: f64 = 42.0;
pub const BACKDROP_CALM_CENTER_ALPHA: f64 = 0.55;

pub const CUSTOM_BACKDROP_IMAGE_LIMIT: usize = 20 * 1024 * 1024;
pub const CUSTOM_BACKDROP_VIDEO_LIMIT: usize = 100 * 1024 * 1024;
pub const CUSTOM_BACKDROP_PAGE_LIMIT: usize = 8 * 1024 * 1024;
pub const CUSTOM_BACKDROP_CAPACITY: usize = 12;
pub const CUSTOM_BACKDROP_SCRIM_ALPHA: f64 = 0.35;
pub const CUSTOM_BACKDROP_NAME_LIMIT: usize = 80;
pub const CHAT_VISUAL_FRAME_SANDBOX: &str = "allow-scripts allow-forms";

pub const CHAT_BACKDROP_SCENES: [ChatBackdropScene; 6] = [
    ChatBackdropScene::Pond,
    ChatBackdropScene::Jellyfish,
    ChatBackdropScene::Sunset,
    ChatBackdropScene::Snowfall,
    ChatBackdropScene::Fireflies,
    ChatBackdropScene::Coast,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatBackdropScene {
    Pond,
    Jellyfish,
    Sunset,
    Snowfall,
    Fireflies,
    Coast,
}

impl ChatBackdropScene {
    pub const fn canonical_str(self) -> &'static str {
        match self {
            Self::Pond => "pond",
            Self::Jellyfish => "jellyfish",
            Self::Sunset => "sunset",
            Self::Snowfall => "snowfall",
            Self::Fireflies => "fireflies",
            Self::Coast => "coast",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Pond => "Koi pond",
            Self::Jellyfish => "Jellyfish",
            Self::Sunset => "Retro sunset",
            Self::Snowfall => "Snowfall",
            Self::Fireflies => "Fireflies",
            Self::Coast => "Neon coast",
        }
    }

    pub const fn i18n_key(self) -> &'static str {
        match self {
            Self::Pond => "settings.chatBackdrop.pond",
            Self::Jellyfish => "settings.chatBackdrop.jellyfish",
            Self::Sunset => "settings.chatBackdrop.sunset",
            Self::Snowfall => "settings.chatBackdrop.snowfall",
            Self::Fireflies => "settings.chatBackdrop.fireflies",
            Self::Coast => "settings.chatBackdrop.coast",
        }
    }

    pub const fn base_hex(self, scheme: OpenBotColorScheme) -> &'static str {
        match (self, scheme) {
            (Self::Pond, OpenBotColorScheme::Light) => "#e8f6f3",
            (Self::Pond, OpenBotColorScheme::Dark) => "#0f2426",
            (Self::Jellyfish, OpenBotColorScheme::Light) => "#ebf3fa",
            (Self::Jellyfish, OpenBotColorScheme::Dark) => "#0e1a2b",
            (Self::Sunset, OpenBotColorScheme::Light) => "#fbf0f3",
            (Self::Sunset, OpenBotColorScheme::Dark) => "#1d1528",
            (Self::Snowfall, OpenBotColorScheme::Light) => "#eef3f7",
            (Self::Snowfall, OpenBotColorScheme::Dark) => "#111a26",
            (Self::Fireflies, OpenBotColorScheme::Light) => "#f2f5ea",
            (Self::Fireflies, OpenBotColorScheme::Dark) => "#121a14",
            (Self::Coast, OpenBotColorScheme::Light) => "#fdf0ec",
            (Self::Coast, OpenBotColorScheme::Dark) => "#1a1030",
        }
    }

    pub fn parse_canonical(raw: &str) -> Option<Self> {
        match raw.trim() {
            "pond" => Some(Self::Pond),
            "jellyfish" => Some(Self::Jellyfish),
            "sunset" => Some(Self::Sunset),
            "snowfall" => Some(Self::Snowfall),
            "fireflies" => Some(Self::Fireflies),
            "coast" => Some(Self::Coast),
            _ => None,
        }
    }

    /// Parses canonical scene names and CLI aliases, returning `(canonical_enum, requested_alias)`.
    pub fn parse_with_alias(raw: &str) -> Option<(Self, &'static str)> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pond" => Some((Self::Pond, "pond")),
            "jellyfish" => Some((Self::Jellyfish, "jellyfish")),
            "forest" => Some((Self::Jellyfish, "forest")),
            "sunset" => Some((Self::Sunset, "sunset")),
            "city" => Some((Self::Sunset, "city")),
            "snowfall" => Some((Self::Snowfall, "snowfall")),
            "snow" => Some((Self::Snowfall, "snow")),
            "fireflies" => Some((Self::Fireflies, "fireflies")),
            "desert" => Some((Self::Fireflies, "desert")),
            "coast" => Some((Self::Coast, "coast")),
            _ => None,
        }
    }
}

pub fn is_chat_backdrop_scene(raw: &str) -> bool {
    ChatBackdropScene::parse_canonical(raw).is_some()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatBackdropChoice {
    Scene(ChatBackdropScene),
    Custom(String),
}

impl ChatBackdropChoice {
    pub fn parse(raw: &str) -> Option<Self> {
        if let Some(scene) = ChatBackdropScene::parse_canonical(raw) {
            return Some(Self::Scene(scene));
        }
        let rest = raw.strip_prefix("custom:")?;
        if rest.is_empty() {
            None
        } else {
            Some(Self::Custom(rest.to_string()))
        }
    }

    pub fn custom_id(&self) -> Option<&str> {
        match self {
            Self::Scene(_) => None,
            Self::Custom(id) => Some(id.as_str()),
        }
    }
}

pub const KOI_FRAMES: [[&str; 5]; 2] = [
    [
        "T....FF.....",
        "TT.BBOOWWO..",
        ".TBOOWWOOEOO",
        "TT.BOOOWWO..",
        "T....FF.....",
    ],
    [
        "............",
        "TT.BBOOWWO..",
        "TTBOOWWOOEOO",
        "TT.BOOOWWO..",
        "......FF....",
    ],
];

pub const JELLY_FRAMES: [[&str; 4]; 2] = [
    ["..XXX..", ".XXXXX.", "XXHXXXX", "X.X.X.X"],
    ["..XXX..", ".XXXXX.", ".XHXXX.", ".X.X.X."],
];

pub const COAST_CAR: [&str; 4] = [
    "...RRRRR....",
    "..RWWRWWR...",
    "LRRRRRRRRRRH",
    ".KK.....KK..",
];
pub const POLICE_CAR: [&str; 5] = [
    "....AB......",
    "...WWWWW....",
    "..WGGWGGW...",
    "LKKKWWWKKKKH",
    ".TT.....TT..",
];
pub const HELI_ROTOR: [&str; 2] = ["RRRRRRRRRRRR.", "...RRRRRR...."];
pub const HELI_BODY: [&str; 6] = [
    "......M......",
    "...BBBBB.....",
    "..GBBBBBBBBBB",
    "...BBBBB...BB",
    "...S...S.....",
    "..SSSSSSS....",
];
pub const SPEEDBOAT: [&str; 3] = ["....GW...", "PWWWWWWWW", ".WWWWWWW."];
pub const YACHT: [&str; 10] = [
    "..............M.............",
    ".............MMM............",
    "..........WWWWWWWW..........",
    ".........WGGGGGGGGW.........",
    "......WWWWWWWWWWWWWWW.......",
    ".....WGGGGGGGGGGGGGGGW......",
    "WWWWWWWWWWWWWWWWWWWWWWWWWWWW",
    "WPPPPPPPPPPPPPPPPPPPPPPPPPP.",
    ".HHHHHHHHHHHHHHHHHHHHHHHH...",
    "..HHHHHHHHHHHHHHHHHHHHHH....",
];
pub const FLAMINGO: [&str; 12] = [
    "..PPB", "..P..", ".P...", ".P...", "..P..", "PPPP.", "PPPPP", ".PPP.", "..L..", "..L..",
    "..L..", ".LL..",
];
pub const PARTY_COLORS: [&str; 5] = ["#ff4fb8", "#4ff0e0", "#ffd27a", "#ff7a3c", "#b08cff"];
pub const FLAMES: [&str; 3] = ["#ff3b1f", "#ff8a1f", "#ffd24a"];
pub const CHASE_SECONDS: f64 = 13.0;
pub const BURN_SECONDS: f64 = 9.0;
pub const ROAD_HEIGHT: i32 = 14;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatBackdropFrame {
    pub scene: ChatBackdropScene,
    pub width: i32,
    pub height: i32,
    pub time: f64,
    pub dark: bool,
}

pub trait ChatBackdropCells {
    fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, color: &str, alpha: f64);
}

pub fn hash(value: f64) -> f64 {
    let x = (value * 127.1 + 311.7).sin() * 43_758.545_3;
    x - x.floor()
}

pub fn wrap(value: f64, size: f64) -> f64 {
    if size == 0.0 {
        0.0
    } else {
        ((value % size) + size) % size
    }
}

pub fn js_round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

pub fn cell<C: ChatBackdropCells + ?Sized>(cells: &mut C, x: f64, y: f64, color: &str, alpha: f64) {
    cells.fill(js_round(x), js_round(y), 1, 1, color, alpha);
}

pub fn disc<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    cx: f64,
    cy: f64,
    radius: f64,
    color: &str,
    alpha: f64,
) {
    let r = radius.ceil() as i32;
    let r_sq = radius * radius;
    for dy in -r..=r {
        for dx in -r..=r {
            if ((dx * dx + dy * dy) as f64) <= r_sq {
                cells.fill(
                    js_round(cx + dx as f64),
                    js_round(cy + dy as f64),
                    1,
                    1,
                    color,
                    alpha,
                );
            }
        }
    }
}

pub fn sprite<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    rows: &[&str],
    x: f64,
    y: f64,
    colors: &[(char, &str)],
    flip: bool,
    alpha: f64,
) {
    let left = js_round(x);
    let top = js_round(y);
    for (row_index, row) in rows.iter().enumerate() {
        let row_bytes = row.as_bytes();
        let row_len = row_bytes.len() as i32;
        for (col_idx, &ch_byte) in row_bytes.iter().enumerate() {
            let ch = ch_byte as char;
            let Some((_, color)) = colors.iter().find(|(k, _)| *k == ch) else {
                continue;
            };
            let col = col_idx as i32;
            let gx = left + if flip { row_len - 1 - col } else { col };
            cells.fill(gx, top + row_index as i32, 1, 1, color, alpha);
        }
    }
}

pub fn line<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    color: &str,
    alpha: f64,
) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1.0);
    let mut step = 0_i32;
    while (step as f64) <= steps {
        let ratio = (step as f64) / steps;
        cell(
            cells,
            x0 + (x1 - x0) * ratio,
            y0 + (y1 - y0) * ratio,
            color,
            alpha,
        );
        step += 1;
    }
}

pub fn count_for(frame: &ChatBackdropFrame, cells_each: f64, min: usize, max: usize) -> usize {
    let area = (frame.width as f64) * (frame.height as f64);
    let rounded = js_round(area / cells_each).max(0) as usize;
    rounded.clamp(min, max)
}

pub fn twinkling_stars<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    color: &str,
    top: f64,
    seed: f64,
) {
    let count = count_for(frame, 260.0, 12, 90);
    for index in 0..count {
        let idx = index as f64;
        let light =
            (frame.time * (0.6 + hash(seed + idx) * 1.4) + hash(seed + idx + 50.0) * 6.28).sin();
        cell(
            cells,
            hash(seed + idx + 100.0) * (frame.width as f64),
            hash(seed + idx + 200.0) * top,
            color,
            0.25 + 0.6 * light.max(0.0),
        );
    }
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn lerp(from: f64, to: f64, share: f64) -> f64 {
    from + (to - from) * share
}

fn rgb_mix(from: [u8; 3], to: [u8; 3], share: f64) -> String {
    let amount = clamp01(share);
    let r = js_round(from[0] as f64 + (to[0] as f64 - from[0] as f64) * amount).clamp(0, 255);
    let g = js_round(from[1] as f64 + (to[1] as f64 - from[1] as f64) * amount).clamp(0, 255);
    let b = js_round(from[2] as f64 + (to[2] as f64 - from[2] as f64) * amount).clamp(0, 255);
    format!("rgb({r} {g} {b})")
}

fn paint_pond<C: ChatBackdropCells + ?Sized>(cells: &mut C, frame: &ChatBackdropFrame) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let water = if dark { "#1c4245" } else { "#bfe6e0" };
    let pad = if dark { "#2d6847" } else { "#7cc48a" };
    let pad_dark = if dark { "#21503a" } else { "#5ea96d" };
    let flower = if dark { "#e7a3c4" } else { "#f39ac0" };
    let koi = [
        ('B', if dark { "#e9eef0" } else { "#ffffff" }),
        ('W', if dark { "#e9eef0" } else { "#ffffff" }),
        ('O', if dark { "#e8823a" } else { "#f28a3c" }),
        ('F', if dark { "#c96f33" } else { "#f6ad74" }),
        ('T', if dark { "#c96f33" } else { "#f6ad74" }),
        ('E', "#1f1f1f"),
    ];

    let glints = count_for(frame, 140.0, 16, 120);
    for index in 0..glints {
        let idx = index as f64;
        let glow = (t * (0.5 + hash(idx) * 0.8) + hash(idx + 1.0) * 6.28).sin();
        if glow <= 0.2 {
            continue;
        }
        let x = wrap(hash(idx + 2.0) * w + t * 0.4, w);
        let y = hash(idx + 3.0) * h;
        cell(cells, x, y, water, glow);
        cell(cells, x + 1.0, y, water, glow * 0.6);
    }

    let period = 7.0;
    for index in 0..4 {
        let idx = index as f64;
        let clock = t + (idx * period) / 4.0;
        let round = (clock / period).floor();
        let progress = wrap(clock, period) / period;
        let cx = hash(round * 13.0 + idx) * w;
        let cy = hash(round * 17.0 + idx + 5.0) * h;
        let radius = 1.0 + progress * 7.0;
        let mut angle = 0.0_f64;
        while angle < 6.28 {
            cell(
                cells,
                cx + angle.cos() * radius,
                cy + angle.sin() * radius * 0.6,
                water,
                1.0 - progress,
            );
            angle += 1.0 / radius;
        }
    }

    let fish = count_for(frame, 1_100.0, 3, 8);
    let span = w + 30.0;
    for index in 0..fish {
        let idx = index as f64;
        let direction = if hash(idx + 20.0) > 0.5 { 1.0 } else { -1.0 };
        let speed = 2.0 + hash(idx + 10.0) * 2.5;
        let x = wrap(hash(idx + 30.0) * span + t * speed * direction, span) - 15.0;
        let y = hash(idx + 40.0) * (h - 6.0) + (t * 0.7 + idx).sin() * 2.0;
        let frame_idx = (t * 4.0 + idx).floor() as isize;
        let tail = if frame_idx >= 0 {
            &KOI_FRAMES[(frame_idx as usize) % 2]
        } else {
            &KOI_FRAMES[0]
        };
        sprite(cells, tail, x, y, &koi, direction < 0.0, 1.0);
    }

    let pads: [[f64; 3]; 5] = [
        [0.07, 0.16, 4.5],
        [0.92, 0.82, 5.5],
        [0.12, 0.86, 3.5],
        [0.88, 0.2, 3.5],
        [0.95, 0.5, 2.5],
    ];
    let pond_base = ChatBackdropScene::Pond.base_hex(if dark {
        OpenBotColorScheme::Dark
    } else {
        OpenBotColorScheme::Light
    });
    for (index, &[px_norm, py_norm, radius]) in pads.iter().enumerate() {
        let cx = px_norm * w;
        let cy = py_norm * h + (t * 0.5 + index as f64).sin() * 0.6;
        disc(cells, cx, cy, radius, pad, 1.0);
        disc(cells, cx + 1.0, cy + 1.0, radius - 1.5, pad_dark, 0.5);
        let mut notch = 0_i32;
        while (notch as f64) <= radius {
            cell(
                cells,
                cx + notch as f64,
                cy - (notch as f64) * 0.4,
                pond_base,
                1.0,
            );
            notch += 1;
        }
        if index == 1 {
            disc(cells, cx - 1.0, cy - 1.0, 1.2, flower, 1.0);
        }
    }
}

fn paint_jellyfish<C: ChatBackdropCells + ?Sized>(cells: &mut C, frame: &ChatBackdropFrame) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let kelp = if dark { "#1f5a47" } else { "#86c9a1" };
    let bubble = if dark { "#35598a" } else { "#a9cbe8" };
    let tones = if dark {
        ["#ff8fc6", "#a493ff", "#6fd3ff"]
    } else {
        ["#f29cc3", "#ad97ee", "#79c3ea"]
    };

    let bubbles = count_for(frame, 200.0, 10, 70);
    for index in 0..bubbles {
        let idx = index as f64;
        let y = h - wrap(hash(idx) * h + t * (2.0 + hash(idx + 1.0) * 3.0), h + 4.0);
        let x = hash(idx + 2.0) * w + (t * 2.0 + idx).sin() * 0.8;
        cell(cells, x, y, bubble, 0.8);
    }

    let stalks = js_round(w / 30.0).max(4) as usize;
    for index in 0..stalks {
        let idx = index as f64;
        let side = index % 2 == 0;
        let base = if side {
            hash(idx + 60.0) * w * 0.14
        } else {
            w - 1.0 - hash(idx + 60.0) * w * 0.14
        };
        let height = h * (0.22 + hash(idx + 61.0) * 0.25);
        let mut step = 0_i32;
        while (step as f64) < height {
            let step_f = step as f64;
            let sway = (t * 0.8 + step_f * 0.15 + idx).sin() * (step_f / height) * 3.0;
            cell(cells, base + sway, h - 1.0 - step_f, kelp, 1.0);
            if step % 5 == 2 {
                let offset = if step % 10 == 2 { 1.0 } else { -1.0 };
                cell(cells, base + sway + offset, h - 1.0 - step_f, kelp, 0.8);
            }
            step += 1;
        }
    }

    let jellies = count_for(frame, 1_500.0, 3, 7);
    let span = h + 24.0;
    let highlight = if dark { "#ffffff" } else { "#fff6fb" };
    for index in 0..jellies {
        let idx = index as f64;
        let tone = tones[index % tones.len()];
        let pulse_idx = (t * 1.5 + idx).floor() as isize;
        let jelly_frame = if pulse_idx >= 0 {
            &JELLY_FRAMES[(pulse_idx as usize) % 2]
        } else {
            &JELLY_FRAMES[0]
        };
        let rise = if (t * 1.5 * std::f64::consts::PI + idx).sin() > 0.0 {
            1.6
        } else {
            0.8
        };
        let y = h + 8.0 - wrap(hash(idx + 70.0) * span + t * rise, span);
        let x = hash(idx + 71.0) * (w - 8.0) + (t * 0.35 + idx * 2.0).sin() * 3.0;
        if dark {
            disc(cells, x + 3.0, y + 2.0, 5.0, tone, 0.1);
        }
        let palette = [('X', tone), ('H', highlight)];
        sprite(cells, jelly_frame, x, y, &palette, false, 1.0);
        for strand in 0..3 {
            let strand_f = strand as f64;
            let length = 5 + js_round(hash((index * 3 + strand) as f64) * 4.0);
            for step in 0..length {
                let step_f = step as f64;
                let sway =
                    (t * 2.0 + step_f * 0.6 + strand_f + idx).sin() * (step_f / 3.0).min(1.0);
                cell(
                    cells,
                    x + 1.0 + strand_f * 2.0 + sway,
                    y + 4.0 + step_f,
                    tone,
                    0.75 - step_f * 0.06,
                );
            }
        }
    }
}

fn paint_sunset<C: ChatBackdropCells + ?Sized>(cells: &mut C, frame: &ChatBackdropFrame) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let horizon = (h * 0.68).floor();
    let grid = if dark { "#ff4fb8" } else { "#f4bdd9" };
    let ridge = if dark { "#3b2160" } else { "#ecc9e2" };
    let ridge_far = if dark { "#2a1a45" } else { "#f3dbea" };

    if dark {
        twinkling_stars(cells, frame, "#ffffff", horizon * 0.8, 400.0);
    } else {
        for index in 0..4 {
            let idx = index as f64;
            let length = 8.0 + hash(idx + 300.0) * 10.0;
            let x = wrap(hash(idx + 301.0) * (w + 30.0) + t * 0.6, w + 30.0) - 20.0;
            let y = 3.0 + hash(idx + 302.0) * horizon * 0.5;
            line(cells, x, y, x + length, y, "#ffffff", 0.9);
            line(
                cells,
                x + 2.0,
                y - 1.0,
                x + length - 3.0,
                y - 1.0,
                "#ffffff",
                0.9,
            );
        }
    }

    let radius = (w.min(h) * 0.18).clamp(6.0, 22.0);
    let sx = w * 0.74;
    let sy = horizon - 2.0;
    let top: [u8; 3] = if dark {
        [255, 214, 92]
    } else {
        [255, 196, 120]
    };
    let bottom: [u8; 3] = if dark {
        [255, 79, 139]
    } else {
        [255, 120, 170]
    };
    for dy in -(radius.ceil() as i32)..=0 {
        let dy_f = dy as f64;
        let share = (dy_f + radius) / radius;
        if dy_f > -radius * 0.55 {
            let band = wrap(dy_f + t * 1.5, 5.0);
            if band < 1.0 + share * 1.5 {
                continue;
            }
        }
        let half = (radius * radius - dy_f * dy_f).max(0.0).sqrt();
        let color = rgb_mix(top, bottom, share);
        line(
            cells,
            sx - half,
            sy + dy_f,
            sx + half,
            sy + dy_f,
            &color,
            1.0,
        );
    }

    for x in 0..frame.width {
        let x_f = x as f64;
        let far = ((x_f * 0.09 + 2.0).sin() + (x_f * 0.033).sin()) * h * 0.04 + h * 0.06;
        let near = (((x_f * 0.14).sin() + (x_f * 0.05 + 1.0).sin()) * h * 0.05 + h * 0.03).max(0.0)
            * if x_f < w * 0.5 { 1.0 } else { 0.4 };
        line(cells, x_f, horizon - far, x_f, horizon, ridge_far, 1.0);
        line(cells, x_f, horizon - near, x_f, horizon, ridge, 1.0);
    }

    let depth = h - horizon;
    let rows = 8.0;
    for row in 0..8 {
        let z = wrap(row as f64 + t * 0.6, rows) / rows;
        let y = horizon + depth * z * z;
        line(cells, 0.0, y, w, y, grid, 0.2 + 0.35 * z);
    }
    let vanish = w * 0.5;
    for column in -12..=12 {
        let col = column as f64;
        line(
            cells,
            vanish + col * 2.0,
            horizon,
            vanish + col * w * 0.12,
            h + 2.0,
            grid,
            0.3,
        );
    }
    line(cells, 0.0, horizon, w, horizon, grid, 0.7);
}

fn paint_snowfall<C: ChatBackdropCells + ?Sized>(cells: &mut C, frame: &ChatBackdropFrame) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let tree = if dark { "#1f3b35" } else { "#8db7a6" };
    let tree_shade = if dark { "#18302b" } else { "#73a08e" };
    let snow = if dark { "#dfe8f5" } else { "#ffffff" };
    let flake = if dark { "#dfe8f5" } else { "#a7bed3" };
    let trunk = if dark { "#2d2420" } else { "#9a8676" };

    if dark {
        twinkling_stars(cells, frame, "#c9d6ff", h * 0.5, 500.0);
        let mx = w * 0.84;
        let my = h * 0.16;
        disc(cells, mx, my, 4.5, "#f3ecd2", 1.0);
        cell(cells, mx - 1.0, my - 1.0, "#d9cfae", 1.0);
        cell(cells, mx + 2.0, my + 1.0, "#d9cfae", 1.0);
    }

    let ground = js_round(h * 0.03).max(2);
    cells.fill(0, frame.height - ground, frame.width, ground, snow, 1.0);

    let pines = js_round(w / 18.0).max(6) as usize;
    let bottom = (frame.height - ground) as f64;
    for index in 0..pines {
        let idx = index as f64;
        let side = index % 2 == 0;
        let offset = hash(idx + 80.0) * w * 0.2;
        let x = if side { offset } else { w - 1.0 - offset };
        let height = js_round(10.0 + hash(idx + 81.0) * 14.0);
        line(cells, x, bottom - 2.0, x, bottom, trunk, 1.0);
        let tier_div = js_round(height as f64 / 3.0).max(4);
        for step in 0..height {
            let y = bottom - 2.0 - step as f64;
            let tier = step % tier_div;
            let half = js_round(((height - step) as f64 / height as f64) * 6.0 - tier as f64 * 0.6)
                .max(0) as f64;
            let row_color = if step % 2 == 0 { tree } else { tree_shade };
            line(cells, x - half, y, x + half, y, row_color, 1.0);
            if tier == 0 && half > 0.0 {
                cell(cells, x - half, y, snow, 1.0);
                cell(cells, x + half, y, snow, 1.0);
            }
        }
        cell(cells, x, bottom - 2.0 - height as f64, snow, 1.0);
    }

    let flakes = count_for(frame, 70.0, 30, 260);
    for index in 0..flakes {
        let idx = index as f64;
        let near = hash(idx + 90.0);
        let y = wrap(hash(idx + 91.0) * h + t * (1.5 + near * 3.0), h);
        let x = wrap(
            hash(idx + 92.0) * w + (t * (0.5 + near) + idx).sin() * 1.5 + t * 0.4,
            w,
        );
        cell(cells, x, y, flake, 0.35 + near * 0.6);
    }
}

fn paint_fireflies<C: ChatBackdropCells + ?Sized>(cells: &mut C, frame: &ChatBackdropFrame) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let grass = if dark { "#1f3a22" } else { "#a6cf88" };
    let grass_tip = if dark { "#2a4d2c" } else { "#88bb67" };
    let glow = if dark { "#e9ff8a" } else { "#e3b23c" };

    if dark {
        twinkling_stars(cells, frame, "#d8e2c0", h * 0.4, 600.0);
    }

    for x in 0..frame.width {
        let x_f = x as f64;
        let edge = (x_f / w - 0.5).abs() * 2.0;
        let height = js_round((2.0 + hash(x_f + 700.0) * 5.0) * (0.6 + edge * 1.2));
        for step in 0..height {
            let step_f = step as f64;
            let height_f = height as f64;
            let sway = if step_f > height_f * 0.5 {
                js_round((t * 1.2 + x_f * 0.3).sin() * (step_f / height_f))
            } else {
                0
            };
            let color = if step > height - 2 { grass_tip } else { grass };
            cell(
                cells,
                (x + sway) as f64,
                (frame.height - 1 - step) as f64,
                color,
                1.0,
            );
        }
    }

    let flies = count_for(frame, 500.0, 8, 28);
    let halo_scale = if dark { 0.12 } else { 0.18 };
    for index in 0..flies {
        let idx = index as f64;
        let x = wrap(
            hash(idx + 710.0) * w + (t * 0.25 * (1.0 + hash(idx + 711.0)) + idx).sin() * w * 0.06,
            w,
        );
        let y = h * 0.3
            + hash(idx + 712.0) * h * 0.62
            + (t * 0.45 * (1.0 + hash(idx + 713.0)) + idx * 2.0).sin() * 4.0;
        let blink = (t * (0.8 + hash(idx + 714.0)) + idx * 1.7).sin().max(0.0);
        if blink <= 0.05 {
            continue;
        }
        disc(cells, x, y, 2.2, glow, blink * halo_scale);
        cell(cells, x - 1.0, y, glow, blink * 0.35);
        cell(cells, x + 1.0, y, glow, blink * 0.35);
        cell(cells, x, y - 1.0, glow, blink * 0.35);
        cell(cells, x, y + 1.0, glow, blink * 0.35);
        cell(cells, x, y, glow, blink);
    }
}

#[derive(Debug, Clone, Copy)]
struct CoastLayout {
    horizon: i32,
    shore: i32,
    road_top: i32,
    road_bottom: i32,
    lane: f64,
    crash_x: i32,
    crash_at: f64,
    speed: f64,
    after: f64,
    fade: f64,
}

fn coast_layout(frame: &ChatBackdropFrame) -> CoastLayout {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let shore = (h * 0.58).floor() as i32;
    let road_top = shore + js_round(h * 0.03).max(1);
    let crash_x = js_round(w * 0.8);
    let crash_at = CHASE_SECONDS;
    let cycle = CHASE_SECONDS + BURN_SECONDS;
    let local = wrap(frame.time, cycle);
    CoastLayout {
        horizon: (h * 0.45).floor() as i32,
        shore,
        road_top,
        road_bottom: road_top + ROAD_HEIGHT,
        lane: road_top as f64 + (ROAD_HEIGHT as f64) / 2.0,
        crash_x,
        crash_at,
        speed: (crash_x as f64 + 28.0) / CHASE_SECONDS,
        after: local - crash_at,
        fade: clamp01((cycle - local) / 1.5),
    }
}

fn flame(t: f64, seed: f64) -> &'static str {
    let idx = (hash(seed + (t * 10.0).floor()) * (FLAMES.len() as f64)).floor() as isize;
    if idx >= 0 && (idx as usize) < FLAMES.len() {
        FLAMES[idx as usize]
    } else {
        FLAMES[0]
    }
}

fn party_color(index: f64) -> &'static str {
    let wrapped = wrap(index.floor(), PARTY_COLORS.len() as f64) as usize;
    PARTY_COLORS
        .get(wrapped)
        .copied()
        .unwrap_or(PARTY_COLORS[0])
}

fn headlights<C: ChatBackdropCells + ?Sized>(cells: &mut C, front: f64, y: f64, direction: f64) {
    for step in 0..7 {
        let step_f = step as f64;
        let beam_x = front + step_f * direction;
        cell(cells, beam_x, y, "#fff3b0", 0.35 - step_f * 0.045);
        if step > 2 {
            cell(cells, beam_x, y + 1.0, "#fff3b0", 0.2 - step_f * 0.02);
        }
    }
}

struct PalmBase {
    x: f64,
    y: f64,
    height: i32,
    lean: f64,
    seed: f64,
}

fn palm<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    base: PalmBase,
    burning: f64,
) {
    let t = frame.time;
    let dark = frame.dark;
    let trunk = if dark { "#3a2433" } else { "#b98d6e" };
    let frond = if dark { "#1d4a45" } else { "#4f9f7a" };
    let frond_tip = if dark { "#2a6359" } else { "#73bd92" };
    let height_f = base.height as f64;
    let mut top_x = base.x;
    for step in 0..=base.height {
        let share = (step as f64) / height_f;
        let x = base.x + base.lean * share * share * height_f * 0.35;
        let y = base.y - step as f64;
        cell(cells, x, y, trunk, 1.0);
        cell(
            cells,
            x + 1.0,
            y,
            trunk,
            if step % 3 == 0 { 0.6 } else { 1.0 },
        );
        top_x = x;
    }
    let top_y = base.y - height_f;
    let sway = (t * 1.1 + base.seed).sin() * (0.08 + burning * 0.1);
    let angles = [-2.9, -2.4, -1.9, -1.2, -0.7, -0.2];
    for (index, &angle) in angles.iter().enumerate() {
        let idx = index as f64;
        let length = height_f * (0.45 + hash(base.seed + idx) * 0.2);
        let mut step = 0_i32;
        while (step as f64) < length {
            let step_f = step as f64;
            let bend = angle + sway * (step_f / length);
            let x = top_x + 0.5 + bend.cos() * step_f;
            let y = top_y + bend.sin() * step_f * 0.55 + step_f * step_f * 0.035;
            cell(
                cells,
                x,
                y,
                if step_f > length * 0.7 {
                    frond_tip
                } else {
                    frond
                },
                1.0,
            );
            if step % 2 == 0 && step > 2 {
                cell(cells, x, y + 1.0, frond, 0.7);
            }
            let spark = base.seed + idx * 40.0 + step_f;
            if burning > 0.0 && hash(spark) > 0.3 {
                cell(cells, x, y - 1.0, flame(t, spark), burning);
            }
            step += 1;
        }
    }
    disc(cells, top_x + 0.5, top_y + 1.0, 1.3, trunk, 1.0);
    if burning <= 0.0 {
        return;
    }
    for ember in 0..10 {
        let emb = ember as f64;
        let age = wrap(t * 0.8 + hash(base.seed + emb + 300.0), 1.0);
        let x = top_x + (hash(base.seed + emb + 301.0) - 0.5) * 12.0 + age * 4.0;
        cell(
            cells,
            x,
            top_y - 2.0 - age * 12.0,
            flame(t, base.seed + emb),
            burning * (1.0 - age),
        );
    }
}

fn coast_sky<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let t = frame.time;
    let dark = frame.dark;
    let horizon = layout.horizon;
    let horizon_f = horizon as f64;
    let glow = if dark { "#ff5c8c" } else { "#ffbaa0" };
    let glow_start = (horizon_f * 0.3).floor() as i32;
    if dark {
        twinkling_stars(cells, frame, "#ffd9f0", horizon_f * 0.45, 800.0);
    }
    let denom = (horizon - glow_start).max(1) as f64;
    for y in glow_start..horizon {
        let share = (y - glow_start) as f64 / denom;
        cells.fill(
            0,
            y,
            frame.width,
            1,
            glow,
            share * share * if dark { 0.75 } else { 0.8 },
        );
    }
    let bird = if dark { "#f4c3dc" } else { "#7a5168" };
    for index in 0..3 {
        let idx = index as f64;
        let x = wrap(
            hash(idx + 810.0) * (w + 20.0) + t * (1.2 + hash(idx + 811.0)),
            w + 20.0,
        ) - 10.0;
        let y = horizon_f * (0.18 + hash(idx + 812.0) * 0.3) + (t * 0.6 + idx).sin() * 1.5;
        let flap = if ((t * 3.0 + idx).floor() as i64).rem_euclid(2) == 0 {
            -1.0
        } else {
            0.0
        };
        cell(cells, x - 1.0, y + flap, bird, 0.8);
        cell(cells, x, y, bird, 0.8);
        cell(cells, x + 1.0, y + flap, bird, 0.8);
    }
}

fn coast_sun<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let horizon = layout.horizon;
    let shore = layout.shore;
    let horizon_f = horizon as f64;
    let radius = (w.min(h) * 0.14).clamp(5.0, 18.0);
    let sx = w * 0.2;
    let (sun_top, sun_bottom): ([u8; 3], [u8; 3]) = if dark {
        ([255, 208, 96], [255, 70, 130])
    } else {
        ([255, 214, 130], [255, 120, 110])
    };
    let halo_color = rgb_mix(sun_top, sun_bottom, 0.6);
    disc(
        cells,
        sx,
        horizon_f,
        radius + 4.0,
        &halo_color,
        if dark { 0.12 } else { 0.16 },
    );
    for dy in -(radius.ceil() as i32)..=0 {
        let dy_f = dy as f64;
        let half = (radius * radius - dy_f * dy_f).max(0.0).sqrt();
        let row_color = rgb_mix(sun_top, sun_bottom, (dy_f + radius) / radius);
        line(
            cells,
            sx - half,
            horizon_f + dy_f,
            sx + half,
            horizon_f + dy_f,
            &row_color,
            1.0,
        );
    }
    let water_top: [u8; 3] = if dark { [70, 32, 96] } else { [247, 200, 196] };
    let water_bottom: [u8; 3] = if dark { [18, 44, 74] } else { [143, 211, 208] };
    let span_water = (shore - horizon).max(1) as f64;
    for y in (horizon + 1)..shore {
        let row_color = rgb_mix(water_top, water_bottom, (y - horizon) as f64 / span_water);
        cells.fill(0, y, frame.width, 1, &row_color, 1.0);
    }
    let mut y = horizon + 1;
    while y < shore {
        let y_f = y as f64;
        let depth = (y - horizon) as f64 / span_water;
        let length = radius * (1.0 - depth * 0.6) * (0.5 + 0.5 * hash(y_f + (t * 3.0).floor()));
        let drift = (t * 1.6 + y_f * 0.7).sin() * 1.5;
        let shimmer_color = rgb_mix(sun_top, sun_bottom, depth);
        line(
            cells,
            sx - length + drift,
            y_f,
            sx + length + drift,
            y_f,
            &shimmer_color,
            0.7 - depth * 0.4,
        );
        y += 2;
    }
    let glints = count_for(frame, 220.0, 8, 60);
    for index in 0..glints {
        let idx = index as f64;
        let shine = (t * (0.7 + hash(idx + 830.0)) + hash(idx + 831.0) * 6.28).sin();
        if shine <= 0.3 {
            continue;
        }
        let x = wrap(hash(idx + 832.0) * w + t * 0.5, w);
        let gy = horizon_f + 1.0 + hash(idx + 833.0) * ((shore - horizon - 2) as f64);
        line(
            cells,
            x,
            gy,
            x + 1.0,
            gy,
            "#ffffff",
            shine * if dark { 0.5 } else { 0.7 },
        );
    }
}

fn coast_skyline<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let horizon = layout.horizon;
    let skyline_start = (w * 0.68).floor() as i32;
    let skyline_end = (w * 0.98).floor() as i32;
    let building = if dark { "#2b1745" } else { "#e6b3c0" };
    let window_lights = if dark {
        ["#ffd27a", "#4ff0e0", "#ff7ad1"]
    } else {
        ["#fff4e6", "#fff4e6", "#ffe0ec"]
    };
    for x in skyline_start..=skyline_end {
        let block = ((x - skyline_start) as f64 / 4.0).floor();
        let tall = hash(block + 820.0);
        let height = js_round(h * (0.04 + tall * tall * 0.16));
        cells.fill(x, horizon - height, 1, height + 1, building, 1.0);
        if (x - skyline_start) % 4 == 3 {
            continue;
        }
        let mut y = horizon - height + 2;
        while y < horizon - 1 {
            let lit = hash((x * 31 + y * 7) as f64 + (t * 0.25 + hash((x + y) as f64)).floor());
            if lit >= 0.55 {
                let l_idx = ((lit * 10.0).floor() as usize) % window_lights.len();
                cell(
                    cells,
                    x as f64,
                    y as f64,
                    window_lights[l_idx],
                    if dark { 0.9 } else { 0.8 },
                );
            }
            y += 2;
        }
    }
    let tower_x =
        js_round(skyline_start as f64 + (skyline_end - skyline_start) as f64 * 0.55) as f64;
    let tower_top = (horizon - js_round(h * 0.24)) as f64;
    line(
        cells,
        tower_x,
        tower_top,
        tower_x,
        horizon as f64,
        building,
        1.0,
    );
    line(
        cells,
        tower_x + 1.0,
        tower_top + 3.0,
        tower_x + 1.0,
        horizon as f64,
        building,
        1.0,
    );
    if (t * 3.0).sin() > 0.0 {
        cell(cells, tower_x, tower_top - 1.0, "#ff3b5c", 1.0);
    }
}

fn coast_yacht<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let t = frame.time;
    let dark = frame.dark;
    let horizon_f = layout.horizon as f64;
    let length = YACHT[0].len() as f64;
    let span = w + length * 2.0;
    let x = wrap(hash(870.0) * span - t * 2.0, span) - length;
    let y = horizon_f + js_round((layout.shore - layout.horizon) as f64 * 0.3) as f64
        - (YACHT.len() as f64)
        + 2.0;
    let bottom = y + YACHT.len() as f64;
    let mast_col = YACHT[0].find('M').unwrap_or(14) as f64;
    let mast_x = x + length - 1.0 - mast_col;
    let blink = (t * 4.0).floor();
    for end in [x, x + length - 1.0] {
        let steps = (end - mast_x).abs().max(6.0);
        let mut step = 0_i32;
        while (step as f64) <= steps {
            let share = (step as f64) / steps;
            cell(
                cells,
                lerp(mast_x, end, share),
                lerp(y, y + 6.0, share),
                party_color((step as f64) / 2.0 + blink),
                0.9,
            );
            step += 2;
        }
    }
    let yacht_palette = [
        ('M', if dark { "#c9c0d8" } else { "#9a86a0" }),
        ('W', if dark { "#e9e4f0" } else { "#ffffff" }),
        ('G', if dark { "#ffd27a" } else { "#bfe9f0" }),
        ('P', if dark { "#ff4fb8" } else { "#f0609e" }),
        ('H', if dark { "#2a2050" } else { "#3a4a7a" }),
    ];
    sprite(cells, &YACHT, x, y, &yacht_palette, true, 1.0);
    for index in 0..12 {
        let idx = index as f64;
        let ripple = (t * 2.0 + idx * 1.7).sin();
        if ripple <= 0.0 {
            continue;
        }
        cell(
            cells,
            x + hash(idx + 871.0) * length,
            bottom + 1.0 + hash(idx + 872.0) * 3.0,
            party_color(idx + blink),
            ripple * 0.45,
        );
    }
    for shot in 0..3 {
        let shot_f = shot as f64;
        let clock = t + shot_f * 0.9;
        let round = (clock / 2.7).floor();
        let phase = wrap(clock, 2.7);
        let launch_x = x + 4.0 + hash(round * 5.0 + shot_f) * (length - 8.0);
        let burst_y = (horizon_f * (0.15 + hash(round * 5.0 + shot_f + 1.0) * 0.35)).max(4.0);
        let color = party_color(hash(round * 5.0 + shot_f + 2.0) * (PARTY_COLORS.len() as f64));
        if phase < 0.8 {
            let rocket_y = lerp(y, burst_y, phase / 0.8);
            cell(cells, launch_x, rocket_y, "#fff3b0", 1.0);
            cell(cells, launch_x, rocket_y + 1.0, color, 0.6);
            cell(cells, launch_x, rocket_y + 2.0, color, 0.3);
            continue;
        }
        let burst = phase - 0.8;
        let alpha = clamp01(1.0 - burst / 1.6);
        if burst < 0.15 {
            disc(cells, launch_x, burst_y, 3.0, "#ffffff", 0.5);
        }
        for spark in 0..24 {
            let angle = (spark as f64 / 24.0) * std::f64::consts::TAU;
            let reach = burst * if spark % 2 == 0 { 13.0 } else { 8.0 };
            let spark_x = launch_x + angle.cos() * reach;
            let spark_y = burst_y + angle.sin() * reach * 0.85 + burst * burst * 5.0;
            cell(cells, spark_x, spark_y, color, alpha);
            cell(
                cells,
                spark_x - angle.cos(),
                spark_y - angle.sin(),
                color,
                alpha * 0.4,
            );
        }
    }
}

fn coast_helicopter<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let t = frame.time;
    let dark = frame.dark;
    let horizon_f = layout.horizon as f64;
    let shore_f = layout.shore as f64;
    let track = clamp01(layout.after / 2.0) * layout.fade;
    let patrol_x = w * 0.78 + (t * 0.25).sin() * w * 0.07;
    let x = lerp(patrol_x, layout.crash_x as f64 - 16.0, track);
    let y = (horizon_f - h * 0.42).max(3.0) + (t * 0.9).sin() * 1.2;
    let flip = track > 0.5 || (t * 0.25).cos() > 0.0;
    if dark {
        let beam_x = x + 6.5;
        let beam_top = y + 6.0;
        let target_y = lerp(
            horizon_f + (shore_f - horizon_f) * 0.5,
            layout.lane - 3.0,
            track,
        );
        let beam_length = (target_y - beam_top).max(1.0);
        let target_x = lerp(
            beam_x + (t * 0.7).sin() * 0.45 * beam_length,
            layout.crash_x as f64,
            track,
        );
        let mut step = 0_i32;
        while (step as f64) < beam_length {
            let step_f = step as f64;
            let half = 0.5 + step_f * 0.16;
            let center = lerp(beam_x, target_x, step_f / beam_length);
            cells.fill(
                js_round(center - half),
                js_round(beam_top + step_f),
                js_round(half * 2.0).max(1),
                1,
                "#fff6d8",
                0.16 - (step_f / beam_length) * 0.08,
            );
            step += 1;
        }
    }
    let body = if dark { "#0d0816" } else { "#6b5270" };
    let rotor_idx = (t * 12.0).floor() as isize;
    let rotor_row = if rotor_idx >= 0 {
        HELI_ROTOR[(rotor_idx as usize) % 2]
    } else {
        HELI_ROTOR[0]
    };
    let heli_rows = [
        rotor_row,
        HELI_BODY[0],
        HELI_BODY[1],
        HELI_BODY[2],
        HELI_BODY[3],
        HELI_BODY[4],
        HELI_BODY[5],
    ];
    let heli_palette = [
        ('R', if dark { "#4a3a5a" } else { "#9a86a0" }),
        ('M', body),
        ('B', body),
        ('S', body),
        ('G', if dark { "#4ff0e0" } else { "#bfe9f0" }),
    ];
    sprite(cells, &heli_rows, x, y, &heli_palette, flip, 1.0);
    if (t * 4.0).sin() > 0.6 {
        cell(
            cells,
            if flip { x } else { x + 12.0 },
            y + 3.0,
            "#ff3b5c",
            1.0,
        );
    }
}

fn coast_speedboat<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let t = frame.time;
    let dark = frame.dark;
    let span = w + 40.0;
    let x = wrap(hash(860.0) * span + t * 5.0, span) - 20.0;
    let bounce = if (t * 3.0).sin() > 0.5 { 1.0 } else { 0.0 };
    let y = layout.horizon as f64
        + js_round((layout.shore - layout.horizon) as f64 * 0.55) as f64
        + bounce;
    for step in 1..16 {
        let step_f = step as f64;
        let spread = step_f * 0.22;
        let alpha = 0.6 - step_f * 0.035;
        cell(cells, x - step_f, y + 2.0 - spread, "#ffffff", alpha);
        cell(cells, x - step_f, y + 2.0 + spread, "#ffffff", alpha);
    }
    let boat_palette = [
        ('W', if dark { "#e9e4f0" } else { "#ffffff" }),
        ('P', if dark { "#ff4fb8" } else { "#f0609e" }),
        ('G', if dark { "#4ff0e0" } else { "#9fd8ff" }),
    ];
    sprite(cells, &SPEEDBOAT, x, y, &boat_palette, false, 1.0);
}

fn coast_beach<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let t = frame.time;
    let dark = frame.dark;
    let sand = if dark { "#3a2440" } else { "#f6dcb4" };
    let curb = if dark { "#ff4fb8" } else { "#f39ac0" };
    cells.fill(
        0,
        layout.shore,
        frame.width,
        layout.road_top - layout.shore,
        sand,
        1.0,
    );
    let flamingo = [
        ('P', if dark { "#ff7ab8" } else { "#f58bb4" }),
        ('B', "#2a1a2a"),
        ('L', if dark { "#d0628f" } else { "#e07aa3" }),
    ];
    let flam_y = (layout.road_top - FLAMINGO.len() as i32) as f64;
    sprite(cells, &FLAMINGO, w * 0.25, flam_y, &flamingo, false, 1.0);
    let nod = if (t * 0.8).sin() > 0.7 { 1.0 } else { 0.0 };
    sprite(
        cells,
        &FLAMINGO,
        w * 0.25 + 6.0,
        flam_y + nod,
        &flamingo,
        true,
        1.0,
    );
    cells.fill(
        0,
        layout.road_top,
        frame.width,
        ROAD_HEIGHT,
        if dark { "#16101f" } else { "#c4b4c8" },
        1.0,
    );
    cells.fill(
        0,
        layout.road_bottom,
        frame.width,
        frame.height - layout.road_bottom,
        sand,
        1.0,
    );
    let curb_alpha = if dark { 0.8 } else { 0.9 };
    line(
        cells,
        0.0,
        layout.road_top as f64,
        w,
        layout.road_top as f64,
        curb,
        curb_alpha,
    );
    line(
        cells,
        0.0,
        layout.road_bottom as f64,
        w,
        layout.road_bottom as f64,
        curb,
        curb_alpha,
    );
    let dash_color = if dark { "#ffd27a" } else { "#fff6e0" };
    let mut x = 0_i32;
    while x < frame.width {
        line(
            cells,
            x as f64,
            layout.lane,
            (x + 3) as f64,
            layout.lane,
            dash_color,
            0.7,
        );
        x += 8;
    }
}

fn coast_chase<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let t = frame.time;
    let dark = frame.dark;
    let local = layout.crash_at + layout.after;
    let crashed = layout.after > 0.0;
    let recoil = layout.after.clamp(0.0, 0.2) * 10.0;
    let lead_x = -20.0 + layout.speed * local;
    let police_x = if crashed {
        layout.crash_x as f64 - 12.0 - recoil
    } else {
        -40.0 + layout.speed * local
    };
    let police_y =
        js_round(layout.lane - 6.0 * clamp01((local - layout.crash_at + 0.7) / 0.7)) as f64;
    let oncoming_x = if crashed {
        layout.crash_x as f64 + recoil
    } else {
        layout.crash_x as f64 + 12.0 * (layout.crash_at - local)
    };
    let glass = if dark { "#9fd8ff" } else { "#e9f6ff" };
    let wheels = "#140c18";
    let burnt = if dark { "#5a4048" } else { "#5a4a50" };
    let wreck = [
        ('R', burnt),
        ('W', burnt),
        ('K', wheels),
        ('L', burnt),
        ('H', burnt),
        ('A', burnt),
        ('B', burnt),
        ('G', burnt),
        ('T', wheels),
    ];
    let flash = ((t * 6.0).floor() as i64).rem_euclid(2) == 0;
    let oncoming_live = [
        ('R', if dark { "#3fd9d0" } else { "#3aaeb8" }),
        ('W', glass),
        ('L', "#ff3b3b"),
        ('H', "#fff3b0"),
        ('K', wheels),
    ];
    sprite(
        cells,
        &COAST_CAR,
        oncoming_x,
        layout.lane - 5.0,
        if crashed { &wreck } else { &oncoming_live },
        true,
        if crashed { layout.fade } else { 1.0 },
    );
    let lead_palette = [
        ('R', if dark { "#ff4fb8" } else { "#f0609e" }),
        ('W', glass),
        ('L', "#ff3b3b"),
        ('H', "#fff3b0"),
        ('K', wheels),
    ];
    sprite(
        cells,
        &COAST_CAR,
        lead_x,
        layout.lane + 1.0,
        &lead_palette,
        false,
        1.0,
    );
    if dark {
        headlights(
            cells,
            lead_x + COAST_CAR[0].len() as f64,
            layout.lane + 3.0,
            1.0,
        );
        if !crashed {
            disc(
                cells,
                police_x + 4.5,
                police_y,
                4.0,
                if flash { "#ff2a4a" } else { "#3a6aff" },
                0.2,
            );
            headlights(cells, oncoming_x - 1.0, layout.lane - 3.0, -1.0);
            headlights(
                cells,
                police_x + POLICE_CAR[0].len() as f64,
                police_y + 3.0,
                1.0,
            );
        }
    }
    let police_live = [
        ('A', if flash { "#ff2a4a" } else { "#5a1020" }),
        ('B', if flash { "#1c2a6a" } else { "#3a6aff" }),
        ('W', if dark { "#e9e4f0" } else { "#ffffff" }),
        ('G', glass),
        ('K', if dark { "#1a1a2e" } else { "#2a2a40" }),
        ('L', "#ff3b3b"),
        ('H', "#fff3b0"),
        ('T', wheels),
    ];
    sprite(
        cells,
        &POLICE_CAR,
        police_x,
        police_y,
        if crashed { &wreck } else { &police_live },
        false,
        if crashed { layout.fade } else { 1.0 },
    );
}

fn coast_explosion<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let t = frame.time;
    let dark = frame.dark;
    let after = layout.after;
    let fade = layout.fade;
    if after <= 0.0 {
        return;
    }
    let x = layout.crash_x as f64;
    let y = layout.lane - 4.0;
    if after < 0.25 {
        disc(
            cells,
            x,
            y,
            4.0 + after * 60.0,
            "#fff6d8",
            0.8 * (1.0 - after / 0.25),
        );
    }
    if after > 0.4 {
        disc(
            cells,
            x,
            y - 1.0,
            12.0 + (t * 9.0).sin() * 1.5,
            "#ff8a1f",
            (if dark { 0.16 } else { 0.1 }) * fade,
        );
    }
    if after < 1.6 {
        let shrink = clamp01((1.6 - after) / 1.2);
        let radius = 13.0 * (after / 0.4).min(1.0) * shrink + 1.0;
        disc(cells, x, y - 1.0, radius + 2.0, "#ff3b1f", 0.55 * shrink);
        disc(cells, x, y - 1.0, radius, "#ff8a1f", 0.85 * shrink);
        disc(cells, x, y - 1.0, radius * 0.55, "#ffe14a", shrink);
    }
    if after > 0.4 {
        for index in 0..14 {
            let idx = index as f64;
            let fire_x = x - 10.0 + hash(idx + 890.0) * 20.0;
            let height = 1.0 + hash(idx + (t * 10.0).floor()) * 4.0;
            let mut step = 0_usize;
            while (step as f64) < height {
                let flame_color = FLAMES[2 - step.min(2)];
                cell(
                    cells,
                    fire_x,
                    y - 1.0 - step as f64,
                    flame_color,
                    fade * (1.0 - step as f64 / 6.0),
                );
                step += 1;
            }
        }
    }
    let debris = if dark { "#7a5a60" } else { "#5a4a50" };
    for piece in 0..16 {
        let p = piece as f64;
        let vx = (hash(p + 880.0) - 0.5) * 36.0;
        let vy = -(8.0 + hash(p + 881.0) * 18.0);
        let ground = layout.lane - 2.0 + hash(p + 882.0) * 6.0;
        let landing = (-vy + (vy * vy + 80.0 * (ground - y)).max(0.0).sqrt()) / 40.0;
        let flight = after.min(landing);
        let color = if piece % 3 == 0 && after < landing {
            flame(t, p)
        } else {
            debris
        };
        cell(
            cells,
            x + vx * flight,
            y + vy * flight + 20.0 * flight * flight,
            color,
            fade,
        );
    }
    if after < 0.3 {
        return;
    }
    let smoke = if dark { "#8a7a90" } else { "#8a8290" };
    for puff in 0..18 {
        let p = puff as f64;
        let age = wrap(after * 0.9 + (p / 18.0) * 4.0, 4.0);
        if age > after {
            continue;
        }
        let puff_x = x + (age * 1.5 + p).sin() * 2.0 + age * 4.0;
        disc(
            cells,
            puff_x,
            y - 4.0 - age * 8.0,
            1.5 + age * 1.5,
            smoke,
            0.45 * (1.0 - age / 4.0) * fade,
        );
    }
}

fn coast_palms<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
    layout: &CoastLayout,
) {
    let w = frame.width as f64;
    let h = frame.height as f64;
    let ground =
        (layout.road_bottom + js_round((frame.height - layout.road_bottom) as f64 * 0.35)) as f64;
    let burning = clamp01((layout.after - 1.2) / 0.5) * layout.fade;
    let palms: [[f64; 3]; 4] = [
        [0.035, 0.34, 0.6],
        [0.1, 0.25, -0.4],
        [0.9, 0.27, 0.5],
        [0.965, 0.36, -0.6],
    ];
    for (index, &[px_norm, size_norm, lean]) in palms.iter().enumerate() {
        let base = PalmBase {
            x: px_norm * w,
            y: ground,
            height: js_round(h.min(w * 0.75) * size_norm).max(8),
            lean,
            seed: 850.0 + (index * 7) as f64,
        };
        palm(cells, frame, base, if index == 2 { burning } else { 0.0 });
    }
}

struct ShakenCells<'a, C: ChatBackdropCells + ?Sized> {
    inner: &'a mut C,
    dx: i32,
    dy: i32,
}

impl<C: ChatBackdropCells + ?Sized> ChatBackdropCells for ShakenCells<'_, C> {
    fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, color: &str, alpha: f64) {
        self.inner
            .fill(x + self.dx, y + self.dy, width, height, color, alpha);
    }
}

fn paint_coast<C: ChatBackdropCells + ?Sized>(base: &mut C, frame: &ChatBackdropFrame) {
    let layout = coast_layout(frame);
    coast_sky(base, frame, &layout);
    coast_sun(base, frame, &layout);
    coast_skyline(base, frame, &layout);
    coast_yacht(base, frame, &layout);
    coast_helicopter(base, frame, &layout);
    coast_speedboat(base, frame, &layout);
    coast_beach(base, frame, &layout);

    let strength = if layout.after > 0.0 && layout.after < 0.7 {
        (1.0 - layout.after / 0.7) * 2.5
    } else {
        0.0
    };
    if strength == 0.0 {
        coast_chase(base, frame, &layout);
        coast_palms(base, frame, &layout);
        coast_explosion(base, frame, &layout);
    } else {
        let mut shaken = ShakenCells {
            inner: base,
            dx: js_round((frame.time * 90.0).sin() * strength),
            dy: js_round((frame.time * 77.0).cos() * strength),
        };
        coast_chase(&mut shaken, frame, &layout);
        coast_palms(&mut shaken, frame, &layout);
        coast_explosion(&mut shaken, frame, &layout);
    }
}

pub fn paint_chat_backdrop_scene<C: ChatBackdropCells + ?Sized>(
    cells: &mut C,
    frame: &ChatBackdropFrame,
) {
    let scheme = if frame.dark {
        OpenBotColorScheme::Dark
    } else {
        OpenBotColorScheme::Light
    };
    cells.fill(
        0,
        0,
        frame.width,
        frame.height,
        frame.scene.base_hex(scheme),
        1.0,
    );
    match frame.scene {
        ChatBackdropScene::Pond => paint_pond(cells, frame),
        ChatBackdropScene::Jellyfish => paint_jellyfish(cells, frame),
        ChatBackdropScene::Sunset => paint_sunset(cells, frame),
        ChatBackdropScene::Snowfall => paint_snowfall(cells, frame),
        ChatBackdropScene::Fireflies => paint_fireflies(cells, frame),
        ChatBackdropScene::Coast => paint_coast(cells, frame),
    }
}

/// Evaluates the 4-stop horizontal `CALM_CENTER = 0.55` overlay alpha (`0 -> 0.30 -> 0.62 -> 0.90 -> 1.0`).
pub fn calm_center_alpha_at(u: f64) -> f64 {
    let t = clamp01(u);
    let stop_alpha = if t <= 0.30 || t >= 0.90 {
        0.0
    } else if t <= 0.62 {
        (t - 0.30) / 0.32
    } else {
        (0.90 - t) / 0.28
    };
    BACKDROP_CALM_CENTER_ALPHA * stop_alpha
}

pub(crate) fn parse_backdrop_css_color(raw: &str) -> Option<[u8; 4]> {
    let s = raw.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some([r, g, b, 255])
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some([r, g, b, a])
            }
            _ => None,
        };
    }
    let body = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = body
        .split(|ch: char| ch == ',' || ch.is_ascii_whitespace() || ch == '/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let r = parts[0].parse::<f64>().ok()?.round().clamp(0.0, 255.0) as u8;
    let g = parts[1].parse::<f64>().ok()?.round().clamp(0.0, 255.0) as u8;
    let b = parts[2].parse::<f64>().ok()?.round().clamp(0.0, 255.0) as u8;
    let a = if let Some(alpha_str) = parts.get(3) {
        (alpha_str.parse::<f64>().ok()?.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        255
    };
    Some([r, g, b, a])
}

struct RasterCanvas {
    width: i32,
    height: i32,
    rgba_pixels: Vec<u8>,
    base_filled: bool,
    draw_cells: usize,
}

impl RasterCanvas {
    fn new(width: u32, height: u32) -> Self {
        let len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        Self {
            width: width as i32,
            height: height as i32,
            rgba_pixels: vec![0_u8; len],
            base_filled: false,
            draw_cells: 0,
        }
    }

    fn blend_rect(&mut self, x: i32, y: i32, w: i32, h: i32, src: [u8; 4], alpha: f64) {
        if w <= 0 || h <= 0 || !alpha.is_finite() || alpha <= 0.0 {
            return;
        }
        let eff_a = (src[3] as f64 / 255.0) * alpha.clamp(0.0, 1.0);
        if eff_a <= 0.0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let stride = (self.width as usize) * 4;
        if eff_a >= 0.999_9 {
            let opaque = [src[0], src[1], src[2], 255];
            for py in y0..y1 {
                let row_off = (py as usize) * stride;
                for px_i in x0..x1 {
                    let off = row_off + (px_i as usize) * 4;
                    self.rgba_pixels[off..off + 4].copy_from_slice(&opaque);
                }
            }
            return;
        }
        let inv_a = 1.0 - eff_a;
        let sr = (src[0] as f64) * eff_a;
        let sg = (src[1] as f64) * eff_a;
        let sb = (src[2] as f64) * eff_a;
        for py in y0..y1 {
            let row_off = (py as usize) * stride;
            for px_i in x0..x1 {
                let off = row_off + (px_i as usize) * 4;
                let dr = self.rgba_pixels[off] as f64;
                let dg = self.rgba_pixels[off + 1] as f64;
                let db = self.rgba_pixels[off + 2] as f64;
                let da = self.rgba_pixels[off + 3] as f64 / 255.0;
                let out_a = eff_a + da * inv_a;
                if out_a > 0.0 {
                    self.rgba_pixels[off] = ((sr + dr * da * inv_a) / out_a + 0.5)
                        .floor()
                        .clamp(0.0, 255.0) as u8;
                    self.rgba_pixels[off + 1] = ((sg + dg * da * inv_a) / out_a + 0.5)
                        .floor()
                        .clamp(0.0, 255.0) as u8;
                    self.rgba_pixels[off + 2] = ((sb + db * da * inv_a) / out_a + 0.5)
                        .floor()
                        .clamp(0.0, 255.0) as u8;
                    self.rgba_pixels[off + 3] =
                        (out_a * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    fn apply_calm_center(&mut self, base_hex: &str) {
        let Some(base_rgb) = parse_backdrop_css_color(base_hex) else {
            return;
        };
        let w = self.width.max(1);
        for gx in 0..self.width {
            let u = if w >= 6 {
                (gx as f64 + 0.5) / (w as f64)
            } else {
                (gx as f64) / ((w - 1).max(1) as f64)
            };
            let a = calm_center_alpha_at(u);
            if a > 0.0 {
                self.blend_rect(gx, 0, 1, self.height, base_rgb, a);
            }
        }
    }
}

impl ChatBackdropCells for RasterCanvas {
    fn fill(&mut self, x: i32, y: i32, width: i32, height: i32, color: &str, alpha: f64) {
        let Some(parsed) = parse_backdrop_css_color(color) else {
            return;
        };
        if !self.base_filled && x == 0 && y == 0 && width == self.width && height == self.height {
            self.base_filled = true;
        } else if width > 0 && height > 0 && alpha.is_finite() && alpha > 0.0 {
            self.draw_cells = self.draw_cells.saturating_add(1);
        }
        self.blend_rect(x, y, width, height, parsed, alpha);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatBackdropRaster {
    pub scene: ChatBackdropScene,
    pub scheme: OpenBotColorScheme,
    pub grid_w: u32,
    pub grid_h: u32,
    pub cell_px: u32,
    pub base_hex: &'static str,
    pub calm_center_alpha: f64,
    pub animated: bool,
    pub time_sec: f64,
    pub draw_cells: usize,
    pub rgba_pixels: Vec<u8>,
}

impl ChatBackdropRaster {
    /// Converts `rgba_pixels` (`RGBA8`) into `BGRA8` byte order required by `gpui::RenderImage`.
    pub fn to_bgra_bytes(&self) -> Vec<u8> {
        let mut bgra = self.rgba_pixels.clone();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        bgra
    }

    /// Wraps the `(grid_w x grid_h)` BGRA8 buffer in a `gpui::RenderImage`.
    pub fn to_gpui_render_image(&self) -> Arc<RenderImage> {
        let bgra = self.to_bgra_bytes();
        let img_buf = image::RgbaImage::from_raw(self.grid_w.max(1), self.grid_h.max(1), bgra)
            .unwrap_or_else(|| image::RgbaImage::new(1, 1));
        Arc::new(RenderImage::new(vec![image::Frame::new(img_buf)]))
    }

    /// Nearest-neighbor upscales each grid cell by `cell_px` (`6px`) into a crisp BGRA8 `RenderImage`.
    pub fn to_pixelated_gpui_render_image(&self) -> Arc<RenderImage> {
        let scale = self.cell_px.max(1) as usize;
        let out_w = (self.grid_w.max(1) as usize).saturating_mul(scale);
        let out_h = (self.grid_h.max(1) as usize).saturating_mul(scale);
        let mut bgra = vec![0_u8; out_w * out_h * 4];
        let gw = self.grid_w.max(1) as usize;
        for gy in 0..(self.grid_h.max(1) as usize) {
            for gx in 0..gw {
                let src_off = (gy * gw + gx) * 4;
                let px_bgra = [
                    self.rgba_pixels[src_off + 2],
                    self.rgba_pixels[src_off + 1],
                    self.rgba_pixels[src_off],
                    self.rgba_pixels[src_off + 3],
                ];
                for sy in 0..scale {
                    let row_start = ((gy * scale + sy) * out_w + gx * scale) * 4;
                    for sx in 0..scale {
                        let dst = row_start + sx * 4;
                        bgra[dst..dst + 4].copy_from_slice(&px_bgra);
                    }
                }
            }
        }
        let img_buf = image::RgbaImage::from_raw(out_w as u32, out_h as u32, bgra)
            .unwrap_or_else(|| image::RgbaImage::new(1, 1));
        Arc::new(RenderImage::new(vec![image::Frame::new(img_buf)]))
    }
}

pub fn rasterize_chat_backdrop(
    scene: ChatBackdropScene,
    scheme: OpenBotColorScheme,
    viewport_w_px: u32,
    viewport_h_px: u32,
    elapsed_sec: f64,
    reduced_motion: bool,
) -> ChatBackdropRaster {
    let grid_w = ((viewport_w_px.max(1) as f64) / (BACKDROP_CELL_PX as f64))
        .ceil()
        .max(1.0) as u32;
    let grid_h = ((viewport_h_px.max(1) as f64) / (BACKDROP_CELL_PX as f64))
        .ceil()
        .max(1.0) as u32;
    let time_sec = if reduced_motion {
        BACKDROP_STILL_TIME
    } else if elapsed_sec.is_finite() && elapsed_sec >= 0.0 {
        elapsed_sec
    } else {
        0.0
    };
    let base_hex = scene.base_hex(scheme);
    let frame = ChatBackdropFrame {
        scene,
        width: grid_w as i32,
        height: grid_h as i32,
        time: time_sec,
        dark: scheme == OpenBotColorScheme::Dark,
    };
    let mut canvas_buf = RasterCanvas::new(grid_w, grid_h);
    paint_chat_backdrop_scene(&mut canvas_buf, &frame);
    canvas_buf.apply_calm_center(base_hex);
    ChatBackdropRaster {
        scene,
        scheme,
        grid_w,
        grid_h,
        cell_px: BACKDROP_CELL_PX,
        base_hex,
        calm_center_alpha: BACKDROP_CALM_CENTER_ALPHA,
        animated: !reduced_motion,
        time_sec,
        draw_cells: canvas_buf.draw_cells,
        rgba_pixels: canvas_buf.rgba_pixels,
    }
}

fn dedup_backdrop_animation_frame(window: &Window) {
    thread_local! {
        static LAST_BACKDROP_ANIM: RefCell<HashMap<EntityId, Instant>> = RefCell::new(HashMap::new());
    }
    let view_id = window.current_view();
    let now = Instant::now();
    let gate = Duration::from_millis(BACKDROP_FRAME_MS / 2);
    let should_req = LAST_BACKDROP_ANIM.with(|cell| {
        let mut map = cell.borrow_mut();
        if map.len() > 32 {
            let stale = Duration::from_secs(5);
            map.retain(|_, ts| now.saturating_duration_since(*ts) <= stale);
        }
        let allow = map
            .get(&view_id)
            .is_none_or(|prev| now.saturating_duration_since(*prev) >= gate);
        if allow {
            map.insert(view_id, now);
        }
        allow
    });
    if should_req {
        window.request_animation_frame();
    }
}

fn paint_backdrop_raster_paths(
    bounds: Bounds<Pixels>,
    raster: &ChatBackdropRaster,
    window: &mut Window,
) {
    let origin_x = f32::from(bounds.origin.x);
    let origin_y = f32::from(bounds.origin.y);
    let max_x = origin_x + f32::from(bounds.size.width);
    let max_y = origin_y + f32::from(bounds.size.height);
    let cell_f = raster.cell_px.max(1) as f32;
    let gw = raster.grid_w as usize;
    let gh = raster.grid_h as usize;

    let mut color_spans: HashMap<u32, Vec<(f32, f32, f32, f32)>> = HashMap::new();
    let mut color_order: Vec<u32> = Vec::new();

    for gy in 0..gh {
        let y0 = origin_y + (gy as f32) * cell_f;
        if y0 >= max_y {
            break;
        }
        let y1 = (y0 + cell_f).min(max_y);
        let mut gx = 0_usize;
        while gx < gw {
            let off = (gy * gw + gx) * 4;
            let packed = ((raster.rgba_pixels[off] as u32) << 24)
                | ((raster.rgba_pixels[off + 1] as u32) << 16)
                | ((raster.rgba_pixels[off + 2] as u32) << 8)
                | (raster.rgba_pixels[off + 3] as u32);
            let run_start = gx;
            gx += 1;
            while gx < gw {
                let next_off = (gy * gw + gx) * 4;
                let next_packed = ((raster.rgba_pixels[next_off] as u32) << 24)
                    | ((raster.rgba_pixels[next_off + 1] as u32) << 16)
                    | ((raster.rgba_pixels[next_off + 2] as u32) << 8)
                    | (raster.rgba_pixels[next_off + 3] as u32);
                if next_packed != packed {
                    break;
                }
                gx += 1;
            }
            let x0 = origin_x + (run_start as f32) * cell_f;
            if x0 >= max_x {
                break;
            }
            let x1 = (origin_x + (gx as f32) * cell_f).min(max_x);
            let entry = color_spans.entry(packed).or_insert_with(|| {
                color_order.push(packed);
                Vec::new()
            });
            entry.push((x0, y0, x1, y1));
        }
    }

    for packed in color_order {
        let Some(spans) = color_spans.get(&packed) else {
            continue;
        };
        let mut builder = PathBuilder::fill();
        for &(x0, y0, x1, y1) in spans {
            builder.move_to(point(px(x0), px(y0)));
            builder.line_to(point(px(x1), px(y0)));
            builder.line_to(point(px(x1), px(y1)));
            builder.line_to(point(px(x0), px(y1)));
            builder.close();
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, rgba(packed));
        }
    }
}

pub fn render_chat_backdrop_at(
    scene: ChatBackdropScene,
    scheme: OpenBotColorScheme,
    elapsed_override_sec: Option<f64>,
    reduced_motion: bool,
) -> gpui::Div {
    static BACKDROP_START: OnceLock<Instant> = OnceLock::new();
    let base = parse_backdrop_css_color(scene.base_hex(scheme)).unwrap_or([15, 36, 38, 255]);
    let base_packed = ((base[0] as u32) << 24)
        | ((base[1] as u32) << 16)
        | ((base[2] as u32) << 8)
        | (base[3] as u32);

    div()
        .absolute()
        .inset_0()
        .overflow_hidden()
        .bg(rgba(base_packed))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    let vw = f32::from(bounds.size.width).max(6.0).ceil() as u32;
                    let vh = f32::from(bounds.size.height).max(6.0).ceil() as u32;
                    let elapsed_sec = if reduced_motion {
                        BACKDROP_STILL_TIME
                    } else if let Some(explicit) = elapsed_override_sec {
                        explicit
                    } else {
                        let start = BACKDROP_START.get_or_init(Instant::now);
                        let ms = start.elapsed().as_millis() as u64;
                        let quantized_ms = (ms / BACKDROP_FRAME_MS) * BACKDROP_FRAME_MS;
                        (quantized_ms as f64) / 1000.0
                    };
                    let raster =
                        rasterize_chat_backdrop(scene, scheme, vw, vh, elapsed_sec, reduced_motion);
                    paint_backdrop_raster_paths(bounds, &raster, window);
                    if !reduced_motion && elapsed_override_sec.is_none() {
                        dedup_backdrop_animation_frame(window);
                    }
                },
            )
            .size_full(),
        )
}

pub fn render_chat_backdrop(
    scene: ChatBackdropScene,
    scheme: OpenBotColorScheme,
    reduced_motion: bool,
) -> gpui::Div {
    render_chat_backdrop_at(scene, scheme, None, reduced_motion)
}

pub const CUSTOM_BACKDROP_IMAGE_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
];
pub const CUSTOM_BACKDROP_VIDEO_TYPES: [&str; 2] = ["video/mp4", "video/webm"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomBackdropKind {
    Image,
    Video,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomBackdropError {
    UnsupportedOrOversized {
        mime_type: String,
        size_bytes: usize,
    },
    NotFound(String),
    DecodeFailed(String),
}

impl std::fmt::Display for CustomBackdropError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedOrOversized {
                mime_type,
                size_bytes,
            } => write!(
                f,
                "This file cannot be a backdrop (mime={mime_type}, bytes={size_bytes})."
            ),
            Self::NotFound(id) => write!(f, "Custom backdrop not found: {id}"),
            Self::DecodeFailed(reason) => write!(f, "Failed to decode custom backdrop: {reason}"),
        }
    }
}

impl std::error::Error for CustomBackdropError {}

pub fn custom_backdrop_kind(mime_type: &str, size_bytes: usize) -> Option<CustomBackdropKind> {
    let normalized = mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if CUSTOM_BACKDROP_IMAGE_TYPES.contains(&normalized.as_str()) {
        return (size_bytes <= CUSTOM_BACKDROP_IMAGE_LIMIT).then_some(CustomBackdropKind::Image);
    }
    if CUSTOM_BACKDROP_VIDEO_TYPES.contains(&normalized.as_str()) {
        return (size_bytes <= CUSTOM_BACKDROP_VIDEO_LIMIT).then_some(CustomBackdropKind::Video);
    }
    if normalized == "text/html" {
        return (size_bytes <= CUSTOM_BACKDROP_PAGE_LIMIT).then_some(CustomBackdropKind::Page);
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatBackdropBubbles {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yours: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<String>,
}

fn is_avatar_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.as_bytes()[1..]
            .iter()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn is_valid_requester_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'@' | b'-'))
}

fn find_meta_tag_by_name(html: &str, target_name: &str) -> Option<(usize, usize, Option<String>)> {
    let bytes = html.as_bytes();
    let mut cursor = 0_usize;
    while cursor + 5 < bytes.len() {
        let Some(rel_lt) = html[cursor..].find('<') else {
            break;
        };
        let lt = cursor + rel_lt;
        if lt + 5 >= bytes.len() {
            break;
        }
        let after_lt = &html[lt + 1..];
        if after_lt.len() < 4 || !after_lt[..4].eq_ignore_ascii_case("meta") {
            cursor = lt + 1;
            continue;
        }
        let boundary = bytes[lt + 5];
        if !boundary.is_ascii_whitespace() && boundary != b'/' && boundary != b'>' {
            cursor = lt + 5;
            continue;
        }
        let mut idx = lt + 5;
        let mut quote: Option<u8> = None;
        let mut gt_opt = None;
        while idx < bytes.len() {
            let b = bytes[idx];
            if let Some(q) = quote {
                if b == q {
                    quote = None;
                }
            } else if b == b'"' || b == b'\'' {
                quote = Some(b);
            } else if b == b'>' {
                gt_opt = Some(idx);
                break;
            }
            idx += 1;
        }
        let Some(gt) = gt_opt else {
            break;
        };
        let attrs = &html[lt + 5..gt];
        let (meta_name, meta_content) = parse_meta_name_and_content(attrs);
        if meta_name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(target_name))
        {
            return Some((lt, gt + 1, meta_content));
        }
        cursor = gt + 1;
    }
    None
}

fn parse_meta_name_and_content(attrs: &str) -> (Option<String>, Option<String>) {
    let bytes = attrs.as_bytes();
    let mut i = 0_usize;
    let mut found_name = None;
    let mut found_content = None;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'-' | b':'))
        {
            i += 1;
        }
        if i == name_start {
            i += 1;
            continue;
        }
        let attr_key = &attrs[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let attr_val = if bytes[i] == b'"' || bytes[i] == b'\'' {
            let q = bytes[i];
            i += 1;
            let val_start = i;
            while i < bytes.len() && bytes[i] != q {
                i += 1;
            }
            let val = &attrs[val_start..i];
            if i < bytes.len() {
                i += 1;
            }
            Some(val.to_string())
        } else {
            let val_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'/' {
                i += 1;
            }
            Some(attrs[val_start..i].to_string())
        };
        if attr_key.eq_ignore_ascii_case("name") {
            found_name = attr_val;
        } else if attr_key.eq_ignore_ascii_case("content") {
            found_content = attr_val;
        }
    }
    (found_name, found_content)
}

fn with_leading_meta(page: &str, tag: &str) -> String {
    let trimmed = page.trim_start();
    let ws_len = page.len() - trimmed.len();
    if trimmed.len() >= 9 && trimmed[..9].eq_ignore_ascii_case("<!doctype") {
        if let Some(rel_gt) = trimmed.find('>') {
            let end = ws_len + rel_gt + 1;
            return format!("{}{}{}", &page[..end], tag, &page[end..]);
        }
    }
    format!("{tag}{page}")
}

/// Parses `<meta name="openbot-bubbles" content="yours=#rrggbb; agents=#rrggbb">` from HTML.
pub fn parse_chat_backdrop_bubbles(html: &str) -> Option<ChatBackdropBubbles> {
    let (_, _, content_opt) = find_meta_tag_by_name(html, "openbot-bubbles")?;
    let content = content_opt?;
    let mut bubbles = ChatBackdropBubbles::default();
    for part in content.split(';') {
        let Some((raw_k, raw_v)) = part.split_once('=') else {
            continue;
        };
        let key = raw_k.trim().to_ascii_lowercase();
        let color = raw_v.trim().to_ascii_lowercase();
        if !is_avatar_hex_color(&color) {
            continue;
        }
        match key.as_str() {
            "yours" => bubbles.yours = Some(color),
            "agents" => bubbles.agents = Some(color),
            _ => {}
        }
    }
    if bubbles.yours.is_none() && bubbles.agents.is_none() {
        None
    } else {
        Some(bubbles)
    }
}

pub fn with_chat_backdrop_bubbles(html: &str, bubbles: &ChatBackdropBubbles) -> String {
    let page = if let Some((start, end, _)) = find_meta_tag_by_name(html, "openbot-bubbles") {
        format!("{}{}", &html[..start], &html[end..])
    } else {
        html.to_string()
    };
    let mut parts = Vec::new();
    if let Some(ref yours) = bubbles.yours {
        parts.push(format!("yours={yours}"));
    }
    if let Some(ref agents) = bubbles.agents {
        parts.push(format!("agents={agents}"));
    }
    if parts.is_empty() {
        return page;
    }
    let tag = format!(
        "<meta name=\"openbot-bubbles\" content=\"{}\">",
        parts.join("; ")
    );
    with_leading_meta(&page, &tag)
}

/// Parses `<meta name="openbot-backdrop-for" content="...">` requester IDs (default `["signed-out"]`).
pub fn parse_chat_backdrop_requesters(html: &str) -> Vec<String> {
    let ids: Vec<String> = find_meta_tag_by_name(html, "openbot-backdrop-for")
        .and_then(|(_, _, content)| content)
        .map(|raw| {
            raw.split_whitespace()
                .filter(|token| is_valid_requester_id(token))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        vec!["signed-out".to_string()]
    } else {
        ids
    }
}

pub fn with_chat_backdrop_requesters(html: &str, ids: &[String]) -> String {
    let mut page = html.to_string();
    while let Some((start, end, _)) = find_meta_tag_by_name(&page, "openbot-backdrop-for") {
        page = format!("{}{}", &page[..start], &page[end..]);
    }
    let filtered: Vec<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| is_valid_requester_id(id))
        .collect();
    let content = if filtered.is_empty() {
        "signed-out".to_string()
    } else {
        filtered.join(" ")
    };
    let tag = format!("<meta name=\"openbot-backdrop-for\" content=\"{content}\">");
    with_leading_meta(&page, &tag)
}

pub fn chat_bubble_color<'a>(
    backdrop: Option<&'a ChatBackdropBubbles>,
    author_kind: &str,
    own_chat_color: Option<&'a str>,
    agent_chat_color: Option<&'a str>,
) -> Option<&'a str> {
    let themed = match author_kind {
        "you" => backdrop.and_then(|b| b.yours.as_deref()),
        "agent" => backdrop.and_then(|b| b.agents.as_deref()),
        _ => None,
    };
    themed.or(own_chat_color).or(agent_chat_color)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomBackdropSummary {
    pub id: String,
    pub name: String,
    pub kind: CustomBackdropKind,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomBackdropEntry {
    pub id: String,
    pub name: String,
    pub kind: CustomBackdropKind,
    pub mime_type: String,
    pub bytes: Vec<u8>,
    pub created_at_ms: u64,
    pub bubbles: Option<ChatBackdropBubbles>,
    pub requesters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CustomBackdropSource {
    Image {
        url: String,
    },
    Video {
        url: String,
        paused: bool,
    },
    Page {
        src: Option<String>,
        html: Option<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct CustomBackdropModel {
    entries: HashMap<String, CustomBackdropEntry>,
    lru_order: VecDeque<String>,
    active_id: Option<String>,
    next_seq: u64,
}

impl CustomBackdropModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn capacity(&self) -> usize {
        CUSTOM_BACKDROP_CAPACITY
    }

    pub const fn scrim_alpha(&self) -> f64 {
        CUSTOM_BACKDROP_SCRIM_ALPHA
    }

    /// Computes `color-mix(in srgb, var(--openbot-bg-canvas) 35%, transparent)` (`chat-backdrop.css:L30`).
    pub fn scrim_css(&self, canvas_hex: &str) -> String {
        let [r, g, b, _] = parse_backdrop_css_color(canvas_hex).unwrap_or([26, 26, 26, 255]);
        format!("rgba({r}, {g}, {b}, 0.35)")
    }

    pub fn set_active_id(&mut self, active_id: Option<String>) {
        if let Some(ref id) = active_id {
            if self.entries.contains_key(id) {
                self.touch_lru(id);
            }
        }
        self.active_id = active_id;
    }

    pub fn active_id(&self) -> Option<&str> {
        self.active_id.as_deref()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn touch_lru(&mut self, id: &str) {
        self.lru_order.retain(|item| item != id);
        self.lru_order.push_back(id.to_string());
    }

    pub fn add(
        &mut self,
        id: Option<String>,
        name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        created_at_ms: u64,
    ) -> Result<String, CustomBackdropError> {
        let Some(kind) = custom_backdrop_kind(mime_type, bytes.len()) else {
            return Err(CustomBackdropError::UnsupportedOrOversized {
                mime_type: mime_type.to_string(),
                size_bytes: bytes.len(),
            });
        };

        self.next_seq = self.next_seq.saturating_add(1);
        let entry_id = id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("custom-backdrop-{}", self.next_seq));

        let trimmed_name: String = name
            .trim()
            .chars()
            .take(CUSTOM_BACKDROP_NAME_LIMIT)
            .collect();
        let resolved_name = if trimmed_name.is_empty() {
            mime_type.to_string()
        } else {
            trimmed_name
        };

        let (bubbles, requesters) = if kind == CustomBackdropKind::Page {
            let html = String::from_utf8_lossy(&bytes);
            (
                parse_chat_backdrop_bubbles(&html),
                parse_chat_backdrop_requesters(&html),
            )
        } else {
            (None, Vec::new())
        };

        if !self.entries.contains_key(&entry_id) {
            while self.entries.len() >= CUSTOM_BACKDROP_CAPACITY {
                let active_ref = self.active_id.as_deref();
                let evict_pos = self
                    .lru_order
                    .iter()
                    .position(|cand| Some(cand.as_str()) != active_ref)
                    .unwrap_or(0);
                if let Some(evicted_id) = self.lru_order.remove(evict_pos) {
                    self.entries.remove(&evicted_id);
                } else {
                    break;
                }
            }
        }

        let entry = CustomBackdropEntry {
            id: entry_id.clone(),
            name: resolved_name,
            kind,
            mime_type: mime_type.to_string(),
            bytes,
            created_at_ms,
            bubbles,
            requesters,
        };
        self.entries.insert(entry_id.clone(), entry);
        self.touch_lru(&entry_id);
        Ok(entry_id)
    }

    pub fn get(&mut self, id: &str) -> Option<&CustomBackdropEntry> {
        if self.entries.contains_key(id) {
            self.touch_lru(id);
        }
        self.entries.get(id)
    }

    pub fn peek(&self, id: &str) -> Option<&CustomBackdropEntry> {
        self.entries.get(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.lru_order.retain(|item| item != id);
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
        }
        self.entries.remove(id).is_some()
    }

    pub fn summaries(&self) -> Vec<CustomBackdropSummary> {
        let mut out: Vec<CustomBackdropSummary> = self
            .entries
            .values()
            .map(|entry| CustomBackdropSummary {
                id: entry.id.clone(),
                name: entry.name.clone(),
                kind: entry.kind,
                created_at: entry.created_at_ms,
            })
            .collect();
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    pub const fn video_should_pause(reduced_motion: bool, document_hidden: bool) -> bool {
        reduced_motion || document_hidden
    }

    pub fn decode_image_render_image(
        entry: &CustomBackdropEntry,
    ) -> Result<Arc<RenderImage>, CustomBackdropError> {
        let decoded = image::load_from_memory(&entry.bytes)
            .map_err(|err| CustomBackdropError::DecodeFailed(err.to_string()))?;
        let mut rgba_buf = decoded.into_rgba8();
        let (w, h) = rgba_buf.dimensions();
        if w == 0 || h == 0 {
            return Err(CustomBackdropError::DecodeFailed(
                "zero-dimension image".to_string(),
            ));
        }
        for pixel in rgba_buf.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Ok(Arc::new(RenderImage::new(vec![image::Frame::new(
            rgba_buf,
        )])))
    }

    pub fn render_custom_backdrop(
        &self,
        id: &str,
        canvas_hex: &str,
        _reduced_motion: bool,
    ) -> gpui::Div {
        let [cr, cg, cb, _] = parse_backdrop_css_color(canvas_hex).unwrap_or([26, 26, 26, 255]);
        let canvas_packed = ((cr as u32) << 24) | ((cg as u32) << 16) | ((cb as u32) << 8) | 0xFF;
        let scrim_alpha_byte = (CUSTOM_BACKDROP_SCRIM_ALPHA * 255.0).round() as u32;
        let scrim_packed = ((cr as u32) << 24)
            | ((cg as u32) << 16)
            | ((cb as u32) << 8)
            | (scrim_alpha_byte & 0xFF);

        let mut root = div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(rgba(canvas_packed));

        if let Some(entry) = self.entries.get(id) {
            if entry.kind == CustomBackdropKind::Image {
                if let Ok(render_img) = Self::decode_image_render_image(entry) {
                    root = root.child(img(render_img).size_full().object_fit(ObjectFit::Cover));
                }
            }
        }

        root.child(div().absolute().inset_0().bg(rgba(scrim_packed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_aliases_and_canonical_scenes_match_spec() {
        let cases = [
            (
                "pond",
                ChatBackdropScene::Pond,
                "pond",
                "#0f2426",
                "#e8f6f3",
            ),
            (
                "jellyfish",
                ChatBackdropScene::Jellyfish,
                "jellyfish",
                "#0e1a2b",
                "#ebf3fa",
            ),
            (
                "forest",
                ChatBackdropScene::Jellyfish,
                "forest",
                "#0e1a2b",
                "#ebf3fa",
            ),
            (
                "sunset",
                ChatBackdropScene::Sunset,
                "sunset",
                "#1d1528",
                "#fbf0f3",
            ),
            (
                "city",
                ChatBackdropScene::Sunset,
                "city",
                "#1d1528",
                "#fbf0f3",
            ),
            (
                "snowfall",
                ChatBackdropScene::Snowfall,
                "snowfall",
                "#111a26",
                "#eef3f7",
            ),
            (
                "snow",
                ChatBackdropScene::Snowfall,
                "snow",
                "#111a26",
                "#eef3f7",
            ),
            (
                "fireflies",
                ChatBackdropScene::Fireflies,
                "fireflies",
                "#121a14",
                "#f2f5ea",
            ),
            (
                "desert",
                ChatBackdropScene::Fireflies,
                "desert",
                "#121a14",
                "#f2f5ea",
            ),
            (
                "coast",
                ChatBackdropScene::Coast,
                "coast",
                "#1a1030",
                "#fdf0ec",
            ),
        ];
        for (input, expected_scene, expected_alias, dark_hex, light_hex) in cases {
            let (scene, alias) = ChatBackdropScene::parse_with_alias(input)
                .unwrap_or_else(|| panic!("expected alias {input} to parse"));
            assert_eq!(scene, expected_scene);
            assert_eq!(alias, expected_alias);
            assert_eq!(scene.base_hex(OpenBotColorScheme::Dark), dark_hex);
            assert_eq!(scene.base_hex(OpenBotColorScheme::Light), light_hex);
        }
        assert!(ChatBackdropScene::parse_with_alias("unknown-scene").is_none());
    }

    #[test]
    fn calm_center_four_stop_gradient_and_reduced_motion_still_time() {
        assert_eq!(calm_center_alpha_at(0.0), 0.0);
        assert_eq!(calm_center_alpha_at(0.30), 0.0);
        assert!((calm_center_alpha_at(0.62) - 0.55).abs() < 1e-12);
        assert_eq!(calm_center_alpha_at(0.90), 0.0);
        assert_eq!(calm_center_alpha_at(1.0), 0.0);

        for scene in CHAT_BACKDROP_SCENES {
            for scheme in [OpenBotColorScheme::Dark, OpenBotColorScheme::Light] {
                let live = rasterize_chat_backdrop(scene, scheme, 480, 300, 1.25, false);
                assert_eq!(live.cell_px, 6);
                assert_eq!(live.grid_w, 80);
                assert_eq!(live.grid_h, 50);
                assert!(live.animated);
                assert!((live.time_sec - 1.25).abs() < 1e-9);
                assert!(live.draw_cells > 0);
                assert_eq!(live.rgba_pixels.len(), 80 * 50 * 4);

                let still = rasterize_chat_backdrop(scene, scheme, 480, 300, 1.25, true);
                assert!(!still.animated);
                assert!((still.time_sec - BACKDROP_STILL_TIME).abs() < 1e-9);
                assert!(still.draw_cells > 0);
            }
        }
    }

    #[test]
    fn coast_chase_crash_shake_and_burn_cycle_phases_render() {
        for &t in &[0.0, 6.5, 13.05, 13.3, 14.5, 18.0, 21.5] {
            let r = rasterize_chat_backdrop(
                ChatBackdropScene::Coast,
                OpenBotColorScheme::Dark,
                600,
                360,
                t,
                false,
            );
            assert!(r.draw_cells > 100);
            let bgra = r.to_bgra_bytes();
            assert_eq!(bgra.len(), r.rgba_pixels.len());
            assert_eq!(bgra[0], r.rgba_pixels[2]);
            assert_eq!(bgra[1], r.rgba_pixels[1]);
            assert_eq!(bgra[2], r.rgba_pixels[0]);
            assert_eq!(bgra[3], r.rgba_pixels[3]);
        }
    }

    #[test]
    fn custom_backdrop_limits_lru_12_scrim_and_bubbles_meta_parser() {
        assert_eq!(
            custom_backdrop_kind("image/webp", CUSTOM_BACKDROP_IMAGE_LIMIT),
            Some(CustomBackdropKind::Image)
        );
        assert_eq!(
            custom_backdrop_kind("image/webp", CUSTOM_BACKDROP_IMAGE_LIMIT + 1),
            None
        );
        assert_eq!(
            custom_backdrop_kind("video/webm", CUSTOM_BACKDROP_VIDEO_LIMIT),
            Some(CustomBackdropKind::Video)
        );
        assert_eq!(
            custom_backdrop_kind("video/webm", CUSTOM_BACKDROP_VIDEO_LIMIT + 1),
            None
        );
        assert_eq!(
            custom_backdrop_kind("text/html; charset=utf-8", CUSTOM_BACKDROP_PAGE_LIMIT),
            Some(CustomBackdropKind::Page)
        );
        assert_eq!(custom_backdrop_kind("image/svg+xml", 128), None);

        let html = "<!DOCTYPE html><meta name=\"openbot-bubbles\" content=\"yours=#1234ab; agents=#CD56ef\"><meta name='openbot-backdrop-for' content='agent-1 agent-2'><body>ok</body>";
        let bubbles = parse_chat_backdrop_bubbles(html).expect("bubbles parsed");
        assert_eq!(bubbles.yours.as_deref(), Some("#1234ab"));
        assert_eq!(bubbles.agents.as_deref(), Some("#cd56ef"));
        assert_eq!(
            parse_chat_backdrop_requesters(html),
            vec!["agent-1".to_string(), "agent-2".to_string()]
        );

        let mut model = CustomBackdropModel::new();
        assert_eq!(model.scrim_css("#1a1a1a"), "rgba(26, 26, 26, 0.35)");
        for i in 0..12 {
            let id = format!("bd-{i:02}");
            model
                .add(
                    Some(id),
                    "Test",
                    "text/html",
                    html.as_bytes().to_vec(),
                    i as u64,
                )
                .expect("within capacity");
        }
        model.set_active_id(Some("bd-00".to_string()));
        model
            .add(
                Some("bd-12".to_string()),
                "Thirteenth",
                "text/html",
                html.as_bytes().to_vec(),
                12,
            )
            .expect("evicts oldest non-active");
        assert_eq!(model.len(), 12);
        assert!(
            model.peek("bd-00").is_some(),
            "active backdrop must not be evicted"
        );
        assert!(
            model.peek("bd-01").is_none(),
            "oldest non-active bd-01 evicted"
        );
        assert!(model.peek("bd-12").is_some());
    }
}
