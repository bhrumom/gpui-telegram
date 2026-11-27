//! Bot identity, 22-agent schema, avatar CAS, members, connectors, attribution,
//! and tool-naming domain contracts.

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use uuid::Uuid;

use crate::{
    BOT_AGENTS, BOT_HANDED_PAIRED_WORKSPACES_MAX, BOT_MAX_CONNECTOR_WORKSPACES, BOT_MAX_MEMBERS,
    BOT_MAX_SKILL_PROJECTS, BOT_NAME_MAX_LENGTH, BotErrorCode, LOCAL_EXECUTION_HOST_ID,
    UNNAMED_BOT_NAME,
};

// ---------------------------------------------------------------------------
// 1. BotAgentId, Retired Agent Migration & Older-Build Support Split
// ---------------------------------------------------------------------------

/// All 22 live Orca Bot agent engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BotAgentId {
    #[default]
    Claude,
    Codex,
    Grok,
    Opencode,
    Omp,
    Pi,
    Antigravity,
    Cursor,
    Hermes,
    Copilot,
    Opencode2,
    Kimi,
    Kiro,
    Aug,
    Goose,
    Kilo,
    #[serde(rename = "mimo-code")]
    MimoCode,
    Droid,
    #[serde(rename = "qwen-code")]
    QwenCode,
    Cline,
    Devin,
    Codebuddy,
}

/// The 7 original agents supported by older Orca builds.
pub const BOT_AGENTS_OLDER_BUILDS_READ: [BotAgentId; 7] = [
    BotAgentId::Claude,
    BotAgentId::Codex,
    BotAgentId::Grok,
    BotAgentId::Opencode,
    BotAgentId::Omp,
    BotAgentId::Pi,
    BotAgentId::Antigravity,
];

/// Retired agent wire identifiers kept for automatic migration.
pub const BOT_RETIRED_AGENTS: [&str; 2] = ["gemini", "claude-code"];

/// Retired agent enum for explicit migration mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotRetiredAgent {
    Gemini,
    ClaudeCode,
}

impl BotRetiredAgent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::ClaudeCode => "claude-code",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "gemini" => Some(Self::Gemini),
            "claude-code" => Some(Self::ClaudeCode),
            _ => None,
        }
    }

    pub const fn successor(self) -> BotAgentId {
        match self {
            Self::Gemini => BotAgentId::Antigravity,
            Self::ClaudeCode => BotAgentId::Claude,
        }
    }
}

impl BotAgentId {
    pub const ALL: [Self; 22] = [
        Self::Claude,
        Self::Codex,
        Self::Grok,
        Self::Opencode,
        Self::Omp,
        Self::Pi,
        Self::Antigravity,
        Self::Cursor,
        Self::Hermes,
        Self::Copilot,
        Self::Opencode2,
        Self::Kimi,
        Self::Kiro,
        Self::Aug,
        Self::Goose,
        Self::Kilo,
        Self::MimoCode,
        Self::Droid,
        Self::QwenCode,
        Self::Cline,
        Self::Devin,
        Self::Codebuddy,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => BOT_AGENTS[0],
            Self::Codex => BOT_AGENTS[1],
            Self::Grok => BOT_AGENTS[2],
            Self::Opencode => BOT_AGENTS[3],
            Self::Omp => BOT_AGENTS[4],
            Self::Pi => BOT_AGENTS[5],
            Self::Antigravity => BOT_AGENTS[6],
            Self::Cursor => BOT_AGENTS[7],
            Self::Hermes => BOT_AGENTS[8],
            Self::Copilot => BOT_AGENTS[9],
            Self::Opencode2 => BOT_AGENTS[10],
            Self::Kimi => BOT_AGENTS[11],
            Self::Kiro => BOT_AGENTS[12],
            Self::Aug => BOT_AGENTS[13],
            Self::Goose => BOT_AGENTS[14],
            Self::Kilo => BOT_AGENTS[15],
            Self::MimoCode => BOT_AGENTS[16],
            Self::Droid => BOT_AGENTS[17],
            Self::QwenCode => BOT_AGENTS[18],
            Self::Cline => BOT_AGENTS[19],
            Self::Devin => BOT_AGENTS[20],
            Self::Codebuddy => BOT_AGENTS[21],
        }
    }

    /// Parses strictly against the 22 live agent wire strings (case-sensitive).
    pub fn parse_strict(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.as_str() == raw)
    }

    pub fn is_older_build_readable(self) -> bool {
        BOT_AGENTS_OLDER_BUILDS_READ.contains(&self)
    }

    pub const fn supports_look_around(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }

    pub const fn supports_tidy_up(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }
}

impl fmt::Display for BotAgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolves a live or retired agent string into `(BotAgentId, was_retired)`.
pub fn migrate_retired_agent_id(raw: &str) -> Option<(BotAgentId, bool)> {
    if let Some(id) = BotAgentId::parse_strict(raw) {
        return Some((id, false));
    }
    BotRetiredAgent::parse(raw).map(|retired| (retired.successor(), true))
}

/// Migrates a persisted JSON bot record if `"agent"` names a retired engine.
///
/// Updates `"agent"` to its successor and deletes `"launch"` (launch options belonged
/// to the retired engine). Returns `true` if the object was migrated.
pub fn migrate_retired_bot_record(value: &mut serde_json::Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let Some(agent_raw) = obj.get("agent").and_then(|v| v.as_str()) else {
        return false;
    };
    let Some(retired) = BotRetiredAgent::parse(agent_raw) else {
        return false;
    };
    obj.insert(
        "agent".to_string(),
        serde_json::Value::String(retired.successor().as_str().to_string()),
    );
    obj.remove("launch");
    true
}

/// Partition of bots readable by older builds vs bots requiring newer builds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitBots {
    pub readable: Vec<OrcaBotRecord>,
    pub newer: Vec<OrcaBotRecord>,
}

/// Partitions `bots` preserving input order into older-build-readable (7 agents) vs newer.
pub fn split_bots_by_older_build_support(bots: &[OrcaBotRecord]) -> SplitBots {
    let mut readable = Vec::new();
    let mut newer = Vec::new();
    for bot in bots {
        if bot.agent.is_older_build_readable() {
            readable.push(bot.clone());
        } else {
            newer.push(bot.clone());
        }
    }
    SplitBots { readable, newer }
}

// ---------------------------------------------------------------------------
// 2. BotName & BotFolderSlug
// ---------------------------------------------------------------------------

/// Validated Bot display name: no Unicode `\p{Cc}` control chars (checked BEFORE trim),
/// trimmed non-empty, `<= 64` Unicode scalar values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BotName(String);

