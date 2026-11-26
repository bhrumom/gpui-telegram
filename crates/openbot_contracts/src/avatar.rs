//! 1:1 Bloub & Plush Bot Avatar contracts, UTF-16 32-bit FNV-1a resolver, hues, moods, and image guards.
//! Mirrors `packages/brand/src/bloub-avatar.ts`, `bloub-avatar-motion.ts`,
//! `packages/ui/src/features/agents/{agent-avatar-mood.ts,AgentAvatar.tsx}`,
//! `packages/contracts/src/{ipc-agent-identity.ts,avatar-images.ts}`, and `@norbert_bodziony/bloub@0.7.0`.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::limits::AVATAR_IMAGE_LIMITS;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AvatarContractError {
    #[error("No avatar seed draws this face.")]
    NoSeedForFace,
    #[error("Invalid avatar hue: {0}")]
    InvalidHue(u16),
}

pub const FNV1A_32_OFFSET_BASIS: u32 = 0x811c_9dc5;
pub const FNV1A_32_PRIME: u32 = 0x0100_0193;
pub const FACE_SEED_TRIES: u32 = 10_000;
pub const WORKING_FACE_MIN_MS: u32 = 700;
pub const WORKING_FACE_MAX_MS: u32 = 3_200;
pub const ACTIVITY_HOLD_MS: u32 = 1_400;
pub const PHOTO_BREATHE: f64 = 0.03;

/// Exact 32-bit FNV-1a over UTF-16 code units matching JS `Math.imul(hash ^ value.charCodeAt(i), 0x01000193) >>> 0`.
/// Does NOT apply any `"openbot"` fallback on empty `""` (`"" -> 0x811c9dc5`).
#[inline]
pub fn fnv1a_utf16_32(value: &str) -> u32 {
    let mut hash = FNV1A_32_OFFSET_BASIS;
    for unit in value.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(FNV1A_32_PRIME);
    }
    hash
}

#[inline]
pub fn stable_index(value: &str, length: usize) -> usize {
    assert!(length > 0, "Bloub avatar options are empty.");
    (fnv1a_utf16_32(value) as usize) % length
}

#[inline]
pub fn avatar_animation_phase(seed: &str) -> f64 {
    (f64::from(fnv1a_utf16_32(seed)) / 4_294_967_296.0) * 1.4
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AvatarStyle {
    Plush,
    Flat,
}

impl AvatarStyle {
    pub const ALL: [Self; 2] = [Self::Plush, Self::Flat];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plush => "plush",
            Self::Flat => "flat",
        }
    }

    pub const fn seed_segment(self) -> &'static str {
        match self {
            Self::Plush => "face",
            Self::Flat => "flat",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "plush" => Some(Self::Plush),
            "flat" => Some(Self::Flat),
            _ => None,
        }
    }
}

/// All 14 `@norbert_bodziony/bloub@0.7.0` engine silhouettes (`ShapeId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineShapeId {
    Cercle,
    Galet,
    Squircle,
    Capsule,
    Triangle,
    Hexagone,
    Nuage,
    Goutte,
    Carre,
    Losange,
    Pentagone,
    Fantome,
    Meduse,
    Poulpe,
}

impl EngineShapeId {
    pub const ALL: [Self; 14] = [
        Self::Cercle,
        Self::Galet,
        Self::Squircle,
        Self::Capsule,
        Self::Triangle,
        Self::Hexagone,
        Self::Nuage,
        Self::Goutte,
        Self::Carre,
        Self::Losange,
        Self::Pentagone,
        Self::Fantome,
        Self::Meduse,
        Self::Poulpe,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cercle => "cercle",
            Self::Galet => "galet",
            Self::Squircle => "squircle",
            Self::Capsule => "capsule",
            Self::Triangle => "triangle",
            Self::Hexagone => "hexagone",
            Self::Nuage => "nuage",
            Self::Goutte => "goutte",
            Self::Carre => "carre",
            Self::Losange => "losange",
            Self::Pentagone => "pentagone",
            Self::Fantome => "fantome",
            Self::Meduse => "meduse",
            Self::Poulpe => "poulpe",
        }
    }

    pub const fn to_supported(self) -> Option<BloubShape> {
        match self {
            Self::Cercle => Some(BloubShape::Cercle),
            Self::Galet => Some(BloubShape::Galet),
            Self::Squircle => Some(BloubShape::Squircle),
            Self::Capsule => Some(BloubShape::Capsule),
            Self::Triangle => Some(BloubShape::Triangle),
            Self::Hexagone => Some(BloubShape::Hexagone),
            Self::Nuage => Some(BloubShape::Nuage),
            Self::Goutte => None,
            Self::Carre => Some(BloubShape::Carre),
            Self::Losange => Some(BloubShape::Losange),
            Self::Pentagone => Some(BloubShape::Pentagone),
            Self::Fantome => Some(BloubShape::Fantome),
            Self::Meduse => Some(BloubShape::Meduse),
            Self::Poulpe => Some(BloubShape::Poulpe),
        }
    }
}

/// 13 brand-supported silhouettes (`Exclude<ShapeId, "goutte">`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BloubShape {
    Cercle,
    Galet,
    Squircle,
    Capsule,
    Triangle,
    Hexagone,
    Nuage,
    Carre,
    Losange,
    Pentagone,
    Fantome,
    Meduse,
    Poulpe,
}

impl BloubShape {
    pub const ALL: [Self; 13] = [
        Self::Cercle,
        Self::Galet,
        Self::Squircle,
        Self::Capsule,
        Self::Triangle,
        Self::Hexagone,
        Self::Nuage,
        Self::Carre,
        Self::Losange,
        Self::Pentagone,
        Self::Fantome,
        Self::Meduse,
        Self::Poulpe,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cercle => "cercle",
            Self::Galet => "galet",
            Self::Squircle => "squircle",
            Self::Capsule => "capsule",
            Self::Triangle => "triangle",
            Self::Hexagone => "hexagone",
            Self::Nuage => "nuage",
            Self::Carre => "carre",
            Self::Losange => "losange",
            Self::Pentagone => "pentagone",
            Self::Fantome => "fantome",
            Self::Meduse => "meduse",
            Self::Poulpe => "poulpe",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == raw)
    }

    pub const fn is_named(self) -> bool {
        matches!(self, Self::Fantome | Self::Meduse | Self::Poulpe)
    }

    pub const fn seeded_lookalike(self) -> Self {
        match self {
            Self::Fantome => Self::Squircle,
            Self::Meduse => Self::Galet,
            Self::Poulpe => Self::Nuage,
            other => other,
        }
    }
}

pub const SUPPORTED_AVATAR_SILHOUETTES: [BloubShape; 13] = BloubShape::ALL;

pub const SEEDED_AVATAR_SILHOUETTES: [EngineShapeId; 11] = [
    EngineShapeId::Cercle,
    EngineShapeId::Galet,
    EngineShapeId::Squircle,
    EngineShapeId::Capsule,
    EngineShapeId::Triangle,
    EngineShapeId::Hexagone,
    EngineShapeId::Nuage,
    EngineShapeId::Goutte,
    EngineShapeId::Carre,
    EngineShapeId::Losange,
    EngineShapeId::Pentagone,
];

pub const SEEDED_SUPPORTED_SILHOUETTES: [BloubShape; 10] = [
    BloubShape::Cercle,
    BloubShape::Galet,
    BloubShape::Squircle,
    BloubShape::Capsule,
    BloubShape::Triangle,
    BloubShape::Hexagone,
    BloubShape::Nuage,
    BloubShape::Carre,
    BloubShape::Losange,
    BloubShape::Pentagone,
];

/// 16 rest expressions (`ExpressionId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BloubExpression {
    Neutre,
    Attentif,
    Surpris,
    Excite,
    Heureux,
    Hilare,
    Colere,
    Triste,
    Effraye,
    Mefiant,
    Confus,
    Curieux,
    Fier,
    Timide,
    Blase,
    Somnolent,
}

impl BloubExpression {
    pub const ALL: [Self; 16] = [
        Self::Neutre,
        Self::Attentif,
        Self::Surpris,
        Self::Excite,
        Self::Heureux,
        Self::Hilare,
        Self::Colere,
        Self::Triste,
        Self::Effraye,
        Self::Mefiant,
        Self::Confus,
        Self::Curieux,
        Self::Fier,
        Self::Timide,
        Self::Blase,
        Self::Somnolent,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Neutre => "neutre",
            Self::Attentif => "attentif",
            Self::Surpris => "surpris",
            Self::Excite => "excite",
            Self::Heureux => "heureux",
            Self::Hilare => "hilare",
            Self::Colere => "colere",
            Self::Triste => "triste",
            Self::Effraye => "effraye",
            Self::Mefiant => "mefiant",
            Self::Confus => "confus",
            Self::Curieux => "curieux",
            Self::Fier => "fier",
            Self::Timide => "timide",
            Self::Blase => "blase",
            Self::Somnolent => "somnolent",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|e| e.as_str() == raw)
    }
}

pub const AVATAR_EXPRESSIONS: [BloubExpression; 16] = BloubExpression::ALL;

/// 15 parametric engine states (`StateId`) with canonical frozen `POSES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BloubParametricState {
    Idle,
    Thinking,
    Wink,
    Wide,
    Alert,
    Notify,
    Exclaim,
    Sleep,
    Egg,
    Hexagon,
    Play,
    Orbit,
    Swirl,
    Burst,
    Comet,
}

impl BloubParametricState {
    pub const ALL: [Self; 15] = [
        Self::Idle,
        Self::Thinking,
        Self::Wink,
        Self::Wide,
        Self::Alert,
        Self::Notify,
        Self::Exclaim,
        Self::Sleep,
        Self::Egg,
        Self::Hexagon,
        Self::Play,
        Self::Orbit,
        Self::Swirl,
        Self::Burst,
        Self::Comet,
    ];