impl BotName {
    pub fn new(raw: &str) -> Result<Self, BotErrorCode> {
        if raw.chars().any(char::is_control) {
            return Err(BotErrorCode::InvalidArgument);
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.chars().count() > BOT_NAME_MAX_LENGTH {
            return Err(BotErrorCode::InvalidArgument);
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for BotName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BotName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BotName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

/// Validates `requested` as a `BotName`, or auto-generates `"New bot"`, `"New bot 2"`, ...
/// avoiding case-insensitive collisions with `existing_names`.
pub fn new_bot_name<S: AsRef<str>>(
    requested: &str,
    existing_names: &[S],
) -> Result<BotName, BotErrorCode> {
    if requested.chars().any(char::is_control) {
        return Err(BotErrorCode::InvalidArgument);
    }
    if !requested.trim().is_empty() {
        return BotName::new(requested);
    }
    let used: HashSet<String> = existing_names
        .iter()
        .map(|s| s.as_ref().trim().to_lowercase())
        .collect();
    if !used.contains(&UNNAMED_BOT_NAME.to_lowercase()) {
        return BotName::new(UNNAMED_BOT_NAME);
    }
    let mut n: u64 = 2;
    loop {
        let candidate = format!("{UNNAMED_BOT_NAME} {n}");
        if !used.contains(&candidate.to_lowercase()) {
            return BotName::new(&candidate);
        }
        n = n.saturating_add(1);
    }
}

/// Validated Bot home folder slug matching `^[a-z0-9]+(?:-[a-z0-9]+)*$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BotFolderSlug(String);

impl BotFolderSlug {
    pub fn new(raw: &str) -> Result<Self, BotErrorCode> {
        if raw.is_empty()
            || !raw
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || raw.starts_with('-')
            || raw.ends_with('-')
            || raw.contains("--")
        {
            return Err(BotErrorCode::InvalidArgument);
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for BotFolderSlug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BotFolderSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BotFolderSlug {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

fn is_windows_reserved_folder_name(slug: &str) -> bool {
    if matches!(slug, "con" | "prn" | "aux" | "nul") {
        return true;
    }
    if let Some(rest) = slug.strip_prefix("com") {
        return rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit();
    }
    if let Some(rest) = slug.strip_prefix("lpt") {
        return rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit();
    }
    false
}

/// Derives a safe `BotFolderSlug` from an arbitrary bot display name,
/// truncating to 40 ASCII chars and suffixing Windows reserved names with `"-bot"`.
pub fn bot_folder_name(bot_name: &str) -> BotFolderSlug {
    let mut slug = String::new();
    let mut prev_dash = true;
    for ch in bot_name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            slug.push(lower);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    let truncated = if trimmed.len() > 40 {
        trimmed[..40].trim_end_matches('-')
    } else {
        trimmed
    };
    let candidate = if truncated.is_empty() {
        "bot".to_string()
    } else if is_windows_reserved_folder_name(truncated) {
        format!("{truncated}-bot")
    } else {
        truncated.to_string()
    };
    BotFolderSlug(candidate)
}

// ---------------------------------------------------------------------------
// 3. BotAvatarSpec (20 Mascots, 9 Outfits, Swatches, PNG Validator, CAS)
// ---------------------------------------------------------------------------

/// All 20 Orca bot mascots (`A1` through `T1`; `S2` retired from picker but valid on disk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BotMascotId {
    A1,
    B1,
    D1,
    E1,
    F1,
    G1,
    H1,
    H2,
    I1,
    J1,
    K2,
    L2,
    M1,
    N1,
    O1,
    P3,
    Q1,
    R3,
    S2,
    T1,
}

impl BotMascotId {
    pub const ALL: [Self; 20] = [
        Self::A1,
        Self::B1,
        Self::D1,
        Self::E1,
        Self::F1,
        Self::G1,
        Self::H1,
        Self::H2,
        Self::I1,
        Self::J1,
        Self::K2,
        Self::L2,
        Self::M1,
        Self::N1,
        Self::O1,
        Self::P3,
        Self::Q1,
        Self::R3,
        Self::S2,
        Self::T1,
    ];

    pub const PICKABLE: [Self; 19] = [
        Self::A1,
        Self::B1,
        Self::D1,
        Self::E1,
        Self::F1,
        Self::G1,
        Self::H1,
        Self::H2,
        Self::I1,
        Self::J1,
        Self::K2,
        Self::L2,
        Self::M1,
        Self::N1,
        Self::O1,
        Self::P3,
        Self::Q1,
        Self::R3,
        Self::T1,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A1 => "A1",
            Self::B1 => "B1",
            Self::D1 => "D1",
            Self::E1 => "E1",
            Self::F1 => "F1",
            Self::G1 => "G1",
            Self::H1 => "H1",
            Self::H2 => "H2",
            Self::I1 => "I1",
            Self::J1 => "J1",
            Self::K2 => "K2",
            Self::L2 => "L2",
            Self::M1 => "M1",
            Self::N1 => "N1",
            Self::O1 => "O1",
            Self::P3 => "P3",
            Self::Q1 => "Q1",
            Self::R3 => "R3",
            Self::S2 => "S2",
            Self::T1 => "T1",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.as_str() == raw)
    }

    pub const fn is_retired(self) -> bool {
        matches!(self, Self::S2)
    }
}

/// All 9 known Orca bot avatar outfits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotAvatarOutfit {
    Tie,
    Detective,
    HardHat,
    Reviewer,
    NightShift,
    Artist,
    Scientist,
    Party,
    Security,
}

impl BotAvatarOutfit {
    pub const ALL: [Self; 9] = [
        Self::Tie,
        Self::Detective,
        Self::HardHat,
        Self::Reviewer,
        Self::NightShift,
        Self::Artist,
        Self::Scientist,
        Self::Party,
        Self::Security,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tie => "tie",
            Self::Detective => "detective",
            Self::HardHat => "hard-hat",
            Self::Reviewer => "reviewer",
            Self::NightShift => "night-shift",
            Self::Artist => "artist",
            Self::Scientist => "scientist",
            Self::Party => "party",
            Self::Security => "security",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == raw)
    }
}

/// Forward-compatible stored outfit slug check (`1..=40` chars, `[a-z0-9-]`).
pub fn is_valid_stored_outfit(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Resolves a stored outfit string to a known `BotAvatarOutfit`, degrading unknown
/// future outfits to `None` while allowing callers to preserve the raw stored value.
pub fn worn_bot_outfit(stored: Option<&str>) -> Option<BotAvatarOutfit> {
    stored.and_then(BotAvatarOutfit::parse)
}

pub const DEFAULT_BOT_AVATAR_COLOR: &str = "#b3dbf4";

pub const BOT_AVATAR_COLORS: [&str; 11] = [
    "#b3dbf4", "#ffffff", "#c9b8e8", "#f4c4cf", "#f8cfa8", "#f6e3a1", "#c5dfb5", "#b5e3d8",
    "#8fa3b8", "#3a3f4b", "#1f2126",
];

pub const BOT_AVATAR_MIN_SIZE: u32 = 512;
pub const BOT_AVATAR_MAX_SIZE: u32 = 2000;
pub const BOT_AVATAR_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Validates a lowercase 6-digit hex color swatch (`^#[0-9a-f]{6}$`).
pub fn is_valid_avatar_color(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 7
        && bytes[0] == b'#'
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

fn srgb_channel_to_linear(byte: u8) -> f64 {
    let c = (byte as f64) / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Returns `true` if `hex` is a valid `#rrggbb` color with WCAG relative luminance `< 0.18`.
pub fn is_dark_bot_avatar_color(hex_color: &str) -> bool {
    if !is_valid_avatar_color(hex_color) {
        return false;
    }
    let Ok(r) = u8::from_str_radix(&hex_color[1..3], 16) else {
        return false;
    };
    let Ok(g) = u8::from_str_radix(&hex_color[3..5], 16) else {
        return false;
    };
    let Ok(b) = u8::from_str_radix(&hex_color[5..7], 16) else {
        return false;
    };
    let lum = 0.2126 * srgb_channel_to_linear(r)
        + 0.7152 * srgb_channel_to_linear(g)
        + 0.0722 * srgb_channel_to_linear(b);
    lum < 0.18
}

/// Parsed representation of an `OrcaBotRecord`'s `avatar` string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParsedBotAvatar {
    Mascot(BotMascotId),
    Upload { version: String },
    Emoji(String),
}

pub fn parse_bot_avatar(raw: Option<&str>) -> Option<ParsedBotAvatar> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(id) = s.strip_prefix("mascot:") {
        return BotMascotId::parse(id).map(ParsedBotAvatar::Mascot);
    }
    if let Some(version) = s.strip_prefix("upload:") {
        if !version.is_empty()
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Some(ParsedBotAvatar::Upload {
                version: version.to_string(),
            });
        }
        return None;
    }
    Some(ParsedBotAvatar::Emoji(s.to_string()))
}

/// Pure-Rust PNG signature + IHDR square/dimension validator (`512..=2000`, `<= 2 MiB`).
pub fn validate_bot_avatar_png(png: &[u8]) -> Result<(u32, u32), BotErrorCode> {
    const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() < 24 || png.len() > BOT_AVATAR_MAX_BYTES {
        return Err(BotErrorCode::BotAvatarInvalid);
    }
    let ihdr_len = u32::from_be_bytes([png[8], png[9], png[10], png[11]]);
    if png[..8] != PNG_SIG || ihdr_len != 13 || &png[12..16] != b"IHDR" {
        return Err(BotErrorCode::BotAvatarInvalid);
    }
    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    if width != height || !(BOT_AVATAR_MIN_SIZE..=BOT_AVATAR_MAX_SIZE).contains(&width) {
        return Err(BotErrorCode::BotAvatarInvalid);
    }
    Ok((width, height))
}

fn normalize_manual_avatar(raw: &str) -> Result<String, BotErrorCode> {
    if raw.chars().any(char::is_control) {
        return Err(BotErrorCode::BotAvatarInvalid);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with("upload:") || trimmed.chars().count() > 32 {
        return Err(BotErrorCode::BotAvatarInvalid);
    }
    if let Some(id) = trimmed.strip_prefix("mascot:")
        && BotMascotId::parse(id).is_none()
    {
        return Err(BotErrorCode::BotAvatarInvalid);
    }
    Ok(trimmed.to_string())
}

/// Persisted avatar specification with CAS `avatar_revision` + `avatar_revision_epoch`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotAvatarSpec {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub avatar: Option<String>,
    #[serde(
        default,
        alias = "avatarColor",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_avatar_color"
    )]
    pub avatar_color: Option<String>,
    #[serde(
        default,
        alias = "avatarOutfit",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_avatar_outfit"
    )]
    pub avatar_outfit: Option<String>,
    #[serde(
        default,
        alias = "customPng",
        deserialize_with = "deserialize_salvage_default"
    )]
    pub custom_png: bool,
    #[serde(
        default,
        alias = "generatedOverMascot",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub generated_over_mascot: Option<String>,
    #[serde(
        default,
        alias = "avatarRevision",
        deserialize_with = "deserialize_salvage_default"
    )]
    pub avatar_revision: u64,
    #[serde(
        default,
        alias = "avatarRevisionEpoch",
        deserialize_with = "deserialize_salvage_default"
    )]
    pub avatar_revision_epoch: String,
}

fn deserialize_salvage_avatar_color<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let val = serde_json::Value::deserialize(deserializer)?;
    Ok(val
        .as_str()
        .filter(|s| is_valid_avatar_color(s))
        .map(str::to_string))
}

fn deserialize_salvage_avatar_outfit<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let val = serde_json::Value::deserialize(deserializer)?;
    Ok(val
        .as_str()
        .filter(|s| is_valid_stored_outfit(s))
        .map(str::to_string))
}

impl BotAvatarSpec {
    pub fn new_with_mascot(mascot: BotMascotId) -> Self {
        Self {
            avatar: Some(format!("mascot:{}", mascot.as_str())),
            avatar_color: Some(DEFAULT_BOT_AVATAR_COLOR.to_string()),
            avatar_outfit: None,
            custom_png: false,
            generated_over_mascot: None,
            avatar_revision: 0,
            avatar_revision_epoch: Uuid::new_v4().to_string(),
        }
    }

    pub fn parsed(&self) -> Option<ParsedBotAvatar> {
        parse_bot_avatar(self.avatar.as_deref())
    }

    pub fn worn_outfit(&self) -> Option<BotAvatarOutfit> {
        worn_bot_outfit(self.avatar_outfit.as_deref())
    }

    /// Compare-and-swap check for avatar mutations.
    pub fn check_cas(
        &self,
        expected_revision: u64,
        expected_epoch: Option<&str>,
    ) -> Result<(), BotErrorCode> {
        if expected_revision != self.avatar_revision {
            return Err(BotErrorCode::BotAvatarChanged);
        }
        if let Some(epoch) = expected_epoch
            && epoch != self.avatar_revision_epoch
        {
            return Err(BotErrorCode::BotAvatarChanged);
        }
        Ok(())
    }

    /// Validates and applies a manual look change (mascot/emoji/color/outfit).
    /// Direct `"upload:*"` assignment is rejected with `BotAvatarInvalid`, and valid
    /// avatars are stored normalized (trimmed).
    pub fn apply_manual_look_update(
        &mut self,
        avatar: Option<String>,
        color: Option<String>,
        outfit: Option<String>,
    ) -> Result<(), BotErrorCode> {
        let normalized_avatar = match avatar {
            Some(ref a) => Some(normalize_manual_avatar(a)?),
            None => None,
        };
        if let Some(ref c) = color
            && !is_valid_avatar_color(c)
        {
            return Err(BotErrorCode::BotAvatarInvalid);
        }
        if let Some(ref o) = outfit
            && !is_valid_stored_outfit(o)
        {
            return Err(BotErrorCode::BotAvatarInvalid);
        }
        self.avatar = normalized_avatar;
        self.avatar_color = color;
        self.avatar_outfit = outfit;
        self.custom_png = false;
        self.generated_over_mascot = None;
        self.avatar_revision = self.avatar_revision.saturating_add(1);
        Ok(())
    }