    pub const SEQUENCE: [Self; 14] = [
        Self::Idle,
        Self::Thinking,
        Self::Wink,
        Self::Wide,
        Self::Alert,
        Self::Notify,
        Self::Exclaim,
        Self::Sleep,
        Self::Egg,
        Self::Hexagon,
        Self::Play,
        Self::Orbit,
        Self::Burst,
        Self::Comet,
    ];

    pub const fn canonical_pose_time(self) -> f64 {
        match self {
            Self::Idle => 1.0,
            Self::Thinking => 1.1,
            Self::Wink => 0.8,
            Self::Wide => 0.8,
            Self::Alert => 0.75,
            Self::Notify => 0.9,
            Self::Exclaim => 0.8,
            Self::Sleep => 0.45,
            Self::Egg => 0.8,
            Self::Hexagon => 0.8,
            Self::Play => 0.9,
            Self::Orbit => 1.2,
            Self::Swirl => 0.5,
            Self::Burst => 0.45,
            Self::Comet => 1.15,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShapeSafeStateId {
    Idle,
    Wink,
    Wide,
    Notify,
    Swirl,
}

impl ShapeSafeStateId {
    pub const ALL: [Self; 5] = [Self::Idle, Self::Wink, Self::Wide, Self::Notify, Self::Swirl];

    pub const fn to_parametric(self) -> BloubParametricState {
        match self {
            Self::Idle => BloubParametricState::Idle,
            Self::Wink => BloubParametricState::Wink,
            Self::Wide => BloubParametricState::Wide,
            Self::Notify => BloubParametricState::Notify,
            Self::Swirl => BloubParametricState::Swirl,
        }
    }
}

pub const SHAPE_SAFE_STATES: [ShapeSafeStateId; 5] = ShapeSafeStateId::ALL;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlushState {
    Idle,
    Wink,
    Wide,
    Thinking,
    Notify,
    Asleep,
    Failed,
}

impl PlushState {
    pub const ALL: [Self; 7] = [
        Self::Idle,
        Self::Wink,
        Self::Wide,
        Self::Thinking,
        Self::Notify,
        Self::Asleep,
        Self::Failed,
    ];

    pub const fn frozen_pose_time(self) -> f64 {
        match self {
            Self::Wink => BloubParametricState::Wink.canonical_pose_time(),
            Self::Wide => BloubParametricState::Wide.canonical_pose_time(),
            Self::Notify => BloubParametricState::Notify.canonical_pose_time(),
            _ => BloubParametricState::Idle.canonical_pose_time(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlushTransition {
    Flip,
    Pop,
    Warp,
    Dissolve,
    Cut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AvatarMood {
    Idle,
    Working,
    Waiting,
    Failed,
    Responded,
    Connecting,
    Asleep,
}

impl AvatarMood {
    pub const ALL: [Self; 7] = [
        Self::Idle,
        Self::Working,
        Self::Waiting,
        Self::Failed,
        Self::Responded,
        Self::Connecting,
        Self::Asleep,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
            Self::Responded => "responded",
            Self::Connecting => "connecting",
            Self::Asleep => "asleep",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStepActivity {
    Searching,
    Browsing,
    Reading,
    Running,
    Editing,
    Delegating,
    Working,
    Reviewing,
}

impl AgentStepActivity {
    pub const ALL: [Self; 8] = [
        Self::Searching,
        Self::Browsing,
        Self::Reading,
        Self::Running,
        Self::Editing,
        Self::Delegating,
        Self::Working,
        Self::Reviewing,
    ];

    pub fn parse(raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        match raw {
            "searching" => Some(Self::Searching),
            "browsing" => Some(Self::Browsing),
            "reading" => Some(Self::Reading),
            "running" => Some(Self::Running),
            "editing" => Some(Self::Editing),
            "delegating" => Some(Self::Delegating),
            "working" => Some(Self::Working),
            "reviewing" => Some(Self::Reviewing),
            _ => None,
        }
    }
}

/// All 12 `@norbert_bodziony/bloub@0.7.0` palette colors (`ColorId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BloubColorId {
    Encre,
    Brun,
    Rouge,
    Orange,
    Ambre,
    Vert,
    Turquoise,
    Bleu,
    Violet,
    Rose,
    Gris,
    Creme,
}

impl BloubColorId {
    pub const ALL: [Self; 12] = [
        Self::Encre,
        Self::Brun,
        Self::Rouge,
        Self::Orange,
        Self::Ambre,
        Self::Vert,
        Self::Turquoise,
        Self::Bleu,
        Self::Violet,
        Self::Rose,
        Self::Gris,
        Self::Creme,
    ];

    pub const fn hex(self) -> &'static str {
        match self {
            Self::Encre => "#0a0a0c",
            Self::Brun => "#8b5e3c",
            Self::Rouge => "#e8483f",
            Self::Orange => "#f08a24",
            Self::Ambre => "#f0b429",
            Self::Vert => "#3ecf8e",
            Self::Turquoise => "#2fbfa0",
            Self::Bleu => "#3b93f0",
            Self::Violet => "#8b5cf6",
            Self::Rose => "#e152b0",
            Self::Gris => "#a3a3a3",
            Self::Creme => "#f1efe9",
        }
    }
}

/// 8 automatic seed colors (`AUTOMATIC_COLORS` / `ServerPlushColor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerPlushColor {
    Rouge,
    Orange,
    Ambre,
    Vert,
    Turquoise,
    Bleu,
    Violet,
    Rose,
}

impl ServerPlushColor {
    pub const ALL: [Self; 8] = [
        Self::Rouge,
        Self::Orange,
        Self::Ambre,
        Self::Vert,
        Self::Turquoise,
        Self::Bleu,
        Self::Violet,
        Self::Rose,
    ];

    pub const fn hex(self) -> &'static str {
        match self {
            Self::Rouge => "#e8483f",
            Self::Orange => "#f08a24",
            Self::Ambre => "#f0b429",
            Self::Vert => "#3ecf8e",
            Self::Turquoise => "#2fbfa0",
            Self::Bleu => "#3b93f0",
            Self::Violet => "#8b5cf6",
            Self::Rose => "#e152b0",
        }
    }
}

pub const AUTOMATIC_COLORS: [ServerPlushColor; 8] = ServerPlushColor::ALL;

/// Canonical 10 hues (`0 | 30 | 55 | 100 | 150 | 185 | 215 | 245 | 280 | 320`), serialized as JSON integer `u16`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum AvatarHue {
    Red = 0,
    Orange = 30,
    Yellow = 55,
    Lime = 100,
    Green = 150,
    Cyan = 185,
    Blue = 215,
    Indigo = 245,
    Violet = 280,
    Magenta = 320,
}

impl AvatarHue {
    pub const ALL: [Self; 10] = [
        Self::Red,
        Self::Orange,
        Self::Yellow,
        Self::Lime,
        Self::Green,
        Self::Cyan,
        Self::Blue,
        Self::Indigo,
        Self::Violet,
        Self::Magenta,
    ];

    pub const fn degrees(self) -> u16 {
        self as u16
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Orange => "Orange",
            Self::Yellow => "Yellow",
            Self::Lime => "Lime",
            Self::Green => "Green",
            Self::Cyan => "Cyan",
            Self::Blue => "Blue",
            Self::Indigo => "Indigo",
            Self::Violet => "Violet",
            Self::Magenta => "Magenta",
        }
    }

    pub const fn swatch(self) -> &'static str {
        match self {
            Self::Red => "#e8483f",
            Self::Orange => "#f08a24",
            Self::Yellow => "#f0b429",
            Self::Lime => "#8ece34",
            Self::Green => "#3ecf8e",
            Self::Cyan => "#2fbfa0",
            Self::Blue => "#3b93f0",
            Self::Indigo => "#5d67ee",
            Self::Violet => "#8b5cf6",
            Self::Magenta => "#e152b0",
        }
    }

    pub const fn from_degrees(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::Red),
            30 => Some(Self::Orange),
            55 => Some(Self::Yellow),
            100 => Some(Self::Lime),
            150 => Some(Self::Green),
            185 => Some(Self::Cyan),
            215 => Some(Self::Blue),
            245 => Some(Self::Indigo),
            280 => Some(Self::Violet),
            320 => Some(Self::Magenta),
            _ => None,
        }
    }
}

impl TryFrom<u16> for AvatarHue {
    type Error = AvatarContractError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::from_degrees(value).ok_or(AvatarContractError::InvalidHue(value))
    }
}

impl Serialize for AvatarHue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.degrees())
    }
}

impl<'de> Deserialize<'de> for AvatarHue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u16::deserialize(deserializer)?;
        Self::from_degrees(raw)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid AvatarHue integer: {raw}")))
    }
}

pub const AVATAR_HUES: [AvatarHue; 10] = AvatarHue::ALL;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvatarHueChoice {
    pub hue: AvatarHue,
    pub label: &'static str,
}

pub const AVATAR_HUE_CHOICES: [AvatarHueChoice; 10] = [
    AvatarHueChoice { hue: AvatarHue::Red, label: "Red" },
    AvatarHueChoice { hue: AvatarHue::Orange, label: "Orange" },
    AvatarHueChoice { hue: AvatarHue::Yellow, label: "Yellow" },
    AvatarHueChoice { hue: AvatarHue::Lime, label: "Lime" },
    AvatarHueChoice { hue: AvatarHue::Green, label: "Green" },
    AvatarHueChoice { hue: AvatarHue::Cyan, label: "Cyan" },
    AvatarHueChoice { hue: AvatarHue::Blue, label: "Blue" },
    AvatarHueChoice { hue: AvatarHue::Indigo, label: "Indigo" },
    AvatarHueChoice { hue: AvatarHue::Violet, label: "Violet" },
    AvatarHueChoice { hue: AvatarHue::Magenta, label: "Magenta" },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AvatarColorPresetId {
    Pink,
    Sky,
    Navy,
    Wine,
    Brown,
    Sand,
    Grey,
}

impl AvatarColorPresetId {
    pub const ALL: [Self; 7] = [
        Self::Pink,
        Self::Sky,
        Self::Navy,
        Self::Wine,
        Self::Brown,
        Self::Sand,
        Self::Grey,
    ];

    pub const fn hex(self) -> &'static str {
        match self {
            Self::Pink => "#f27ba5",
            Self::Sky => "#38bdf8",
            Self::Navy => "#34479c",
            Self::Wine => "#a8324e",
            Self::Brown => "#8b5e3c",
            Self::Sand => "#d6b98c",
            Self::Grey => "#a3a3a3",
        }
    }

    pub const fn nearest_hue(self) -> Option<AvatarHue> {
        match self {
            Self::Pink => Some(AvatarHue::Magenta),
            Self::Sky => Some(AvatarHue::Cyan),
            Self::Navy => Some(AvatarHue::Blue),
            Self::Wine => Some(AvatarHue::Red),
            Self::Brown => Some(AvatarHue::Orange),
            Self::Sand => Some(AvatarHue::Orange),
            Self::Grey => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AvatarColorChoice {
    Hue {
        hue: AvatarHue,
        color: Option<&'static str>,
        swatch: &'static str,
    },
    Preset {
        preset: AvatarColorPresetId,
        hue: Option<AvatarHue>,
        color: &'static str,
        swatch: &'static str,
    },
}

const fn hue_choice(hue: AvatarHue) -> AvatarColorChoice {
    AvatarColorChoice::Hue {
        hue,
        color: None,
        swatch: hue.swatch(),
    }
}

const fn preset_choice(preset: AvatarColorPresetId) -> AvatarColorChoice {
    AvatarColorChoice::Preset {
        preset,
        hue: preset.nearest_hue(),
        color: preset.hex(),
        swatch: preset.hex(),
    }
}

pub const AVATAR_COLOR_CHOICES: [AvatarColorChoice; 17] = [
    hue_choice(AvatarHue::Red),
    hue_choice(AvatarHue::Orange),
    hue_choice(AvatarHue::Yellow),
    hue_choice(AvatarHue::Lime),
    hue_choice(AvatarHue::Green),
    hue_choice(AvatarHue::Cyan),
    preset_choice(AvatarColorPresetId::Sky),
    hue_choice(AvatarHue::Blue),
    preset_choice(AvatarColorPresetId::Navy),
    hue_choice(AvatarHue::Indigo),
    hue_choice(AvatarHue::Violet),
    hue_choice(AvatarHue::Magenta),
    preset_choice(AvatarColorPresetId::Pink),
    preset_choice(AvatarColorPresetId::Wine),
    preset_choice(AvatarColorPresetId::Brown),
    preset_choice(AvatarColorPresetId::Sand),
    preset_choice(AvatarColorPresetId::Grey),
];

pub fn is_avatar_color_choice(
    choice: AvatarColorChoice,
    hue: Option<AvatarHue>,
    color: Option<&str>,
) -> bool {
    match choice {
        AvatarColorChoice::Hue { hue: h, .. } => color.is_none() && hue == Some(h),
        AvatarColorChoice::Preset { color: c, .. } => color == Some(c),
    }
}

pub fn is_avatar_seed(value: &str) -> bool {
    let len = value.len();
    (1..=128).contains(&len)
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b':' || b == b'-')
}

pub fn is_avatar_color(value: &str) -> bool {
    value.len() == 7
        && value.as_bytes()[0] == b'#'
        && value.as_bytes()[1..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

const NEUTRAL_SATURATION: f64 = 0.15;

/// Exact IEEE-754 `f64` HSL saturation & hue distance match for `ipc-agent-identity.ts:nearestAvatarHue`.
pub fn nearest_avatar_hue(color: &str) -> Option<AvatarHue> {
    if !is_avatar_color(color) {
        return None;
    }
    let red = f64::from(u8::from_str_radix(&color[1..3], 16).ok()?) / 255.0;
    let green = f64::from(u8::from_str_radix(&color[3..5], 16).ok()?) / 255.0;
    let blue = f64::from(u8::from_str_radix(&color[5..7], 16).ok()?) / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let lightness = (max + min) / 2.0;
    let saturation = if delta == 0.0 {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };
    if saturation < NEUTRAL_SATURATION {
        return None;
    }
    let sector = if max == red {
        ((green - blue) / delta + 6.0) % 6.0
    } else if max == green {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    };
    let angle = sector * 60.0;
    let mut nearest = AVATAR_HUES[0];
    let mut nearest_distance = f64::INFINITY;
    for hue in AVATAR_HUES {
        let diff = (angle - f64::from(hue.degrees())).abs();
        let distance = diff.min(360.0 - diff);
        if distance < nearest_distance {
            nearest = hue;
            nearest_distance = distance;
        }
    }
    Some(nearest)
}

static NAMED_SILHOUETTE_SEED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":([a-z]+):(?:face|flat):\d+$").expect("valid named silhouette regex"));
static FLAT_SEED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":flat:\d+$").expect("valid flat seed regex"));