    /// Validates PNG bytes, enforces optional CAS `(revision, epoch)`, computes 16-hex
    /// SHA-256 version, tracks `generated_over_mascot`, and bumps `avatar_revision`.
    pub fn apply_custom_png_upload(
        &mut self,
        png: &[u8],
        expected_revision: Option<u64>,
        expected_epoch: Option<&str>,
    ) -> Result<String, BotErrorCode> {
        validate_bot_avatar_png(png)?;
        if let Some(rev) = expected_revision {
            self.check_cas(rev, expected_epoch)?;
        }
        let digest = hex::encode(Sha256::digest(png));
        let version = digest[..16].to_string();
        let prev_mascot = if expected_revision.is_some() {
            self.avatar
                .as_deref()
                .filter(|a| {
                    a.strip_prefix("mascot:")
                        .and_then(BotMascotId::parse)
                        .is_some()
                })
                .map(str::to_string)
        } else {
            None
        };
        self.generated_over_mascot = prev_mascot;
        self.avatar = Some(format!("upload:{version}"));
        self.custom_png = true;
        self.avatar_revision = self.avatar_revision.saturating_add(1);
        Ok(version)
    }
}

// ---------------------------------------------------------------------------
// 4. BotMember & BotConnectorLinks (with Fail-Closed google_accounts)
// ---------------------------------------------------------------------------

/// Generic forward-compatible salvage deserializer for optional fields:
/// invalid or newer-version shapes deserialize as `None` rather than dropping the bot.
pub(crate) fn deserialize_salvage_opt<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::de::DeserializeOwned,
    D: Deserializer<'de>,
{
    let val = serde_json::Value::deserialize(deserializer)?;
    if val.is_null() {
        return Ok(None);
    }
    Ok(serde_json::from_value(val).ok())
}

/// Forward-compatible salvage deserializer for defaulted non-Option fields:
/// `null` or invalid shapes fall back to `T::default()` rather than dropping the bot.
pub(crate) fn deserialize_salvage_default<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + serde::de::DeserializeOwned,
    D: Deserializer<'de>,
{
    let val = serde_json::Value::deserialize(deserializer)?;
    if val.is_null() {
        return Ok(T::default());
    }
    Ok(serde_json::from_value(val).unwrap_or_default())
}

fn deserialize_salvage_bool_true<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let val = serde_json::Value::deserialize(deserializer)?;
    Ok(val.as_bool().unwrap_or(true))
}

fn deserialize_salvage_vec<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    T: serde::de::DeserializeOwned,
    D: Deserializer<'de>,
{
    let val = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Array(items) = val else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
}

fn deserialize_salvage_host_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let val = serde_json::Value::deserialize(deserializer)?;
    Ok(val
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(default_host_id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotMemberKind {
    App,
}

/// Validates connector avatar URLs: HTTPS only, no userinfo/port, `<= 2048` bytes,
/// allowlisted hosts (`*.slack-edge.com`, `avatars.githubusercontent.com`, `public.linear.app`).
pub fn is_allowed_connector_avatar_url(url: &str) -> bool {
    if url.len() > 2048 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains(':')
        || authority.contains('[')
        || authority.contains(']')
        || authority.contains("..")
        || authority.starts_with('.')
        || authority.ends_with('.')
        || authority.starts_with('-')
        || authority.ends_with('-')
    {
        return false;
    }
    if !authority
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return false;
    }
    let host = authority.to_ascii_lowercase();
    host == "avatars.githubusercontent.com"
        || host == "public.linear.app"
        || (host.ends_with(".slack-edge.com") && host.len() > ".slack-edge.com".len())
}

fn deserialize_salvage_avatar_url<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let val = serde_json::Value::deserialize(deserializer)?;
    Ok(val
        .as_str()
        .filter(|s| is_allowed_connector_avatar_url(s))
        .map(str::to_string))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotMember {
    pub id: String,
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub source: Option<String>,
    #[serde(
        default,
        alias = "workspaceId",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub workspace_id: Option<String>,
    #[serde(
        default,
        alias = "avatarUrl",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_avatar_url"
    )]
    pub avatar_url: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub kind: Option<BotMemberKind>,
}

pub fn bot_member_source(member: &BotMember) -> &str {
    member
        .source
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("slack")
}

pub fn bot_members_from<'a>(members: &'a [BotMember], source: &str) -> Vec<&'a BotMember> {
    members
        .iter()
        .filter(|m| bot_member_source(m) == source)
        .collect()
}

pub fn without_bot_member(
    members: &[BotMember],
    source: &str,
    id: &str,
    workspace_id: Option<&str>,
) -> Vec<BotMember> {
    members
        .iter()
        .filter(|m| {
            !(bot_member_source(m) == source
                && m.id == id
                && (workspace_id.is_none() || m.workspace_id.as_deref() == workspace_id))
        })
        .cloned()
        .collect()
}

pub fn with_slack_members_tagged(members: &[BotMember]) -> Vec<BotMember> {
    members
        .iter()
        .map(|m| {
            let mut next = m.clone();
            if next.source.as_deref().is_none_or(str::is_empty) {
                next.source = Some("slack".to_string());
            }
            next
        })
        .collect()
}

pub fn slack_members_of(members: &[BotMember]) -> Vec<&BotMember> {
    bot_members_from(members, "slack")
}

pub fn linear_members_of(members: &[BotMember]) -> Vec<&BotMember> {
    bot_members_from(members, "linear")
}

pub fn with_github_members_tagged(members: &[BotMember]) -> Vec<BotMember> {
    members
        .iter()
        .map(|m| {
            let mut next = m.clone();
            next.source = Some("github".to_string());
            next
        })
        .collect()
}

pub fn github_members_of(members: &[BotMember]) -> Vec<&BotMember> {
    bot_members_from(members, "github")
}

pub fn validate_members_cap(members: &[BotMember]) -> Result<(), BotErrorCode> {
    if members.len() > BOT_MAX_MEMBERS {
        return Err(BotErrorCode::Full);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotSlackLink {
    #[serde(alias = "appId")]
    pub app_id: String,
    #[serde(default, alias = "teamId", skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(
        default,
        alias = "ownerUserId",
        skip_serializing_if = "Option::is_none"
    )]
    pub owner_user_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotLinearLink {
    #[serde(alias = "workspaceId")]
    pub workspace_id: String,
    #[serde(
        default,
        alias = "workspaceName",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_name: Option<String>,
    #[serde(default, alias = "urlKey", skip_serializing_if = "Option::is_none")]
    pub url_key: Option<String>,
    #[serde(
        default,
        alias = "ownerUserId",
        skip_serializing_if = "Option::is_none"
    )]
    pub owner_user_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotGitHubLink {
    #[serde(default, alias = "removedInstallationIds")]
    pub removed_installation_ids: Vec<String>,
    #[serde(
        default,
        alias = "ownerUserId",
        skip_serializing_if = "Option::is_none"
    )]
    pub owner_user_id: Option<String>,
}


/// Fail-closed deserializer for `google_accounts`.
///
/// - Key ABSENT in JSON -> serde `#[serde(default)]` yields `None` ("all signed-in Google accounts").
/// - Key PRESENT with `null`, non-array, oversize (`> 100`), or any non-string / empty / `> 320`-char
///   element -> ALWAYS yields `Some(Vec::new())` ("NO Google accounts", NEVER `None`).
pub fn deserialize_safe_google_accounts<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    let val = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Array(items) = val else {
        return Ok(Some(Vec::new()));
    };
    if items.len() > 100 {
        return Ok(Some(Vec::new()));
    }
    let mut accounts = Vec::with_capacity(items.len());
    for item in items {
        let Some(s) = item.as_str() else {
            return Ok(Some(Vec::new()));
        };
        if s.is_empty() || s.chars().count() > 320 {
            return Ok(Some(Vec::new()));
        }
        accounts.push(s.to_string());
    }
    Ok(Some(accounts))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotConnectorLinks {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub slack: Option<BotSlackLink>,
    #[serde(
        default,
        alias = "slackApps",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub slack_apps: Option<Vec<BotSlackLink>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub linear: Option<BotLinearLink>,
    #[serde(
        default,
        alias = "linearWorkspaces",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub linear_workspaces: Option<Vec<BotLinearLink>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub github: Option<BotGitHubLink>,
    /// Whether to mirror owner-started Slack threads (default semantic: on when `None`).
    #[serde(
        default,
        alias = "slackMirror",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub slack_mirror: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub google: Option<bool>,
    #[serde(
        default,
        alias = "googleAccounts",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_safe_google_accounts"
    )]
    pub google_accounts: Option<Vec<String>>,
    /// Host-only toggle allowing host MCP servers (default semantic: off when `None`).
    #[serde(
        default,
        alias = "hostMcpServers",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub host_mcp_servers: Option<bool>,
}

impl BotConnectorLinks {
    pub fn validate(&self) -> Result<(), BotErrorCode> {
        if let Some(ref apps) = self.slack_apps
            && apps.len() > BOT_MAX_CONNECTOR_WORKSPACES
        {
            return Err(BotErrorCode::Full);
        }
        if let Some(ref workspaces) = self.linear_workspaces
            && workspaces.len() > BOT_MAX_CONNECTOR_WORKSPACES
        {
            return Err(BotErrorCode::Full);
        }
        if let Some(ref gh) = self.github
            && gh.removed_installation_ids.len() > 100
        {
            return Err(BotErrorCode::Full);
        }
        Ok(())
    }

    pub fn effective_slack_apps(&self) -> Vec<&BotSlackLink> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if let Some(ref apps) = self.slack_apps {
            for app in apps {
                if seen.insert(app.app_id.as_str()) {
                    out.push(app);
                }
            }
        }
        if let Some(ref app) = self.slack
            && seen.insert(app.app_id.as_str())
        {
            out.push(app);
        }
        out
    }

    pub fn effective_linear_workspaces(&self) -> Vec<&BotLinearLink> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        if let Some(ref wss) = self.linear_workspaces {
            for ws in wss {
                if seen.insert(ws.workspace_id.as_str()) {
                    out.push(ws);
                }
            }
        }
        if let Some(ref ws) = self.linear
            && seen.insert(ws.workspace_id.as_str())
        {
            out.push(ws);
        }
        out
    }

    /// Returns `true` if any external connector (`slack`, `linear`, `github`, `google`) is linked.
    /// Note: `host_mcp_servers` is a host-only MCP server toggle rather than an external connector;
    /// inspect it directly or via [`Self::uses_host_mcp_servers`].
    pub fn has_connector(&self) -> bool {
        !self.effective_slack_apps().is_empty()
            || !self.effective_linear_workspaces().is_empty()
            || self.github.is_some()
            || self.google == Some(true)
    }

    /// Returns `true` iff the host-only `host_mcp_servers` toggle is explicitly enabled.
    pub fn uses_host_mcp_servers(&self) -> bool {
        self.host_mcp_servers == Some(true)
    }
}

// ---------------------------------------------------------------------------
// 5. OrcaBotRecord & Forward-Compatible Persisted Normalization
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotPermissionMode {
    #[serde(alias = "default")]
    Ask,
    AcceptEdits,
    Auto,
    FullAccess,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotLaunchConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, alias = "extraArgs", skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotNotifications {
    #[serde(
        default = "default_true",
        alias = "needs_permission",
        deserialize_with = "deserialize_salvage_bool_true"
    )]
    pub needs_permission: bool,
    #[serde(
        default = "default_true",
        alias = "task_complete",
        deserialize_with = "deserialize_salvage_bool_true"
    )]
    pub task_complete: bool,
    #[serde(
        default = "default_true",
        alias = "could_not_finish",
        deserialize_with = "deserialize_salvage_bool_true"
    )]
    pub could_not_finish: bool,
}

fn default_true() -> bool {
    true
}

impl Default for BotNotifications {
    fn default() -> Self {
        Self {
            needs_permission: true,
            task_complete: true,
            could_not_finish: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotLookAroundConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(default, alias = "weekdaysOnly")]
    pub weekdays_only: bool,
    #[serde(default, alias = "lastRunAt", skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotTidyUpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, alias = "lastRunAt", skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
}

/// Stale threshold for Bot Items: `3 | 5 | 7 | 14 | "never"` (semantic default: `5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BotItemsStale {
    Days3,
    #[default]
    Days5,
    Days7,
    Days14,
    Never,
}

impl BotItemsStale {
    pub const fn days(self) -> Option<u32> {
        match self {
            Self::Days3 => Some(3),
            Self::Days5 => Some(5),
            Self::Days7 => Some(7),
            Self::Days14 => Some(14),
            Self::Never => None,
        }
    }
}

impl Serialize for BotItemsStale {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Days3 => serializer.serialize_u32(3),
            Self::Days5 => serializer.serialize_u32(5),
            Self::Days7 => serializer.serialize_u32(7),
            Self::Days14 => serializer.serialize_u32(14),
            Self::Never => serializer.serialize_str("never"),
        }
    }
}