pub fn named_silhouette(seed: &str) -> Option<BloubShape> {
    let caps = NAMED_SILHOUETTE_SEED_RE.captures(seed)?;
    match caps.get(1)?.as_str() {
        "fantome" => Some(BloubShape::Fantome),
        "meduse" => Some(BloubShape::Meduse),
        "poulpe" => Some(BloubShape::Poulpe),
        _ => None,
    }
}

/// 2-step bucket remap matching `bloub-avatar.ts:seededSilhouette`:
/// hashes `${seed}:shape` modulo 11; iff result is `"goutte"` (index 7),
/// remaps via `SEEDED_SUPPORTED_SILHOUETTES[stableIndex("${seed}:shape:replacement", 10)]`.
pub fn seeded_silhouette(seed: &str) -> BloubShape {
    let idx = stable_index(&format!("{seed}:shape"), SEEDED_AVATAR_SILHOUETTES.len());
    match SEEDED_AVATAR_SILHOUETTES[idx].to_supported() {
        Some(shape) => shape,
        None => {
            let repl = stable_index(
                &format!("{seed}:shape:replacement"),
                SEEDED_SUPPORTED_SILHOUETTES.len(),
            );
            SEEDED_SUPPORTED_SILHOUETTES[repl]
        }
    }
}

pub fn seeded_expression(seed: &str) -> BloubExpression {
    AVATAR_EXPRESSIONS[stable_index(&format!("{seed}:expression"), AVATAR_EXPRESSIONS.len())]
}

pub fn automatic_color(seed: &str) -> ServerPlushColor {
    AUTOMATIC_COLORS[stable_index(&format!("{seed}:color"), AUTOMATIC_COLORS.len())]
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BloubAvatarProfile {
    pub shape: BloubShape,
    pub color: String,
    pub expression: BloubExpression,
}

impl BloubAvatarProfile {
    pub fn resolve(seed: &str, hue: Option<AvatarHue>, color: Option<&str>) -> Self {
        bloub_avatar_profile(seed, hue, color)
    }
}

pub fn bloub_avatar_profile(
    seed: &str,
    hue: Option<AvatarHue>,
    color: Option<&str>,
) -> BloubAvatarProfile {
    let shape = named_silhouette(seed).unwrap_or_else(|| seeded_silhouette(seed));
    let expression = seeded_expression(seed);
    let resolved_color = match color {
        Some(c) => c.to_string(),
        None => match hue {
            Some(h) => h.swatch().to_string(),
            None => automatic_color(seed).hex().to_string(),
        },
    };
    BloubAvatarProfile {
        shape,
        color: resolved_color,
        expression,
    }
}

pub const fn avatar_hue_swatch(hue: AvatarHue) -> &'static str {
    hue.swatch()
}

pub fn avatar_hue_for_color(color: &str) -> Option<AvatarHue> {
    AVATAR_HUES.iter().copied().find(|hue| hue.swatch() == color)
}

pub fn avatar_head_color(seed: &str, hue: Option<AvatarHue>, color: Option<&str>) -> String {
    bloub_avatar_profile(seed, hue, color).color
}

pub fn server_plush_color(server_id: &str) -> ServerPlushColor {
    automatic_color(server_id)
}

pub fn server_plush_hex(server_id: &str, color: Option<&str>) -> String {
    color
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| automatic_color(server_id).hex().to_string())
}

pub fn avatar_style_for_seed(seed: &str) -> AvatarStyle {
    if FLAT_SEED_RE.is_match(seed) {
        AvatarStyle::Flat
    } else {
        AvatarStyle::Plush
    }
}

pub fn avatar_default_seed(agent_id: &str, style: AvatarStyle) -> Result<String, AvatarContractError> {
    if style == AvatarStyle::Plush {
        return Ok(agent_id.to_string());
    }
    let face = bloub_avatar_profile(agent_id, None, None);
    avatar_seed_for_face(agent_id, face.shape, face.expression, style)
}

pub fn avatar_seed_for_face(
    agent_id: &str,
    shape: BloubShape,
    expression: BloubExpression,
    style: AvatarStyle,
) -> Result<String, AvatarContractError> {
    let named = shape.is_named().then_some(shape);
    let prefix = match named {
        Some(n) => format!("{agent_id}:{}", n.as_str()),
        None => agent_id.to_string(),
    };
    let seeded_shape = shape.seeded_lookalike();
    let segment = style.seed_segment();
    for index in 0..FACE_SEED_TRIES {
        let candidate = format!("{prefix}:{segment}:{index}");
        if named_silhouette(&candidate) == named
            && seeded_silhouette(&candidate) == seeded_shape
            && seeded_expression(&candidate) == expression
        {
            return Ok(candidate);
        }
    }
    Err(AvatarContractError::NoSeedForFace)
}