impl<'de> Deserialize<'de> for BotItemsStale {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        match val {
            serde_json::Value::Number(n) => match n.as_u64() {
                Some(3) => Ok(Self::Days3),
                Some(5) => Ok(Self::Days5),
                Some(7) => Ok(Self::Days7),
                Some(14) => Ok(Self::Days14),
                _ => Err(serde::de::Error::custom("invalid items_stale number")),
            },
            serde_json::Value::String(s) if s == "never" => Ok(Self::Never),
            _ => Err(serde::de::Error::custom("invalid items_stale value")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedBotOffer {
    #[serde(alias = "offerId")]
    pub offer_id: String,
    #[serde(alias = "acceptedAt")]
    pub accepted_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotOfferAnswerState {
    Sent,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotOfferAnswer {
    #[serde(alias = "offerId")]
    pub offer_id: String,
    pub state: BotOfferAnswerState,
    #[serde(alias = "answeredAt")]
    pub answered_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotOfferAnswerUpdate {
    Claim {
        offer_id: String,
        state: BotOfferAnswerState,
        answered_at: i64,
    },
    Release {
        offer_id: String,
    },
}

/// CAS state transition for offer card answers.
///
/// Rejects claiming an offer that was already `Sent`, or re-dismissing an offer that
/// was already `Dismissed`, with `Err(BotErrorCode::BotOfferAlreadyAnswered)`.
pub fn apply_bot_offer_answer(
    answers: &[BotOfferAnswer],
    update: BotOfferAnswerUpdate,
) -> Result<Vec<BotOfferAnswer>, BotErrorCode> {
    let mut next = answers.to_vec();
    match update {
        BotOfferAnswerUpdate::Claim {
            offer_id,
            state,
            answered_at,
        } => {
            if let Some(existing) = next.iter_mut().find(|a| a.offer_id == offer_id) {
                if existing.state == BotOfferAnswerState::Sent
                    || (existing.state == BotOfferAnswerState::Dismissed
                        && state == BotOfferAnswerState::Dismissed)
                {
                    return Err(BotErrorCode::BotOfferAlreadyAnswered);
                }
                existing.state = state;
                existing.answered_at = answered_at;
            } else {
                next.push(BotOfferAnswer {
                    offer_id,
                    state,
                    answered_at,
                });
            }
            Ok(next)
        }
        BotOfferAnswerUpdate::Release { offer_id } => {
            next.retain(|a| a.offer_id != offer_id);
            Ok(next)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedPairedWorkspace {
    #[serde(alias = "workspaceId")]
    pub workspace_id: String,
    #[serde(alias = "handedAt")]
    pub handed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCancelledWait {
    #[serde(alias = "waitId")]
    pub wait_id: String,
    #[serde(alias = "cancelledAt")]
    pub cancelled_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotImportedFrom {
    pub source: String,
    #[serde(default, alias = "originalName", skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    #[serde(alias = "importedAt")]
    pub imported_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotSharedFrom {
    #[serde(alias = "senderId")]
    pub sender_id: String,
    #[serde(default, alias = "senderName", skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    #[serde(alias = "sharedAt")]
    pub shared_at: i64,
}

fn default_host_id() -> String {
    LOCAL_EXECUTION_HOST_ID.to_string()
}

/// Complete persisted record for an Orca Bot with forward-compatible salvage fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaBotRecord {
    pub id: String,
    pub name: BotName,
    #[serde(
        default = "default_host_id",
        alias = "hostId",
        deserialize_with = "deserialize_salvage_host_id"
    )]
    pub host_id: String,
    pub folder: BotFolderSlug,
    #[serde(
        default,
        alias = "folderWorkspaceId",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub folder_workspace_id: Option<String>,
    #[serde(default)]
    pub agent: BotAgentId,
    #[serde(flatten)]
    pub avatar_spec: BotAvatarSpec,
    #[serde(default, deserialize_with = "deserialize_salvage_default")]
    pub paused: bool,
    #[serde(
        default,
        alias = "permissionMode",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub permission_mode: Option<BotPermissionMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub launch: Option<BotLaunchConfig>,
    #[serde(default, deserialize_with = "deserialize_salvage_default")]
    pub notifications: BotNotifications,
    #[serde(default, deserialize_with = "deserialize_salvage_vec")]
    pub members: Vec<BotMember>,
    #[serde(flatten)]
    pub connectors: BotConnectorLinks,
    #[serde(
        default,
        alias = "skillProjects",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub skill_projects: Option<Vec<String>>,
    #[serde(
        default,
        alias = "sidebarOrder",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub sidebar_order: Option<u32>,
    #[serde(
        default,
        alias = "chatSessionId",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub chat_session_id: Option<String>,
    #[serde(
        default,
        alias = "lookAround",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub look_around: Option<BotLookAroundConfig>,
    #[serde(
        default,
        alias = "tidyUp",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub tidy_up: Option<BotTidyUpConfig>,
    #[serde(
        default,
        alias = "itemsStale",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub items_stale: Option<BotItemsStale>,
    #[serde(
        default,
        alias = "itemsSeen",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub items_seen: Option<u64>,
    #[serde(
        default,
        alias = "firstRun",
        deserialize_with = "deserialize_salvage_default"
    )]
    pub first_run: bool,
    #[serde(
        default,
        alias = "acceptedOffers",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub accepted_offers: Option<Vec<AcceptedBotOffer>>,
    #[serde(
        default,
        alias = "offerAnswers",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub offer_answers: Option<Vec<BotOfferAnswer>>,
    #[serde(
        default,
        alias = "handedPairedWorkspaces",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub handed_paired_workspaces: Option<Vec<HandedPairedWorkspace>>,
    #[serde(
        default,
        alias = "cancelledWaits",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub cancelled_waits: Option<Vec<BotCancelledWait>>,
    #[serde(
        default,
        alias = "importedFrom",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub imported_from: Option<BotImportedFrom>,
    #[serde(
        default,
        alias = "sharedFrom",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub shared_from: Option<BotSharedFrom>,
    #[serde(
        default,
        alias = "templateId",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_salvage_opt"
    )]
    pub template_id: Option<String>,
    #[serde(
        default,
        alias = "createdAt",
        deserialize_with = "deserialize_salvage_default"
    )]
    pub created_at: i64,
    #[serde(
        default,
        alias = "updatedAt",
        deserialize_with = "deserialize_salvage_default"
    )]
    pub updated_at: i64,
}

impl OrcaBotRecord {
    /// Validates collection caps and field lengths on this bot record.
    pub fn validate(&self) -> Result<(), BotErrorCode> {
        validate_members_cap(&self.members)?;
        self.connectors.validate()?;
        if let Some(ref projects) = self.skill_projects
            && projects.len() > BOT_MAX_SKILL_PROJECTS
        {
            return Err(BotErrorCode::Full);
        }
        if let Some(ref handed) = self.handed_paired_workspaces
            && handed.len() > BOT_HANDED_PAIRED_WORKSPACES_MAX
        {
            return Err(BotErrorCode::Full);
        }
        if let Some(ref tpl) = self.template_id
            && tpl.chars().count() > 64
        {
            return Err(BotErrorCode::InvalidArgument);
        }
        Ok(())
    }
}

/// Migrates retired agents, deserializes each record individually (dropping unreadable entries
/// rather than failing the fleet), deduplicates by `id` (keeping first), and tags untagged
/// legacy members with `source = Some("slack")`.
pub fn normalize_persisted_bots(values: &[serde_json::Value]) -> Vec<OrcaBotRecord> {
    let mut seen_ids = HashSet::new();
    let mut out = Vec::new();
    for raw in values {
        let mut cloned = raw.clone();
        migrate_retired_bot_record(&mut cloned);
        let Ok(mut bot) = serde_json::from_value::<OrcaBotRecord>(cloned) else {
            continue;
        };
        if bot.id.is_empty() || !seen_ids.insert(bot.id.clone()) {
            continue;
        }
        bot.members = with_slack_members_tagged(&bot.members);
        out.push(bot);
    }
    out
}

// ---------------------------------------------------------------------------
// 6. ActsForAsker & "asked by <name> via <Bot>" Attribution Formatter
// ---------------------------------------------------------------------------

pub const ASKED_BY_MAX_CHARS: usize = 140;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorSource {
    Slack,
    Github,
    Linear,
    Discord,
    Telegram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAskerRole {
    Owner,
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAskerPlaceKind {
    DirectMessage,
    GroupMessage,
    PublicChannel,
    PrivateChannel,
    Channel,
    Issue,
    PullRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActsForAsker {
    Owner,
    Person {
        source: ConnectorSource,
        #[serde(alias = "userId")]
        user_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(
            default,
            alias = "workspaceId",
            skip_serializing_if = "Option::is_none"
        )]
        workspace_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<ConnectorAskerRole>,
        #[serde(default, alias = "placeKind", skip_serializing_if = "Option::is_none")]
        place_kind: Option<ConnectorAskerPlaceKind>,
    },
    Unknown,
}

/// Returns `true` iff `askers` is non-empty and EVERY entry is `Owner` or a `Person` with `role == Some(Owner)`.
pub fn acts_for_owner(askers: &[ActsForAsker]) -> bool {
    !askers.is_empty()
        && askers.iter().all(|a| {
            matches!(
                a,
                ActsForAsker::Owner
                    | ActsForAsker::Person {
                        role: Some(ConnectorAskerRole::Owner),
                        ..
                    }
            )
        })
}

/// Deduplicates askers preserving first occurrence, keyed by `(source, user_id, workspace_id, role)`
/// for `Person` entries and variant discriminant for `Owner`/`Unknown`.
pub fn distinct_askers(askers: &[ActsForAsker]) -> Vec<ActsForAsker> {
    let mut seen_owner = false;
    let mut seen_unknown = false;
    let mut seen_people: HashSet<(
        ConnectorSource,
        String,
        Option<String>,
        Option<ConnectorAskerRole>,
    )> = HashSet::new();
    let mut out = Vec::new();
    for asker in askers {
        match asker {
            ActsForAsker::Owner => {
                if !seen_owner {
                    seen_owner = true;
                    out.push(asker.clone());
                }
            }
            ActsForAsker::Unknown => {
                if !seen_unknown {
                    seen_unknown = true;
                    out.push(asker.clone());
                }
            }
            ActsForAsker::Person {
                source,
                user_id,
                workspace_id,
                role,
                ..
            } => {
                let key = (*source, user_id.clone(), workspace_id.clone(), *role);
                if seen_people.insert(key) {
                    out.push(asker.clone());
                }
            }
        }
    }
    out
}

/// Returns `"owner"` when acting for the owner, `"member:<user_id>"` when acting for exactly
/// one distinct `Person`, or `None` when ambiguous/empty/unknown.
pub fn thread_acts_for(askers: &[ActsForAsker]) -> Option<String> {
    if acts_for_owner(askers) {
        return Some("owner".to_string());
    }
    let uniq = distinct_askers(askers);
    if uniq.len() == 1
        && let ActsForAsker::Person { user_id, .. } = &uniq[0]
    {
        return Some(format!("member:{user_id}"));
    }
    None
}

/// Formats `"asked by <name> via <Bot>"` bounded to `ASKED_BY_MAX_CHARS` (140).
/// Returns `None` when the owner typed the message directly, when acting strictly for the
/// owner, or when no non-owner `Person` asker is present.
pub fn format_asked_by_line(
    askers: &[ActsForAsker],
    bot_name: &str,
    typed_by_owner: bool,
) -> Option<String> {
    if typed_by_owner || acts_for_owner(askers) {
        return None;
    }
    let bot_clean = bot_name.trim();
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for asker in askers {
        if let ActsForAsker::Person {
            user_id,
            name,
            role,
            ..
        } = asker
        {
            if *role == Some(ConnectorAskerRole::Owner) {
                continue;
            }
            let display = name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| user_id.trim());
            if !display.is_empty() && seen.insert(display.to_lowercase()) {
                names.push(display.to_string());
            }
        }
    }
    if names.is_empty() {
        return None;
    }

    for shown in (1..=names.len()).rev() {
        let others = names.len() - shown;
        let joined = names[..shown].join(", ");
        let candidate = if others == 0 {
            format!("asked by {joined} via {bot_clean}")
        } else if others == 1 {
            format!("asked by {joined} and 1 other via {bot_clean}")
        } else {
            format!("asked by {joined} and {others} others via {bot_clean}")
        };
        if candidate.chars().count() <= ASKED_BY_MAX_CHARS {
            return Some(candidate);
        }
    }

    let prefix = "asked by ";
    let ellipsis = "\u{2026}";
    let others = names.len().saturating_sub(1);
    let mut suffix = if others == 0 {
        format!(" via {bot_clean}")
    } else if others == 1 {
        format!(" and 1 other via {bot_clean}")
    } else {
        format!(" and {others} others via {bot_clean}")
    };
    let fixed = prefix.chars().count() + ellipsis.chars().count() + suffix.chars().count();
    if fixed >= ASKED_BY_MAX_CHARS {
        suffix = format!(" via {bot_clean}");
    }
    let fixed = prefix.chars().count() + ellipsis.chars().count() + suffix.chars().count();
    let budget = ASKED_BY_MAX_CHARS.saturating_sub(fixed);
    let truncated: String = names[0]
        .chars()
        .take(budget)
        .collect::<String>()
        .trim_end()
        .to_string();
    let result = format!("{prefix}{truncated}{ellipsis}{suffix}");
    if result.chars().count() <= ASKED_BY_MAX_CHARS {
        Some(result)
    } else {
        Some(result.chars().take(ASKED_BY_MAX_CHARS).collect())
    }
}

/// Appends `"\n\n{asked_by_line}"` to `body` unless already present (case-insensitive)
/// or no attribution line is required.
pub fn with_asked_by(
    body: &str,
    askers: &[ActsForAsker],
    bot_name: &str,
    typed_by_owner: bool,
) -> String {
    let Some(line) = format_asked_by_line(askers, bot_name, typed_by_owner) else {
        return body.to_string();
    };
    if body.to_lowercase().contains(&line.to_lowercase()) {
        return body.to_string();
    }
    let trimmed = body.trim_end();
    if trimmed.is_empty() {
        line
    } else {
        format!("{trimmed}\n\n{line}")
    }
}

// ---------------------------------------------------------------------------
// 7. BotToolName, Smart Auto-Namer & Header/Env/URL Validators
// ---------------------------------------------------------------------------

pub const BOT_TOOL_NAME_NOISE: [&str; 10] = [
    "mcp", "server", "servers", "api", "www", "app", "com", "io", "dev", "ai",
];

/// Validated Bot MCP/HTTP tool name matching `^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BotToolName(String);

impl BotToolName {
    pub fn new(raw: &str) -> Result<Self, BotErrorCode> {
        let bytes = raw.as_bytes();
        if bytes.is_empty() || bytes.len() > 64 {
            return Err(BotErrorCode::InvalidArgument);
        }
        if !bytes[0].is_ascii_alphanumeric() {
            return Err(BotErrorCode::InvalidArgument);
        }
        if !bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
        {
            return Err(BotErrorCode::InvalidArgument);
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for BotToolName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BotToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BotToolName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

fn meaningful_tool_slug(words: &[&str]) -> String {
    let mut kept = Vec::new();
    for word in words {
        let clean: String = word
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if !clean.is_empty() && !BOT_TOOL_NAME_NOISE.contains(&clean.as_str()) {
            kept.push(clean);
        }
    }
    let joined = kept.join("-");
    if joined.len() > 64 {
        joined[..64].trim_end_matches('-').to_string()
    } else {
        joined
    }
}

fn strip_http_scheme(s: &str) -> Option<&str> {
    if s.as_bytes()
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"https://"))
    {
        Some(&s[8..])
    } else if s
        .as_bytes()
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"http://"))
    {
        Some(&s[7..])
    } else {
        None
    }
}

fn extract_url_host(detail: &str) -> Option<&str> {
    let rest = strip_http_scheme(detail)?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    (!host.is_empty()).then_some(host)
}

fn extract_url_path_segments(detail: &str) -> Vec<&str> {
    let Some(rest) = strip_http_scheme(detail) else {
        return Vec::new();
    };
    let Some(slash_idx) = rest.find('/') else {
        return Vec::new();
    };
    let after_host = &rest[slash_idx + 1..];
    let path_only = after_host
        .split(['?', '#'])
        .next()
        .unwrap_or(after_host);
    path_only
        .split(['/', '.', '_', '-'])
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_loopback_or_ip_host(host: &str) -> bool {
    let lower = host.trim().to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return true;
    }
    lower.parse::<std::net::IpAddr>().is_ok()
}

/// Derives a concise, valid `BotToolName` from an HTTP(S) URL or stdio CLI command.
pub fn bot_tool_name_from(detail: &str) -> BotToolName {
    let trimmed = detail.trim();
    if let Some(host) = extract_url_host(trimmed) {
        if is_loopback_or_ip_host(host) {
            let path_parts = extract_url_path_segments(trimmed);
            let path_slug = meaningful_tool_slug(&path_parts);
            if let Ok(name) = BotToolName::new(&path_slug) {
                return name;
            }
        }
        let host_parts: Vec<&str> = host.split('.').collect();
        let slug = meaningful_tool_slug(&host_parts);
        if let Ok(name) = BotToolName::new(&slug) {
            return name;
        }
        let path_parts = extract_url_path_segments(trimmed);
        let path_slug = meaningful_tool_slug(&path_parts);
        if let Ok(name) = BotToolName::new(&path_slug) {
            return name;
        }
        return BotToolName("tool".to_string());
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return BotToolName("tool".to_string());
    }
    let chosen = parts
        .iter()
        .skip(1)
        .find(|p| !p.starts_with('-'))
        .copied()
        .unwrap_or(parts[0]);

    let (scope, raw_base) = if let Some(after_at) = chosen.strip_prefix('@') {
        if let Some((sc, rest)) = after_at.split_once('/') {
            (Some(sc), rest)
        } else {
            (None, chosen)
        }
    } else {
        (None, chosen)
    };

    let base_no_ver = match raw_base.rfind('@') {
        Some(idx) if idx > 0 => &raw_base[..idx],
        _ => raw_base,
    };
    let words: Vec<&str> = base_no_ver
        .split(['-', '_', '.', '/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    let mut slug = meaningful_tool_slug(&words);
    if slug.is_empty()
        && let Some(sc) = scope
    {
        let sc_words: Vec<&str> = sc
            .split(['-', '_', '.'])
            .filter(|s| !s.is_empty())
            .collect();
        slug = meaningful_tool_slug(&sc_words);
    }
    BotToolName::new(&slug).unwrap_or_else(|_| BotToolName("tool".to_string()))
}

/// Returns `name` if not in `taken`, else `<prefix>-2`, `<prefix>-3`, ... capped at 64 bytes.
pub fn free_bot_tool_name(name: &BotToolName, taken: &HashSet<String>) -> BotToolName {
    if !taken.contains(name.as_str()) {
        return name.clone();
    }
    let base = name.as_str();
    let mut n: u64 = 2;
    loop {
        let suffix = format!("-{n}");
        let max_prefix = 64usize.saturating_sub(suffix.len());
        let prefix = if base.len() > max_prefix {
            &base[..max_prefix]
        } else {
            base
        };
        let candidate = format!("{prefix}{suffix}");
        if !taken.contains(&candidate)
            && let Ok(valid) = BotToolName::new(&candidate)
        {
            return valid;
        }
        n = n.saturating_add(1);
    }
}

/// Validates RFC 9110 HTTP token header name (`^[!#$%&'*+.^_`|~0-9A-Za-z-]+$`).
pub fn is_valid_bot_tool_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Validates POSIX environment variable name (`^[A-Za-z_][A-Za-z0-9_]*$`).
pub fn is_valid_bot_tool_env_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

fn is_valid_tool_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|p| p > 0)
}

fn is_valid_tool_hostname(host: &str) -> bool {
    if host.is_empty()
        || host.starts_with('.')
        || host.ends_with('.')
        || host.starts_with('-')
        || host.ends_with('-')
        || host.contains("..")
    {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// Validates an HTTP(S) tool endpoint URL (`<= 2048` bytes, no userinfo `@` in authority,
/// permitting `@` inside the path such as `https://server.smithery.ai/@owner/mcp`).
pub fn is_valid_bot_tool_url(url: &str) -> bool {
    if url.len() > 2048 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some(rest) = strip_http_scheme(url) else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(after_open) = authority.strip_prefix('[') {
        let Some((ipv6, port_part)) = after_open.split_once(']') else {
            return false;
        };
        if ipv6.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        if let Some(port) = port_part.strip_prefix(':') {
            return is_valid_tool_port(port);
        }
        return port_part.is_empty();
    }
    let (host, port_opt) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if !is_valid_tool_hostname(host) {
        return false;
    }
    if let Some(port) = port_opt {
        return is_valid_tool_port(port);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_png(width: u32, height: u32, extra_bytes: usize) -> Vec<u8> {
        let mut buf = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        buf.extend_from_slice(&13u32.to_be_bytes());
        buf.extend_from_slice(b"IHDR");
        buf.extend_from_slice(&width.to_be_bytes());
        buf.extend_from_slice(&height.to_be_bytes());
        buf.resize(buf.len() + extra_bytes, 0u8);
        buf
    }

    fn sample_bot(id: &str, name: &str, agent: BotAgentId) -> OrcaBotRecord {
        OrcaBotRecord {
            id: id.to_string(),
            name: BotName::new(name).unwrap(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            folder: bot_folder_name(name),
            folder_workspace_id: None,
            agent,
            avatar_spec: BotAvatarSpec::new_with_mascot(BotMascotId::A1),
            paused: false,
            permission_mode: None,
            launch: None,
            notifications: BotNotifications::default(),
            members: Vec::new(),
            connectors: BotConnectorLinks::default(),
            skill_projects: None,
            sidebar_order: None,
            chat_session_id: None,
            look_around: None,
            tidy_up: None,
            items_stale: None,
            items_seen: None,
            first_run: false,
            accepted_offers: None,
            offer_answers: None,
            handed_paired_workspaces: None,
            cancelled_waits: None,
            imported_from: None,
            shared_from: None,
            template_id: None,
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[test]
    fn bot_agents_22_live_retired_migration_and_older_build_split() {
        assert_eq!(BotAgentId::ALL.len(), 22);
        for (id, wire) in BotAgentId::ALL.into_iter().zip(BOT_AGENTS) {
            assert_eq!(id.as_str(), wire);
            assert_eq!(BotAgentId::parse_strict(wire), Some(id));
            let s = serde_json::to_string(&id).unwrap();
            assert_eq!(s, format!("\"{wire}\""));
            let back: BotAgentId = serde_json::from_str(&s).unwrap();
            assert_eq!(back, id);
        }
        assert_eq!(BotAgentId::parse_strict("gemini"), None);
        assert_eq!(
            migrate_retired_agent_id("gemini"),
            Some((BotAgentId::Antigravity, true))
        );
        assert_eq!(
            migrate_retired_agent_id("mimo-code"),
            Some((BotAgentId::MimoCode, false))
        );
        assert_eq!(migrate_retired_agent_id("unknown"), None);

        let mut rec = json!({
            "id": "b1",
            "name": "Gem",
            "folder": "gem",
            "agent": "gemini",
            "launch": { "model": "gemini-2.5-pro" }
        });
        assert!(migrate_retired_bot_record(&mut rec));
        assert_eq!(rec["agent"], "antigravity");
        assert!(rec.get("launch").is_none());

        let bots = vec![
            sample_bot("1", "A", BotAgentId::Claude),
            sample_bot("2", "B", BotAgentId::Cursor),
            sample_bot("3", "C", BotAgentId::Codex),
            sample_bot("4", "D", BotAgentId::Cline),
            sample_bot("5", "E", BotAgentId::Antigravity),
            sample_bot("6", "F", BotAgentId::Codebuddy),
        ];
        let split = split_bots_by_older_build_support(&bots);
        assert_eq!(
            split.readable.iter().map(|b| b.agent).collect::<Vec<_>>(),
            vec![BotAgentId::Claude, BotAgentId::Codex, BotAgentId::Antigravity]
        );
        assert_eq!(
            split.newer.iter().map(|b| b.agent).collect::<Vec<_>>(),
            vec![BotAgentId::Cursor, BotAgentId::Cline, BotAgentId::Codebuddy]
        );
    }

    #[test]
    fn bot_name_rejects_control_chars_before_trim_and_handles_unicode_len() {
        assert_eq!(BotName::new("  Scout  ").unwrap().as_str(), "Scout");
        let max_emoji = "\u{1f916}".repeat(64);
        assert_eq!(BotName::new(&max_emoji).unwrap().as_str(), max_emoji);
        let too_long = "\u{1f916}".repeat(65);
        assert_eq!(BotName::new(&too_long), Err(BotErrorCode::InvalidArgument));

        for bad in [
            "",
            "   ",
            "A\nB",
            "Bot\n",
            "\nBot",
            "A\rB",
            "A\tB",
            "A\0B",
            "A\x1bB",
            "A\x7fB",
            "A\u{0085}B",
            "A\u{009f}B",
        ] {
            assert_eq!(
                BotName::new(bad),
                Err(BotErrorCode::InvalidArgument),
                "expected invalid for {bad:?}"
            );
        }

        let generated = new_bot_name("", &["new bot", "New Bot 2"]).unwrap();
        assert_eq!(generated.as_str(), "New bot 3");
        assert_eq!(
            new_bot_name(" \n ", &["x"]),
            Err(BotErrorCode::InvalidArgument)
        );
    }

    #[test]
    fn bot_folder_slug_and_windows_reserved_names() {
        assert_eq!(BotFolderSlug::new("my-bot-1").unwrap().as_str(), "my-bot-1");
        assert_eq!(BotFolderSlug::new("a").unwrap().as_str(), "a");
        for bad in ["", "../x", "a/b", "a\\b", "MyBot", "a_b", "-a", "a-", "a--b", "."] {
            assert_eq!(
                BotFolderSlug::new(bad),
                Err(BotErrorCode::InvalidArgument),
                "expected invalid slug for {bad:?}"
            );
        }
        assert_eq!(bot_folder_name("CON").as_str(), "con-bot");
        assert_eq!(bot_folder_name("lpt9").as_str(), "lpt9-bot");
        assert_eq!(bot_folder_name("COM0").as_str(), "com0-bot");
        assert_eq!(bot_folder_name("NUL").as_str(), "nul-bot");
        assert_eq!(bot_folder_name("!!!").as_str(), "bot");
        assert_eq!(bot_folder_name("PR Review Bot!").as_str(), "pr-review-bot");
    }

    #[test]
    fn bot_avatar_mascots_outfits_png_and_cas() {
        assert_eq!(BotMascotId::ALL.len(), 20);
        assert_eq!(BotMascotId::PICKABLE.len(), 19);
        assert!(BotMascotId::S2.is_retired());
        assert!(!BotMascotId::A1.is_retired());
        assert_eq!(BotMascotId::parse("C1"), None);
        assert_eq!(BotMascotId::parse("T1"), Some(BotMascotId::T1));
        assert_eq!(BotAvatarOutfit::ALL.len(), 9);
        assert_eq!(worn_bot_outfit(Some("hard-hat")), Some(BotAvatarOutfit::HardHat));
        assert_eq!(worn_bot_outfit(Some("space-suit")), None);
        assert!(is_valid_stored_outfit("space-suit"));

        assert!(is_dark_bot_avatar_color("#1f2126"));
        assert!(!is_dark_bot_avatar_color("#ffffff"));

        let ok_png = make_png(512, 512, 16);
        assert_eq!(validate_bot_avatar_png(&ok_png), Ok((512, 512)));
        assert_eq!(
            validate_bot_avatar_png(&make_png(511, 511, 0)),
            Err(BotErrorCode::BotAvatarInvalid)
        );
        assert_eq!(
            validate_bot_avatar_png(&make_png(512, 600, 0)),
            Err(BotErrorCode::BotAvatarInvalid)
        );
        assert_eq!(
            validate_bot_avatar_png(&make_png(512, 512, BOT_AVATAR_MAX_BYTES)),
            Err(BotErrorCode::BotAvatarInvalid)
        );

        let mut spec = BotAvatarSpec::new_with_mascot(BotMascotId::H2);
        let epoch = spec.avatar_revision_epoch.clone();
        assert_eq!(
            spec.apply_manual_look_update(Some("upload:abc".into()), None, None),
            Err(BotErrorCode::BotAvatarInvalid)
        );
        assert_eq!(
            spec.apply_custom_png_upload(&ok_png, Some(99), Some(&epoch)),
            Err(BotErrorCode::BotAvatarChanged)
        );
        assert_eq!(
            spec.apply_custom_png_upload(&ok_png, Some(0), Some("wrong-epoch")),
            Err(BotErrorCode::BotAvatarChanged)
        );
        let ver = spec
            .apply_custom_png_upload(&ok_png, Some(0), Some(&epoch))
            .unwrap();
        assert_eq!(ver.len(), 16);
        assert!(spec.custom_png);
        assert_eq!(spec.avatar_revision, 1);
        assert_eq!(spec.generated_over_mascot.as_deref(), Some("mascot:H2"));
    }

    #[test]
    fn google_accounts_fail_closed_direct_and_flattened_in_bot_record() {
        let absent: BotConnectorLinks = serde_json::from_value(json!({})).unwrap();
        assert_eq!(absent.google_accounts, None);

        for malformed in [
            json!({ "google_accounts": null }),
            json!({ "googleAccounts": null }),
            json!({ "google_accounts": "a@b.com" }),
            json!({ "google_accounts": 123 }),
            json!({ "google_accounts": true }),
            json!({ "google_accounts": {} }),
            json!({ "google_accounts": [""] }),
            json!({ "google_accounts": [123] }),
            json!({ "google_accounts": ["x".repeat(321)] }),
            json!({ "google_accounts": (0..101).map(|i| format!("u{i}@a.com")).collect::<Vec<_>>() }),
        ] {
            let parsed: BotConnectorLinks = serde_json::from_value(malformed.clone()).unwrap();
            assert_eq!(
                parsed.google_accounts,
                Some(Vec::new()),
                "must fail closed to Some([]) for {malformed}"
            );
        }

        let valid: BotConnectorLinks =
            serde_json::from_value(json!({ "google_accounts": ["a@b.com"] })).unwrap();
        assert_eq!(valid.google_accounts, Some(vec!["a@b.com".to_string()]));

        // Verify through #[serde(flatten)] inside OrcaBotRecord as well!
        let rec_absent: OrcaBotRecord = serde_json::from_value(json!({
            "id": "b1",
            "name": "Scout",
            "folder": "scout"
        }))
        .unwrap();
        assert_eq!(rec_absent.connectors.google_accounts, None);

        let rec_null: OrcaBotRecord = serde_json::from_value(json!({
            "id": "b1",
            "name": "Scout",
            "folder": "scout",
            "google_accounts": null
        }))
        .unwrap();
        assert_eq!(rec_null.connectors.google_accounts, Some(Vec::new()));

        let rec_bad_camel: OrcaBotRecord = serde_json::from_value(json!({
            "id": "b1",
            "name": "Scout",
            "folder": "scout",
            "googleAccounts": "leak@example.com"
        }))
        .unwrap();
        assert_eq!(rec_bad_camel.connectors.google_accounts, Some(Vec::new()));
    }

    #[test]
    fn bot_members_connector_caps_offer_answers_and_persisted_salvage() {
        assert!(is_allowed_connector_avatar_url(
            "https://ca.slack-edge.com/T1-U1-abc"
        ));
        assert!(is_allowed_connector_avatar_url(
            "https://avatars.githubusercontent.com/u/42"
        ));
        assert!(is_allowed_connector_avatar_url(
            "https://public.linear.app/a/b.png"
        ));
        assert!(!is_allowed_connector_avatar_url(
            "http://ca.slack-edge.com/T1"
        ));
        assert!(!is_allowed_connector_avatar_url(
            "https://user@ca.slack-edge.com/T1"
        ));
        assert!(!is_allowed_connector_avatar_url(
            "https://ca.slack-edge.com:443/T1"
        ));
        assert!(!is_allowed_connector_avatar_url(
            "https://evil.com/x.png"
        ));

        let member: BotMember = serde_json::from_value(json!({
            "id": "U1",
            "name": "Dana",
            "avatarUrl": "https://evil.com/a.png",
            "kind": "future_kind"
        }))
        .unwrap();
        assert_eq!(member.avatar_url, None);
        assert_eq!(member.kind, None);
        assert_eq!(bot_member_source(&member), "slack");

        let too_many_members: Vec<BotMember> = (0..=BOT_MAX_MEMBERS)
            .map(|i| BotMember {
                id: format!("U{i}"),
                name: format!("User {i}"),
                source: Some("slack".into()),
                workspace_id: None,
                avatar_url: None,
                kind: None,
            })
            .collect();
        assert_eq!(validate_members_cap(&too_many_members), Err(BotErrorCode::Full));

        let too_many_connectors = BotConnectorLinks {
            slack_apps: Some(
                (0..=BOT_MAX_CONNECTOR_WORKSPACES)
                    .map(|i| BotSlackLink {
                        app_id: format!("A{i}"),
                        team_id: None,
                        owner_user_id: None,
                    })
                    .collect(),
            ),
            ..Default::default()
        };
        assert_eq!(too_many_connectors.validate(), Err(BotErrorCode::Full));

        let answers = apply_bot_offer_answer(
            &[],
            BotOfferAnswerUpdate::Claim {
                offer_id: "o1".into(),
                state: BotOfferAnswerState::Sent,
                answered_at: 10,
            },
        )
        .unwrap();
        assert_eq!(
            apply_bot_offer_answer(
                &answers,
                BotOfferAnswerUpdate::Claim {
                    offer_id: "o1".into(),
                    state: BotOfferAnswerState::Dismissed,
                    answered_at: 20,
                }
            ),
            Err(BotErrorCode::BotOfferAlreadyAnswered)
        );
        let released = apply_bot_offer_answer(
            &answers,
            BotOfferAnswerUpdate::Release {
                offer_id: "o1".into(),
            },
        )
        .unwrap();
        assert!(released.is_empty());

        let normalized = normalize_persisted_bots(&[
            json!({
                "id": "b1",
                "name": "Gem",
                "folder": "gem",
                "agent": "gemini",
                "launch": { "model": "old" },
                "permissionMode": "future-mode",
                "itemsStale": "never",
                "members": [{ "id": "U1", "name": "Dana" }]
            }),
            json!({ "id": "b1", "name": "Dup", "folder": "dup" }),
            json!({ "id": "bad", "name": "A\nB", "folder": "bad" }),
        ]);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].agent, BotAgentId::Antigravity);
        assert_eq!(normalized[0].launch, None);
        assert_eq!(normalized[0].permission_mode, None);
        assert_eq!(normalized[0].items_stale, Some(BotItemsStale::Never));
        assert_eq!(normalized[0].members[0].source.as_deref(), Some("slack"));
    }

    #[test]
    fn acts_for_asker_and_140_char_attribution_formatter() {
        assert!(acts_for_owner(&[ActsForAsker::Owner]));
        assert!(!acts_for_owner(&[]));

        let carol = ActsForAsker::Person {
            source: ConnectorSource::Slack,
            user_id: "U_CAROL".into(),
            name: Some("Carol".into()),
            workspace_id: Some("T1".into()),
            role: Some(ConnectorAskerRole::Member),
            place_kind: Some(ConnectorAskerPlaceKind::DirectMessage),
        };
        assert_eq!(
            format_asked_by_line(&[ActsForAsker::Owner], "Scout", false),
            None
        );
        assert_eq!(
            format_asked_by_line(std::slice::from_ref(&carol), "Scout", true),
            None
        );
        assert_eq!(
            format_asked_by_line(std::slice::from_ref(&carol), "Scout", false),
            Some("asked by Carol via Scout".to_string())
        );
        assert_eq!(
            thread_acts_for(std::slice::from_ref(&carol)),
            Some("member:U_CAROL".to_string())
        );
        assert_eq!(
            with_asked_by("Done.", std::slice::from_ref(&carol), "Scout", false),
            "Done.\n\nasked by Carol via Scout"
        );
        assert_eq!(
            with_asked_by("Done.\n\nAsked by carol via scout", &[carol], "Scout", false),
            "Done.\n\nAsked by carol via scout"
        );

        let many: Vec<ActsForAsker> = (1..=10)
            .map(|i| ActsForAsker::Person {
                source: ConnectorSource::Slack,
                user_id: format!("U{i}"),
                name: Some(format!("LongContributorNameNumber{i:02}")),
                workspace_id: None,
                role: Some(ConnectorAskerRole::Member),
                place_kind: None,
            })
            .collect();
        let line = format_asked_by_line(&many, "Scout", false).unwrap();
        assert!(line.chars().count() <= ASKED_BY_MAX_CHARS);
        assert!(line.contains("others via Scout"));

        let huge_single = vec![ActsForAsker::Person {
            source: ConnectorSource::Github,
            user_id: "u1".into(),
            name: Some("X".repeat(200)),
            workspace_id: None,
            role: Some(ConnectorAskerRole::Member),
            place_kind: None,
        }];
        let trunc = format_asked_by_line(&huge_single, "Scout", false).unwrap();
        assert!(trunc.chars().count() <= ASKED_BY_MAX_CHARS);
        assert!(trunc.ends_with("\u{2026} via Scout"));
    }

    #[test]
    fn bot_tool_name_auto_namer_and_validators() {
        assert_eq!(
            bot_tool_name_from("https://mcp.notion.com/sse").as_str(),
            "notion"
        );
        assert_eq!(
            bot_tool_name_from("npx -y @sentry/mcp-server@1.2.3").as_str(),
            "sentry"
        );
        assert_eq!(
            bot_tool_name_from("https://mcp.api.dev.io").as_str(),
            "tool"
        );
        let taken: HashSet<String> = ["notion".to_string(), "notion-2".to_string()]
            .into_iter()
            .collect();
        let free = free_bot_tool_name(&BotToolName::new("notion").unwrap(), &taken);
        assert_eq!(free.as_str(), "notion-3");

        let max64 = BotToolName::new(&"a".repeat(64)).unwrap();
        let taken64: HashSet<String> = [max64.as_str().to_string()].into_iter().collect();
        let free64 = free_bot_tool_name(&max64, &taken64);
        assert_eq!(free64.as_str().len(), 64);
        assert!(free64.as_str().ends_with("-2"));

        let future_spec: BotAvatarSpec =
            serde_json::from_value(json!({ "avatarOutfit": "space-suit", "avatarColor": "#b3dbf4" }))
                .unwrap();
        assert_eq!(future_spec.avatar_outfit.as_deref(), Some("space-suit"));
        assert_eq!(future_spec.worn_outfit(), None);

        assert!(is_valid_bot_tool_header_name("X-Api-Key"));
        assert!(!is_valid_bot_tool_header_name("Bad Header"));
        assert!(is_valid_bot_tool_env_name("GITHUB_TOKEN"));
        assert!(!is_valid_bot_tool_env_name("1BAD"));
        assert!(is_valid_bot_tool_url("https://server.smithery.ai/@owner/mcp"));
        assert!(!is_valid_bot_tool_url("https://user:pass@evil.com/mcp"));
        assert!(!is_valid_bot_tool_url("ftp://example.com/mcp"));
    }

    #[test]
    fn utf8_multibyte_tool_url_and_name_never_panic() {
        for input in [
            "中中中",
            "a🤖🤖",
            "ééééx",
            "nx 🚀🚀",
            "日本語URL",
            "你好世界",
            "🔥🔥🔥",
        ] {
            assert!(!is_valid_bot_tool_url(input), "expected false for {input:?}");
            let name = bot_tool_name_from(input);
            assert!(
                !name.as_str().is_empty() && name.as_str().len() <= 64,
                "invalid derived tool name for {input:?}: {name}"
            );
        }
    }

    #[test]
    fn manual_avatar_trim_control_and_upload_bypass_guards() {
        let mut spec = BotAvatarSpec::new_with_mascot(BotMascotId::A1);
        let original = spec.clone();

        for bad in [
            "upload:abc",
            " upload:abc",
            "\u{00a0}upload:abc",
            "   ",
            " mascot:NOPE",
            "\nmascot:A1",
            "mascot:A1\n",
            "a\nb",
            &"🤖".repeat(33),
        ] {
            assert_eq!(
                spec.apply_manual_look_update(Some(bad.to_string()), None, None),
                Err(BotErrorCode::BotAvatarInvalid),
                "must reject {bad:?}"
            );
            assert_eq!(spec, original, "failed update must not mutate spec");
        }

        spec.apply_manual_look_update(Some("  mascot:H2  ".into()), Some("#b3dbf4".into()), None)
            .unwrap();
        assert_eq!(spec.avatar.as_deref(), Some("mascot:H2"));
        assert_eq!(spec.parsed(), Some(ParsedBotAvatar::Mascot(BotMascotId::H2)));

        spec.apply_manual_look_update(Some("  🤖  ".into()), None, None)
            .unwrap();
        assert_eq!(spec.avatar.as_deref(), Some("🤖"));
        assert_eq!(spec.parsed(), Some(ParsedBotAvatar::Emoji("🤖".into())));
    }

    #[test]
    fn upstream_notifications_slack_mirror_and_host_mcp_servers_schema() {
        let upstream = json!({
            "id": "b-up",
            "name": "UpstreamBot",
            "folder": "upstream-bot",
            "slackMirror": false,
            "hostMcpServers": true,
            "notifications": {
                "needsPermission": false,
                "taskComplete": true,
                "couldNotFinish": false
            }
        });
        let rec: OrcaBotRecord = serde_json::from_value(upstream.clone()).unwrap();
        assert_eq!(rec.connectors.slack_mirror, Some(false));
        assert_eq!(rec.connectors.host_mcp_servers, Some(true));
        assert!(rec.connectors.uses_host_mcp_servers());
        assert!(!rec.connectors.has_connector());
        assert!(!rec.notifications.needs_permission);
        assert!(rec.notifications.task_complete);
        assert!(!rec.notifications.could_not_finish);

        let rec_str: OrcaBotRecord =
            serde_json::from_str(&serde_json::to_string(&upstream).unwrap()).unwrap();
        assert_eq!(rec_str, rec);

        // Partial malformed notifications salvage only malformed fields to true.
        let partial: BotNotifications = serde_json::from_value(json!({
            "needsPermission": false,
            "taskComplete": "bad",
            "couldNotFinish": null
        }))
        .unwrap();
        assert!(!partial.needs_permission);
        assert!(partial.task_complete);
        assert!(partial.could_not_finish);

        // Malformed slackMirror / hostMcpServers salvage to None.
        let bad_conn: BotConnectorLinks = serde_json::from_value(json!({
            "slackMirror": { "enabled": true },
            "hostMcpServers": ["legacy-array"],
            "google": "yes"
        }))
        .unwrap();
        assert_eq!(bad_conn.slack_mirror, None);
        assert_eq!(bad_conn.host_mcp_servers, None);
        assert_eq!(bad_conn.google, None);
    }

    #[test]
    fn tool_url_rejects_malformed_hosts_invalid_ipv6_and_signed_ports() {
        for bad in [
            "https://.../sse",
            "https://---/sse",
            "https://.example.com/sse",
            "https://example..com/sse",
            "https://-bad.com/sse",
            "https://bad-.com/sse",
            "https://a.-b.com/sse",
            "https://[not-ipv6!!!$%]/sse",
            "https://example.com:+443/sse",
            "https://[::1]:+8080/sse",
            "https://example.com:/sse",
            "https://example.com:0/sse",
            "https://example.com:65536/sse",
        ] {
            assert!(!is_valid_bot_tool_url(bad), "must reject malformed URL {bad}");
        }

        for good in [
            "https://[::1]:8080/sse",
            "https://[2001:db8::1]/mcp",
            "http://localhost:3000/mcp",
            "https://mcp.example-1.com:443/@owner/mcp",
            "HTTPS://127.0.0.1:9000/sse",
        ] {
            assert!(is_valid_bot_tool_url(good), "must accept valid URL {good}");
        }
    }

    #[test]
    fn forward_compat_salvage_preserves_bot_and_keeps_google_accounts_fail_closed() {
        let raw = json!({
            "id": "b-salvage",
            "name": "SalvageBot",
            "folder": "salvage-bot",
            "hostId": null,
            "folderWorkspaceId": 12345,
            "avatar": 999,
            "generatedOverMascot": ["bad"],
            "customPng": "not-bool",
            "avatarRevision": "bad",
            "avatarRevisionEpoch": null,
            "paused": null,
            "firstRun": "bad",
            "notifications": "future-v2",
            "members": [
                { "id": "U1", "name": "Alice", "source": 123 },
                { "broken": true },
                { "id": "U2", "name": "Bob", "workspaceId": ["bad"] }
            ],
            "google": "future-obj",
            "googleAccounts": "leak@example.com",
            "sidebarOrder": "top",
            "chatSessionId": { "nested": 1 },
            "itemsSeen": "many",
            "templateId": 42,
            "createdAt": "now",
            "updatedAt": null
        });

        let via_value: OrcaBotRecord = serde_json::from_value(raw.clone()).unwrap();
        let via_str: OrcaBotRecord =
            serde_json::from_str(&serde_json::to_string(&raw).unwrap()).unwrap();
        assert_eq!(via_value, via_str);

        let normalized = normalize_persisted_bots(&[raw]);
        assert_eq!(normalized.len(), 1);
        let bot = &normalized[0];
        assert_eq!(bot.host_id, LOCAL_EXECUTION_HOST_ID);
        assert_eq!(bot.folder_workspace_id, None);
        assert_eq!(bot.avatar_spec.avatar, None);
        assert_eq!(bot.avatar_spec.generated_over_mascot, None);
        assert!(!bot.avatar_spec.custom_png);
        assert_eq!(bot.avatar_spec.avatar_revision, 0);
        assert_eq!(bot.avatar_spec.avatar_revision_epoch, "");
        assert!(!bot.paused);
        assert!(!bot.first_run);
        assert_eq!(bot.notifications, BotNotifications::default());
        assert_eq!(bot.members.len(), 2);
        assert_eq!(bot.members[0].id, "U1");
        assert_eq!(bot.members[0].source.as_deref(), Some("slack"));
        assert_eq!(bot.members[1].id, "U2");
        assert_eq!(bot.members[1].workspace_id, None);
        assert_eq!(bot.connectors.google, None);
        // CRITICAL: google_accounts remains strictly fail-closed to Some([]), NEVER None!
        assert_eq!(bot.connectors.google_accounts, Some(Vec::new()));
        assert_eq!(bot.sidebar_order, None);
        assert_eq!(bot.chat_session_id, None);
        assert_eq!(bot.items_seen, None);
        assert_eq!(bot.template_id, None);
        assert_eq!(bot.created_at, 0);
        assert_eq!(bot.updated_at, 0);
    }

    #[test]
    fn png_avatar_validates_exact_13_byte_ihdr_length() {
        let mut bad_len_zero = make_png(512, 512, 0);
        bad_len_zero[8..12].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(
            validate_bot_avatar_png(&bad_len_zero),
            Err(BotErrorCode::BotAvatarInvalid)
        );

        let mut bad_len_14 = make_png(512, 512, 0);
        bad_len_14[8..12].copy_from_slice(&14u32.to_be_bytes());
        assert_eq!(
            validate_bot_avatar_png(&bad_len_14),
            Err(BotErrorCode::BotAvatarInvalid)
        );

        let valid = make_png(512, 512, 0);
        assert_eq!(validate_bot_avatar_png(&valid), Ok((512, 512)));
    }
}