pub fn avatar_candidate_seeds(agent_id: &str, current_seed: &str, batch: u32) -> Vec<String> {
    let mut candidates = vec![current_seed.to_string()];
    let first_profile = bloub_avatar_profile(current_seed, None, None);
    let mut unseen_silhouettes: HashSet<BloubShape> =
        SEEDED_SUPPORTED_SILHOUETTES.iter().copied().collect();
    let mut used_expressions: HashSet<BloubExpression> = HashSet::from([first_profile.expression]);
    unseen_silhouettes.remove(&first_profile.shape);
    let mut index: u32 = 1;

    while !unseen_silhouettes.is_empty() {
        let candidate = format!("{agent_id}:avatar:{batch}:{index}");
        index += 1;
        if candidates.contains(&candidate) {
            continue;
        }
        let profile = bloub_avatar_profile(&candidate, None, None);
        if !unseen_silhouettes.remove(&profile.shape) {
            continue;
        }
        candidates.push(candidate);
        used_expressions.insert(profile.expression);
    }

    while candidates.len() < 12 {
        let candidate = format!("{agent_id}:avatar:{batch}:{index}");
        index += 1;
        if candidates.contains(&candidate) {
            continue;
        }
        let profile = bloub_avatar_profile(&candidate, None, None);
        if used_expressions.contains(&profile.expression) {
            continue;
        }
        candidates.push(candidate);
        used_expressions.insert(profile.expression);
    }

    candidates
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AvatarMoodPresentation {
    pub state: ShapeSafeStateId,
    pub expression: Option<BloubExpression>,
    pub rings: bool,
    pub breathe: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlushMoodPresentation {
    pub state: PlushState,
    pub expression: Option<BloubExpression>,
    pub busy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoverFace {
    pub state: PlushState,
    pub expression: Option<BloubExpression>,
}

pub const HOVER_FACES: [HoverFace; 5] = [
    HoverFace { state: PlushState::Wink, expression: None },
    HoverFace { state: PlushState::Idle, expression: Some(BloubExpression::Surpris) },
    HoverFace { state: PlushState::Idle, expression: Some(BloubExpression::Excite) },
    HoverFace { state: PlushState::Idle, expression: Some(BloubExpression::Attentif) },
    HoverFace { state: PlushState::Idle, expression: Some(BloubExpression::Curieux) },
];

pub const fn avatar_mood_presentation(mood: AvatarMood) -> AvatarMoodPresentation {
    match mood {
        AvatarMood::Idle => AvatarMoodPresentation {
            state: ShapeSafeStateId::Idle,
            expression: None,
            rings: false,
            breathe: 0.0,
        },
        AvatarMood::Working => AvatarMoodPresentation {
            state: ShapeSafeStateId::Idle,
            expression: Some(BloubExpression::Attentif),
            rings: true,
            breathe: 0.03,
        },
        AvatarMood::Waiting => AvatarMoodPresentation {
            state: ShapeSafeStateId::Idle,
            expression: Some(BloubExpression::Curieux),
            rings: false,
            breathe: 0.0,
        },
        AvatarMood::Failed => AvatarMoodPresentation {
            state: ShapeSafeStateId::Idle,
            expression: Some(BloubExpression::Triste),
            rings: false,
            breathe: 0.0,
        },
        AvatarMood::Responded => AvatarMoodPresentation {
            state: ShapeSafeStateId::Idle,
            expression: Some(BloubExpression::Heureux),
            rings: false,
            breathe: 0.0,
        },
        AvatarMood::Connecting => AvatarMoodPresentation {
            state: ShapeSafeStateId::Swirl,
            expression: Some(BloubExpression::Attentif),
            rings: true,
            breathe: 0.0,
        },
        AvatarMood::Asleep => AvatarMoodPresentation {
            state: ShapeSafeStateId::Idle,
            expression: Some(BloubExpression::Somnolent),
            rings: false,
            breathe: 0.0,
        },
    }
}

pub const fn plush_mood_presentation(mood: AvatarMood) -> PlushMoodPresentation {
    match mood {
        AvatarMood::Idle => PlushMoodPresentation { state: PlushState::Idle, expression: None, busy: false },
        AvatarMood::Working => PlushMoodPresentation { state: PlushState::Thinking, expression: Some(BloubExpression::Attentif), busy: true },
        AvatarMood::Connecting => PlushMoodPresentation { state: PlushState::Thinking, expression: Some(BloubExpression::Attentif), busy: true },
        AvatarMood::Waiting => PlushMoodPresentation { state: PlushState::Notify, expression: Some(BloubExpression::Curieux), busy: false },
        AvatarMood::Failed => PlushMoodPresentation { state: PlushState::Failed, expression: None, busy: false },
        AvatarMood::Responded => PlushMoodPresentation { state: PlushState::Idle, expression: Some(BloubExpression::Heureux), busy: false },
        AvatarMood::Asleep => PlushMoodPresentation { state: PlushState::Asleep, expression: None, busy: false },
    }
}

pub const fn activity_mood_presentation(activity: AgentStepActivity) -> PlushMoodPresentation {
    match activity {
        AgentStepActivity::Searching => PlushMoodPresentation { state: PlushState::Idle, expression: Some(BloubExpression::Curieux), busy: true },
        AgentStepActivity::Browsing => PlushMoodPresentation { state: PlushState::Wide, expression: None, busy: true },
        AgentStepActivity::Reading => PlushMoodPresentation { state: PlushState::Idle, expression: Some(BloubExpression::Attentif), busy: true },
        AgentStepActivity::Running => PlushMoodPresentation { state: PlushState::Idle, expression: Some(BloubExpression::Excite), busy: true },
        AgentStepActivity::Editing => PlushMoodPresentation { state: PlushState::Idle, expression: Some(BloubExpression::Fier), busy: true },
        AgentStepActivity::Delegating => PlushMoodPresentation { state: PlushState::Wink, expression: None, busy: true },
        AgentStepActivity::Working => PlushMoodPresentation { state: PlushState::Idle, expression: Some(BloubExpression::Attentif), busy: true },
        AgentStepActivity::Reviewing => plush_mood_presentation(AvatarMood::Working),
    }
}

pub fn avatar_mood_is_busy(mood: AvatarMood) -> bool {
    let p = avatar_mood_presentation(mood);
    p.rings || p.breathe > 0.0
}

pub fn avatar_breathe_amount(mood: AvatarMood, style: AvatarStyle) -> f64 {
    match style {
        AvatarStyle::Flat => avatar_mood_presentation(mood).breathe,
        AvatarStyle::Plush => {
            if plush_mood_presentation(mood).busy {
                PHOTO_BREATHE
            } else {
                0.0
            }
        }
    }
}

/// Deterministic working expression cycle for `seed` at step `tick`, never repeating adjacent expressions.
pub fn working_cycle_for_seed(seed: &str, tick: usize) -> BloubExpression {
    let mut current = BloubExpression::Attentif;
    for step in 0..tick {
        let idx = stable_index(&format!("{seed}:working:{step}"), AVATAR_EXPRESSIONS.len() - 1);
        let mut chosen = 0usize;
        for candidate in AVATAR_EXPRESSIONS {
            if candidate == current {
                continue;
            }
            if chosen == idx {
                current = candidate;
                break;
            }
            chosen += 1;
        }
    }
    current
}

pub const fn compute_agent_avatar_mood(
    failed_turn: bool,
    waiting_input: bool,
    is_working: bool,
    recent_reply: bool,
    asleep: bool,
    connecting: bool,
) -> AvatarMood {
    if failed_turn {
        AvatarMood::Failed
    } else if waiting_input {
        AvatarMood::Waiting
    } else if is_working {
        AvatarMood::Working
    } else if recent_reply {
        AvatarMood::Responded
    } else if asleep {
        AvatarMood::Asleep
    } else if connecting {
        AvatarMood::Connecting
    } else {
        AvatarMood::Idle
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentAvatarMoodsInput {
    pub agent_ids: Vec<String>,
    pub failed_turns: HashSet<String>,
    pub pending_prompts: HashSet<String>,
    pub pending_approvals: HashSet<String>,
    pub working_agents: HashSet<String>,
    pub recent_replies: HashSet<String>,
}

pub fn compute_agent_avatar_moods(input: &AgentAvatarMoodsInput) -> BTreeMap<String, AvatarMood> {
    let mut moods = BTreeMap::new();
    for id in &input.agent_ids {
        if input.failed_turns.contains(id) {
            moods.insert(id.clone(), AvatarMood::Failed);
        } else if input.pending_approvals.contains(id) || input.pending_prompts.contains(id) {
            moods.insert(id.clone(), AvatarMood::Waiting);
        } else if input.working_agents.contains(id) {
            moods.insert(id.clone(), AvatarMood::Working);
        } else if input.recent_replies.contains(id) {
            moods.insert(id.clone(), AvatarMood::Responded);
        }
    }
    moods
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AvatarMimeType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
}

pub const AVATAR_MIME_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

pub fn is_avatar_mime_type(mime_type: &str) -> bool {
    AVATAR_MIME_TYPES.contains(&mime_type)
}

pub fn is_valid_avatar_image(mime_type: &str, bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > AVATAR_IMAGE_LIMITS.stored_bytes {
        return false;
    }
    match mime_type {
        "image/png" => bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/webp" => {
            bytes.len() >= 12
                && bytes.starts_with(&[0x52, 0x49, 0x46, 0x46])
                && bytes[8..12] == [0x57, 0x45, 0x42, 0x50]
        }
        _ => false,
    }
}

pub const fn avatar_file_extension(mime_type: AvatarMimeType) -> &'static str {
    match mime_type {
        AvatarMimeType::Png => "png",
        AvatarMimeType::Jpeg => "jpg",
        AvatarMimeType::Webp => "webp",
    }
}
