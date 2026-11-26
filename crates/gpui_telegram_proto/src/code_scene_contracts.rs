//! Shared wire & domain contracts for Code Scene ("代码场景").
//!
//! Defines the authoritative DTOs shared across `gpui_telegram_code_scene`,
//! `gpui_telegram_agent`, `gpui_telegram_server`, `gpui_telegram_client`,
//! and `gpui_telegram_desktop`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Canonical source code format identifier for `CodeSceneScript` v1.
pub const CODE_SCENE_FORMAT_CS1: &str = "cs1";
/// Contract schema version.
pub const CODE_SCENE_SCHEMA_VERSION_V1: u32 = 1;

pub const MANIFEST_MIN_WIDTH: u32 = 160;
pub const MANIFEST_MAX_WIDTH: u32 = 720;
pub const MANIFEST_DEFAULT_WIDTH: u32 = 360;
pub const MANIFEST_MIN_HEIGHT: u32 = 160;
pub const MANIFEST_MAX_HEIGHT: u32 = 1280;
pub const MANIFEST_DEFAULT_HEIGHT: u32 = 640;
pub const MANIFEST_MIN_FPS: u32 = 15;
pub const MANIFEST_MAX_FPS: u32 = 60;
pub const MANIFEST_DEFAULT_FPS: u32 = 60;
pub const VIDEO_MIN_DURATION_SEC: f64 = 5.0;
pub const VIDEO_MAX_DURATION_SEC: f64 = 120.0;
pub const VIDEO_DEFAULT_DURATION_SEC: f64 = 15.0;

pub const DEFAULT_FEED_LIMIT: usize = 12;
pub const MAX_FEED_LIMIT: usize = 50;

pub const CODE_SCENE_ACTION_OPEN_PREFIX: &str = "code_scene:open:";
pub const CODE_SCENE_ACTION_REMIX_PREFIX: &str = "code_scene:remix:";
pub const CODE_SCENE_ACTION_SHARE_PREFIX: &str = "code_scene:share:";

/// Chat card / inline keyboard action protocol for opening, remixing, or sharing a scene.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "action", content = "scene_id", rename_all = "snake_case")]
pub enum CodeSceneCardAction {
    Open(String),
    Remix(String),
    Share(String),
}

impl CodeSceneCardAction {
    pub fn to_payload(&self) -> String {
        match self {
            Self::Open(id) => format!("{CODE_SCENE_ACTION_OPEN_PREFIX}{id}"),
            Self::Remix(id) => format!("{CODE_SCENE_ACTION_REMIX_PREFIX}{id}"),
            Self::Share(id) => format!("{CODE_SCENE_ACTION_SHARE_PREFIX}{id}"),
        }
    }

    pub fn parse_payload(payload: &str) -> Option<Self> {
        let (variant, raw_id) = if let Some(rest) = payload.strip_prefix(CODE_SCENE_ACTION_OPEN_PREFIX) {
            (0u8, rest)
        } else if let Some(rest) = payload.strip_prefix(CODE_SCENE_ACTION_REMIX_PREFIX) {
            (1u8, rest)
        } else if let Some(rest) = payload.strip_prefix(CODE_SCENE_ACTION_SHARE_PREFIX) {
            (2u8, rest)
        } else {
            return None;
        };
        let id = raw_id.trim();
        if id.is_empty() || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return None;
        }
        let owned = id.to_string();
        match variant {
            0 => Some(Self::Open(owned)),
            1 => Some(Self::Remix(owned)),
            _ => Some(Self::Share(owned)),
        }
    }
}

/// Executable modality of a Code Scene asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Video,
    #[default]
    Animation,
    Game,
}

impl AssetKind {
    pub const ALL: [Self; 3] = [Self::Video, Self::Animation, Self::Game];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Animation => "animation",
            Self::Game => "game",
        }
    }

    pub const fn label_zh(self) -> &'static str {
        match self {
            Self::Video => "视频",
            Self::Animation => "动画",
            Self::Game => "游戏",
        }
    }

    pub const fn label_en(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Animation => "Animation",
            Self::Game => "Game",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "video" | "videos" | "视频" | "代码视频" => Some(Self::Video),
            "animation" | "animations" | "anim" | "动画" | "代码动画" => Some(Self::Animation),
            "game" | "games" | "游戏" | "小游戏" | "代码游戏" => Some(Self::Game),
            _ => None,
        }
    }
}

/// Top-level vertical feed tabs in Code Scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSceneFeedTab {
    #[default]
    ForYou,
    Following,
    Videos,
    Animations,
    Games,
    MyWorks,
}

impl CodeSceneFeedTab {
    pub const ALL: [Self; 6] = [
        Self::ForYou,
        Self::Following,
        Self::Videos,
        Self::Animations,
        Self::Games,
        Self::MyWorks,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ForYou => "for_you",
            Self::Following => "following",
            Self::Videos => "videos",
            Self::Animations => "animations",
            Self::Games => "games",
            Self::MyWorks => "my_works",
        }
    }

    pub const fn label_zh(self) -> &'static str {
        match self {
            Self::ForYou => "推荐",
            Self::Following => "关注",
            Self::Videos => "视频",
            Self::Animations => "动画",
            Self::Games => "游戏",
            Self::MyWorks => "我的作品",
        }
    }

    pub const fn label_en(self) -> &'static str {
        match self {
            Self::ForYou => "For You",
            Self::Following => "Following",
            Self::Videos => "Videos",
            Self::Animations => "Animations",
            Self::Games => "Games",
            Self::MyWorks => "My Works",
        }
    }

    pub const fn filter_kind(self) -> Option<AssetKind> {
        match self {
            Self::Videos => Some(AssetKind::Video),
            Self::Animations => Some(AssetKind::Animation),
            Self::Games => Some(AssetKind::Game),
            Self::ForYou | Self::Following | Self::MyWorks => None,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "for_you" | "foryou" | "recommend" | "推荐" => Some(Self::ForYou),
            "following" | "follow" | "关注" => Some(Self::Following),
            "videos" | "video" | "视频" => Some(Self::Videos),
            "animations" | "animation" | "动画" => Some(Self::Animations),
            "games" | "game" | "游戏" => Some(Self::Games),
            "my_works" | "myworks" | "mine" | "我的作品" => Some(Self::MyWorks),
            _ => None,
        }
    }
}

/// Behavioral telemetry event kinds (Stage 0 signals for Douyin recommender).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneBehaviorEventKind {
    #[default]
    Impression,
    Dwell,
    Complete,
    Replay,
    GameInteraction,
    Like,
    DoubleTapLike,
    Unlike,
    Bookmark,
    Unbookmark,
    Comment,
    Share,
    FollowAuthor,
    UnfollowAuthor,
    InspectCode,
    Remix,
    FastSkip,
    NotInterested,
    HideAuthor,
    HideTag,
}

impl SceneBehaviorEventKind {
    pub const ALL: [Self; 20] = [
        Self::Impression,
        Self::Dwell,
        Self::Complete,
        Self::Replay,
        Self::GameInteraction,
        Self::Like,
        Self::DoubleTapLike,
        Self::Unlike,
        Self::Bookmark,
        Self::Unbookmark,
        Self::Comment,
        Self::Share,
        Self::FollowAuthor,
        Self::UnfollowAuthor,
        Self::InspectCode,
        Self::Remix,
        Self::FastSkip,
        Self::NotInterested,
        Self::HideAuthor,
        Self::HideTag,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Impression => "impression",
            Self::Dwell => "dwell",
            Self::Complete => "complete",
            Self::Replay => "replay",
            Self::GameInteraction => "game_interaction",
            Self::Like => "like",
            Self::DoubleTapLike => "double_tap_like",
            Self::Unlike => "unlike",
            Self::Bookmark => "bookmark",
            Self::Unbookmark => "unbookmark",
            Self::Comment => "comment",
            Self::Share => "share",
            Self::FollowAuthor => "follow_author",
            Self::UnfollowAuthor => "unfollow_author",
            Self::InspectCode => "inspect_code",
            Self::Remix => "remix",
            Self::FastSkip => "fast_skip",
            Self::NotInterested => "not_interested",
            Self::HideAuthor => "hide_author",
            Self::HideTag => "hide_tag",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|k| k.as_str() == normalized)
    }

    pub const fn is_explicit_positive(self) -> bool {
        matches!(
            self,
            Self::Like
                | Self::DoubleTapLike
                | Self::Bookmark
                | Self::Comment
                | Self::Share
                | Self::FollowAuthor
                | Self::InspectCode
                | Self::Remix
        )
    }

    pub const fn is_negative(self) -> bool {
        matches!(
            self,
            Self::FastSkip
                | Self::NotInterested
                | Self::HideAuthor
                | Self::HideTag
                | Self::Unlike
                | Self::Unbookmark
                | Self::UnfollowAuthor
        )
    }

    pub const fn is_hard_negative(self) -> bool {
        matches!(self, Self::NotInterested | Self::HideAuthor | Self::HideTag)
    }
}

/// Severity for script compilation & dry-run validation diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    #[default]
    Error,
    Warning,
}

/// Comment list sorting mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSceneCommentSort {
    #[default]
    Hot,
    Newest,
}

/// Execution manifest controlling logical stage dimensions, frame rate, duration, and deterministic seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetManifest {
    pub kind: AssetKind,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub duration_sec: Option<f64>,
    pub seed: u64,
}

impl Default for AssetManifest {
    fn default() -> Self {
        Self {
            kind: AssetKind::Animation,
            width: MANIFEST_DEFAULT_WIDTH,
            height: MANIFEST_DEFAULT_HEIGHT,
            fps: MANIFEST_DEFAULT_FPS,
            duration_sec: None,
            seed: 1,
        }
    }
}

impl AssetManifest {
    pub fn for_kind(kind: AssetKind) -> Self {
        Self {
            kind,
            duration_sec: (kind == AssetKind::Video).then_some(VIDEO_DEFAULT_DURATION_SEC),
            ..Self::default()
        }
    }

    pub fn clamped(&self) -> Self {
        let mut copy = self.clone();
        copy.clamp_in_place();
        copy
    }

    pub fn clamp_in_place(&mut self) {
        self.width = self.width.clamp(MANIFEST_MIN_WIDTH, MANIFEST_MAX_WIDTH);
        self.height = self.height.clamp(MANIFEST_MIN_HEIGHT, MANIFEST_MAX_HEIGHT);
        self.fps = self.fps.clamp(MANIFEST_MIN_FPS, MANIFEST_MAX_FPS);
        match self.kind {
            AssetKind::Video => {
                let dur = match self.duration_sec {
                    Some(d) if d.is_finite() => d.clamp(VIDEO_MIN_DURATION_SEC, VIDEO_MAX_DURATION_SEC),
                    _ => VIDEO_DEFAULT_DURATION_SEC,
                };
                self.duration_sec = Some(dur);
            }
            AssetKind::Animation | AssetKind::Game => {
                self.duration_sec = match self.duration_sec {
                    Some(d) if d.is_finite() && d > 0.0 => Some(d.clamp(1.0, 600.0)),
                    _ => None,
                };
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(MANIFEST_MIN_WIDTH..=MANIFEST_MAX_WIDTH).contains(&self.width) {
            return Err(format!(
                "manifest.width {} out of range {}..={}",
                self.width, MANIFEST_MIN_WIDTH, MANIFEST_MAX_WIDTH
            ));
        }
        if !(MANIFEST_MIN_HEIGHT..=MANIFEST_MAX_HEIGHT).contains(&self.height) {
            return Err(format!(
                "manifest.height {} out of range {}..={}",
                self.height, MANIFEST_MIN_HEIGHT, MANIFEST_MAX_HEIGHT
            ));
        }
        if !(MANIFEST_MIN_FPS..=MANIFEST_MAX_FPS).contains(&self.fps) {
            return Err(format!(
                "manifest.fps {} out of range {}..={}",
                self.fps, MANIFEST_MIN_FPS, MANIFEST_MAX_FPS
            ));
        }
        match self.kind {
            AssetKind::Video => match self.duration_sec {
                Some(d) if d.is_finite() && (VIDEO_MIN_DURATION_SEC..=VIDEO_MAX_DURATION_SEC).contains(&d) => Ok(()),
                Some(d) => Err(format!(
                    "manifest.duration_sec {d} out of range {VIDEO_MIN_DURATION_SEC}..={VIDEO_MAX_DURATION_SEC} for video"
                )),
                None => Err("manifest.duration_sec is required for video assets".to_string()),
            },
            AssetKind::Animation | AssetKind::Game => match self.duration_sec {
                None => Ok(()),
                Some(d) if d.is_finite() && d > 0.0 => Ok(()),
                Some(d) => Err(format!("manifest.duration_sec {d} must be finite and positive")),
            },
        }
    }
}

/// Full domain & wire DTO representing a Code Scene asset (exact 25 fields from PROJECT.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeSceneAssetDto {
    pub scene_id: String,
    pub title: String,
    pub description: String,
    pub kind: AssetKind,
    pub tags: Vec<String>,
    pub author_user_id: i64,
    pub author_username: String,
    pub creator_bot_id: Option<i64>,
    pub creator_bot_username: Option<String>,
    pub remix_parent_id: Option<String>,
    pub source_code: String,
    pub code_format: String,
    pub manifest: AssetManifest,
    pub controls_hint: String,
    pub plays_count: u64,
    pub likes_count: u64,
    pub bookmarks_count: u64,
    pub comments_count: u64,
    pub shares_count: u64,
    pub remixes_count: u64,
    pub viewer_liked: bool,
    pub viewer_bookmarked: bool,
    pub viewer_following_author: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Default for CodeSceneAssetDto {
    fn default() -> Self {
        Self {
            scene_id: String::new(),
            title: String::new(),
            description: String::new(),
            kind: AssetKind::Animation,
            tags: Vec::new(),
            author_user_id: 0,
            author_username: String::new(),
            creator_bot_id: None,
            creator_bot_username: None,
            remix_parent_id: None,
            source_code: String::new(),
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest: AssetManifest::default(),
            controls_hint: String::new(),
            plays_count: 0,
            likes_count: 0,
            bookmarks_count: 0,
            comments_count: 0,
            shares_count: 0,
            remixes_count: 0,
            viewer_liked: false,
            viewer_bookmarked: false,
            viewer_following_author: false,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }
}

impl CodeSceneAssetDto {
    /// Deterministic 64-char lowercase hex SHA-256 over `code_format + "\n" + source_code`.
    pub fn content_sha256(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.code_format.as_bytes());
        hasher.update(b"\n");
        hasher.update(self.source_code.as_bytes());
        let digest = hasher.finalize();
        let mut out = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "{b:02x}");
        }
        out
    }

    /// Trims whitespace, strips leading `#`, lowercases, and deduplicates preserving order.
    pub fn normalized_tags(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::with_capacity(self.tags.len());
        for raw in &self.tags {
            let clean = raw.trim().trim_start_matches('#').trim().to_ascii_lowercase();
            if !clean.is_empty() && seen.insert(clean.clone()) {
                out.push(clean);
            }
        }
        out
    }
}

/// Multi-task eCPM & re-ranking score breakdown (exact 10 f64 fields from PROJECT.md).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScoreBreakdown {
    pub embedding_sim: f64,
    pub swing_cf: f64,
    pub tag_kind_affinity: f64,
    pub creator_affinity: f64,
    pub quality_ecpm: f64,
    pub freshness: f64,
    pub exploration_bonus: f64,
    pub mmr_penalty: f64,
    pub negative_penalty: f64,
    pub final_score: f64,
}

impl ScoreBreakdown {
    pub fn sanitized(self) -> Self {
        let f = |v: f64| if v.is_finite() { v } else { 0.0 };
        Self {
            embedding_sim: f(self.embedding_sim),
            swing_cf: f(self.swing_cf),
            tag_kind_affinity: f(self.tag_kind_affinity),
            creator_affinity: f(self.creator_affinity),
            quality_ecpm: f(self.quality_ecpm),
            freshness: f(self.freshness),
            exploration_bonus: f(self.exploration_bonus),
            mmr_penalty: f(self.mmr_penalty),
            negative_penalty: f(self.negative_penalty),
            final_score: f(self.final_score),
        }
    }
}

/// Human-readable & programmatic explanation attached to every ranked item.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecommendationExplanation {
    pub recall_channels: Vec<String>,
    pub primary_reason_zh: String,
    pub primary_reason_en: String,
    pub traffic_tier: String,
    pub breakdown: ScoreBreakdown,
}

/// Ranked vertical feed item combining the executable asset and its recommendation explanation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RankedSceneItemDto {
    pub asset: CodeSceneAssetDto,
    pub explanation: RecommendationExplanation,
}

/// Vertical feed recommendation query (exact 7 fields from PROJECT.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeSceneFeedRequest {
    pub user_id: i64,
    pub session_id: String,
    pub tab: CodeSceneFeedTab,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    pub limit: usize,
}

impl Default for CodeSceneFeedRequest {
    fn default() -> Self {
        Self {
            user_id: 0,
            session_id: String::new(),
            tab: CodeSceneFeedTab::ForYou,
            tag_filter: None,
            query: None,
            cursor: None,
            limit: DEFAULT_FEED_LIMIT,
        }
    }
}

impl CodeSceneFeedRequest {
    pub fn clamped_limit(&self) -> usize {
        if self.limit == 0 {
            DEFAULT_FEED_LIMIT
        } else {
            self.limit.clamp(1, MAX_FEED_LIMIT)
        }
    }
}

/// Paginated ranked feed response (exact 3 fields from PROJECT.md).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeSceneFeedResponse {
    pub items: Vec<RankedSceneItemDto>,
    pub next_cursor: Option<String>,
    pub total_catalog: usize,
}

/// Single behavioral telemetry event sent from desktop/client to recommender.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneBehaviorEventDto {
    pub event_id: String,
    pub user_id: i64,
    pub session_id: String,
    pub scene_id: String,
    pub kind: SceneBehaviorEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_kind: Option<AssetKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_user_id: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub dwell_ms: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub completion_ratio: f64,
    #[serde(default)]
    pub loop_count: u32,
    #[serde(default)]
    pub game_inputs: u32,
    #[serde(default)]
    pub game_play_ms: u64,
    #[serde(default)]
    pub game_restarts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub timestamp_ms: i64,
}

impl Default for SceneBehaviorEventDto {
    fn default() -> Self {
        Self {
            event_id: String::new(),
            user_id: 0,
            session_id: String::new(),
            scene_id: String::new(),
            kind: SceneBehaviorEventKind::Impression,
            asset_kind: None,
            author_user_id: None,
            tags: Vec::new(),
            dwell_ms: 0,
            duration_ms: 0,
            completion_ratio: 0.0,
            loop_count: 0,
            game_inputs: 0,
            game_play_ms: 0,
            game_restarts: 0,
            game_score: None,
            reason: None,
            timestamp_ms: 0,
        }
    }
}

impl SceneBehaviorEventDto {
    /// Clamps finite `completion_ratio` to `0.0..=3.0` and drops non-finite `game_score`.
    pub fn sanitized(&self) -> Self {
        let mut copy = self.clone();
        copy.completion_ratio = if copy.completion_ratio.is_finite() {
            copy.completion_ratio.clamp(0.0, 3.0)
        } else {
            0.0
        };
        copy.game_score = copy.game_score.filter(|s| s.is_finite());
        copy
    }

    /// Returns the hidden tag string for `HideTag` events (from `reason` or first tag).
    pub fn hidden_tag(&self) -> Option<&str> {
        if self.kind != SceneBehaviorEventKind::HideTag {
            return None;
        }
        if let Some(reason) = self.reason.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return Some(reason);
        }
        self.tags.first().map(|s| s.trim()).filter(|s| !s.is_empty())
    }
}

fn deserialize_object_events<'de, D>(
    deserializer: D,
) -> Result<Vec<SceneBehaviorEventDto>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw_items = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut events = Vec::with_capacity(raw_items.len());
    for elem in raw_items {
        if !elem.is_object() {
            return Err(serde::de::Error::custom(
                "telemetry batch event must be a JSON object",
            ));
        }
        let ev: SceneBehaviorEventDto =
            serde_json::from_value(elem).map_err(serde::de::Error::custom)?;
        events.push(ev);
    }
    Ok(events)
}

/// Batch telemetry payload. Intentionally omits `#[serde(default)]` so `events` is required,
/// enforces a JSON object envelope, and rejects non-object elements inside `events`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct TelemetryBatchRequest {
    pub events: Vec<SceneBehaviorEventDto>,
}

impl<'de> Deserialize<'de> for TelemetryBatchRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let Some(map) = value.as_object_mut() else {
            return Err(serde::de::Error::custom(
                "telemetry batch request must be a JSON object",
            ));
        };
        let Some(events_val) = map.remove("events") else {
            return Err(serde::de::Error::missing_field("events"));
        };
        let events = deserialize_object_events(events_val).map_err(serde::de::Error::custom)?;
        Ok(Self { events })
    }
}

/// Accepts either a batch `{ "events": [...] }` or a single `SceneBehaviorEventDto` JSON object.
/// Non-object JSON values (such as arrays `[]`) are rejected, and any JSON object containing an
/// `"events"` key is strictly deserialized as `Batch` and never silently falls back to `Single`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TelemetrySubmitPayload {
    Batch(TelemetryBatchRequest),
    Single(Box<SceneBehaviorEventDto>),
}

impl<'de> Deserialize<'de> for TelemetrySubmitPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let Some(map) = value.as_object() else {
            return Err(serde::de::Error::custom(
                "telemetry payload must be a JSON object",
            ));
        };
        if map.contains_key("events") {
            let batch: TelemetryBatchRequest =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self::Batch(batch))
        } else {
            let single: SceneBehaviorEventDto =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(Self::Single(Box::new(single)))
        }
    }
}

impl TelemetrySubmitPayload {
    pub fn into_events(self) -> Vec<SceneBehaviorEventDto> {
        match self {
            Self::Batch(batch) => batch.events,
            Self::Single(ev) => vec![*ev],
        }
    }
}

/// Response returned by `POST /api/v1/code-scene/telemetry`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryIngestResponse {
    pub accepted: usize,
    pub reward: f64,
    pub profile_updated: bool,
}

/// Comment DTO for a Code Scene asset.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeSceneCommentDto {
    pub comment_id: String,
    pub scene_id: String,
    pub author_user_id: i64,
    pub author_username: String,
    pub content: String,
    pub parent_comment_id: Option<String>,
    pub likes_count: u64,
    pub viewer_liked: bool,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListCodeSceneCommentsQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<CodeSceneCommentSort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListCodeSceneCommentsResponse {
    pub scene_id: String,
    pub comments: Vec<CodeSceneCommentDto>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateCodeSceneCommentRequest {
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_comment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateCodeSceneCommentResponse {
    pub comment: CodeSceneCommentDto,
    pub comments_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToggleLikeResponse {
    pub liked: bool,
    pub likes_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToggleBookmarkResponse {
    pub bookmarked: bool,
    pub bookmarks_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToggleFollowResponse {
    pub following: bool,
    pub followers_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListCodeSceneAssetsQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AssetKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_bot_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListCodeSceneAssetsResponse {
    pub assets: Vec<CodeSceneAssetDto>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidateCodeSceneRequest {
    pub source_code: String,
    pub manifest: AssetManifest,
}

/// Structured compile/dry-run diagnostic entry surfaced in `ValidationReportDto`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidationDiagnosticDto {
    pub severity: ValidationSeverity,
    pub stage: Option<String>,
    pub code: String,
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Pre-publish compile & 5-frame dry-run validation report.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidationReportDto {
    #[serde(alias = "valid")]
    pub ok: bool,
    pub diagnostics: Vec<ValidationDiagnosticDto>,
    pub frames_executed: u32,
    pub draw_cmds_per_frame: Vec<usize>,
    pub has_pixel_output: bool,
    pub peak_ops_used: u64,
    pub cover_hash: Option<String>,
    pub feature_tokens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PublishCodeSceneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_id: Option<String>,
    pub title: String,
    pub description: String,
    pub kind: AssetKind,
    pub tags: Vec<String>,
    pub source_code: String,
    pub code_format: String,
    pub manifest: AssetManifest,
    pub controls_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_bot_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_bot_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remix_parent_id: Option<String>,
    #[serde(default, alias = "chat_id", skip_serializing_if = "Option::is_none")]
    pub share_to_chat_id: Option<i64>,
}

impl Default for PublishCodeSceneRequest {
    fn default() -> Self {
        Self {
            scene_id: None,
            title: String::new(),
            description: String::new(),
            kind: AssetKind::Animation,
            tags: Vec::new(),
            source_code: String::new(),
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest: AssetManifest::default(),
            controls_hint: String::new(),
            creator_bot_id: None,
            creator_bot_username: None,
            remix_parent_id: None,
            share_to_chat_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PublishCodeSceneResponse {
    pub asset: CodeSceneAssetDto,
    pub validation: ValidationReportDto,
    pub posted_chat_message_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiGenerateSceneRequest {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AssetKind>,
    #[serde(default, alias = "creator_bot_id", skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<i64>,
    #[serde(default, alias = "creator_bot_username", skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiGenerateSceneResponse {
    pub draft_id: String,
    pub title: String,
    pub description: String,
    pub kind: AssetKind,
    pub tags: Vec<String>,
    pub source_code: String,
    pub code_format: String,
    pub manifest: AssetManifest,
    pub controls_hint: String,
    pub validation: ValidationReportDto,
    pub engine_used: String,
    pub repair_attempts: u32,
    pub bot_id: Option<i64>,
    pub bot_username: Option<String>,
}

impl Default for AiGenerateSceneResponse {
    fn default() -> Self {
        Self {
            draft_id: String::new(),
            title: String::new(),
            description: String::new(),
            kind: AssetKind::Animation,
            tags: Vec::new(),
            source_code: String::new(),
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest: AssetManifest::default(),
            controls_hint: String::new(),
            validation: ValidationReportDto::default(),
            engine_used: String::new(),
            repair_attempts: 0,
            bot_id: None,
            bot_username: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiIterateSceneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<AssetManifest>,
    #[serde(alias = "prompt")]
    pub instruction: String,
    #[serde(default, alias = "creator_bot_id", skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<i64>,
    #[serde(default, alias = "creator_bot_username", skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiPublishSceneRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<AssetManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls_hint: Option<String>,
    #[serde(default, alias = "creator_bot_id", skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<i64>,
    #[serde(default, alias = "creator_bot_username", skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remix_parent_id: Option<String>,
    #[serde(default, alias = "share_to_chat_id", skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemixSceneRequest {
    #[serde(alias = "scene_id")]
    pub parent_scene_id: String,
    pub prompt: String,
    #[serde(default, alias = "creator_bot_id", skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<i64>,
    #[serde(default, alias = "creator_bot_username", skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
    #[serde(default)]
    pub auto_publish: bool,
    #[serde(default, alias = "share_to_chat_id", skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemixSceneResponse {
    pub draft: AiGenerateSceneResponse,
    pub published: Option<PublishCodeSceneResponse>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_kind_and_feed_tab_serde_and_helpers() {
        for kind in AssetKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: AssetKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
            assert_eq!(AssetKind::parse(kind.as_str()), Some(kind));
            assert_eq!(AssetKind::parse(kind.label_zh()), Some(kind));
            assert_eq!(AssetKind::parse(kind.label_en()), Some(kind));
        }
        assert_eq!(AssetKind::parse("小游戏"), Some(AssetKind::Game));
        assert_eq!(AssetKind::parse("unknown"), None);

        for tab in CodeSceneFeedTab::ALL {
            let json = serde_json::to_string(&tab).unwrap();
            assert_eq!(json, format!("\"{}\"", tab.as_str()));
            let back: CodeSceneFeedTab = serde_json::from_str(&json).unwrap();
            assert_eq!(back, tab);
            assert_eq!(CodeSceneFeedTab::parse(tab.as_str()), Some(tab));
            assert_eq!(CodeSceneFeedTab::parse(tab.label_zh()), Some(tab));
        }
        assert_eq!(CodeSceneFeedTab::Videos.filter_kind(), Some(AssetKind::Video));
        assert_eq!(CodeSceneFeedTab::Animations.filter_kind(), Some(AssetKind::Animation));
        assert_eq!(CodeSceneFeedTab::Games.filter_kind(), Some(AssetKind::Game));
        assert_eq!(CodeSceneFeedTab::ForYou.filter_kind(), None);
    }

    #[test]
    fn test_scene_behavior_event_kind_all_20_variants_roundtrip() {
        assert_eq!(SceneBehaviorEventKind::ALL.len(), 20);
        for kind in SceneBehaviorEventKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: SceneBehaviorEventKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
            assert_eq!(SceneBehaviorEventKind::parse(kind.as_str()), Some(kind));
        }
        assert!(SceneBehaviorEventKind::DoubleTapLike.is_explicit_positive());
        assert!(SceneBehaviorEventKind::Remix.is_explicit_positive());
        assert!(!SceneBehaviorEventKind::Dwell.is_explicit_positive());
        assert!(SceneBehaviorEventKind::FastSkip.is_negative());
        assert!(!SceneBehaviorEventKind::FastSkip.is_hard_negative());
        assert!(SceneBehaviorEventKind::NotInterested.is_hard_negative());
        assert!(SceneBehaviorEventKind::HideAuthor.is_hard_negative());
        assert!(SceneBehaviorEventKind::HideTag.is_hard_negative());
    }

    #[test]
    fn test_asset_manifest_defaults_clamping_and_validation() {
        let def: AssetManifest = serde_json::from_str("{}").unwrap();
        assert_eq!(def.width, 360);
        assert_eq!(def.height, 640);
        assert_eq!(def.fps, 60);
        assert_eq!(def.kind, AssetKind::Animation);
        assert!(def.validate().is_ok());

        let extreme_vid = AssetManifest {
            kind: AssetKind::Video,
            width: 0,
            height: 4096,
            fps: 240,
            duration_sec: Some(f64::NAN),
            seed: 42,
        };
        assert!(extreme_vid.validate().is_err());
        let clamped = extreme_vid.clamped();
        assert_eq!(clamped.width, 160);
        assert_eq!(clamped.height, 1280);
        assert_eq!(clamped.fps, 60);
        assert_eq!(clamped.duration_sec, Some(15.0));
        assert!(clamped.validate().is_ok());

        let short_vid = AssetManifest {
            kind: AssetKind::Video,
            duration_sec: Some(1.0),
            ..AssetManifest::default()
        };
        assert!(short_vid.validate().is_err());
        assert_eq!(short_vid.clamped().duration_sec, Some(5.0));

        let long_vid = AssetManifest {
            kind: AssetKind::Video,
            duration_sec: Some(500.0),
            ..AssetManifest::default()
        };
        assert_eq!(long_vid.clamped().duration_sec, Some(120.0));

        let bad_anim = AssetManifest {
            kind: AssetKind::Animation,
            width: 9999,
            fps: 0,
            duration_sec: Some(-2.0),
            ..AssetManifest::default()
        };
        assert!(bad_anim.validate().is_err());
        let fixed_anim = bad_anim.clamped();
        assert_eq!(fixed_anim.width, 720);
        assert_eq!(fixed_anim.fps, 15);
        assert_eq!(fixed_anim.duration_sec, None);
        assert!(fixed_anim.validate().is_ok());
    }

    #[test]
    fn test_code_scene_asset_score_and_feed_roundtrip() {
        let minimal: CodeSceneAssetDto = serde_json::from_str("{}").unwrap();
        assert_eq!(minimal.code_format, "cs1");

        let asset = CodeSceneAssetDto {
            scene_id: "scene_1".into(),
            title: "Neon Test".into(),
            description: "Desc".into(),
            kind: AssetKind::Game,
            tags: vec!["#Neon".into(), "  ARCADE ".into(), "neon".into(), "".into()],
            author_user_id: 777_000,
            author_username: "fabushi_ai_bot".into(),
            creator_bot_id: Some(777_000),
            creator_bot_username: Some("fabushi_ai_bot".into()),
            remix_parent_id: None,
            source_code: "fn on_render(s,t,c){clear(0);}".into(),
            code_format: CODE_SCENE_FORMAT_CS1.into(),
            manifest: AssetManifest::for_kind(AssetKind::Game),
            controls_hint: "Arrow keys".into(),
            plays_count: 10,
            likes_count: 3,
            bookmarks_count: 1,
            comments_count: 2,
            shares_count: 1,
            remixes_count: 0,
            viewer_liked: true,
            viewer_bookmarked: false,
            viewer_following_author: true,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_001_000,
        };
        assert_eq!(asset.normalized_tags(), vec!["neon", "arcade"]);
        let hash = asset.content_sha256();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

        let bd = ScoreBreakdown {
            embedding_sim: f64::NAN,
            swing_cf: f64::INFINITY,
            quality_ecpm: 0.85,
            final_score: 1.25,
            ..ScoreBreakdown::default()
        }
        .sanitized();
        assert_eq!(bd.embedding_sim, 0.0);
        assert_eq!(bd.swing_cf, 0.0);
        assert_eq!(bd.quality_ecpm, 0.85);

        let item = RankedSceneItemDto {
            asset: asset.clone(),
            explanation: RecommendationExplanation {
                recall_channels: vec!["embedding".into(), "hot".into()],
                primary_reason_zh: "根据你的互动推荐".into(),
                primary_reason_en: "Recommended for you".into(),
                traffic_tier: "tier2_rising".into(),
                breakdown: bd,
            },
        };
        let resp = CodeSceneFeedResponse {
            items: vec![item],
            next_cursor: None,
            total_catalog: 18,
        };
        let val = serde_json::to_value(&resp).unwrap();
        assert!(val.get("next_cursor").is_some());
        let back: CodeSceneFeedResponse = serde_json::from_value(val).unwrap();
        assert_eq!(back, resp);

        assert_eq!(CodeSceneFeedRequest { limit: 0, ..Default::default() }.clamped_limit(), 12);
        assert_eq!(CodeSceneFeedRequest { limit: 999, ..Default::default() }.clamped_limit(), 50);
    }

    #[test]
    fn test_telemetry_single_and_batch_payload_deserialization() {
        let single_json = r#"{"event_id":"e1","user_id":101,"session_id":"s1","scene_id":"sc1","kind":"dwell","dwell_ms":4200,"completion_ratio":5.5,"game_score":null}"#;
        let parsed_single: TelemetrySubmitPayload = serde_json::from_str(single_json).unwrap();
        let events = parsed_single.into_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, SceneBehaviorEventKind::Dwell);
        let sanitized = events[0].sanitized();
        assert_eq!(sanitized.completion_ratio, 3.0);

        let batch_json = r#"{"events":[{"event_id":"e2","scene_id":"sc1","kind":"hide_tag","reason":"retro"}]}"#;
        let parsed_batch: TelemetrySubmitPayload = serde_json::from_str(batch_json).unwrap();
        let batch_events = parsed_batch.into_events();
        assert_eq!(batch_events.len(), 1);
        assert_eq!(batch_events[0].hidden_tag(), Some("retro"));

        let empty_batch: TelemetrySubmitPayload = serde_json::from_str(r#"{"events":[]}"#).unwrap();
        assert!(matches!(empty_batch, TelemetrySubmitPayload::Batch(_)));
        assert!(empty_batch.into_events().is_empty());

        let multi_batch: TelemetrySubmitPayload = serde_json::from_str(
            r#"{"events":[{"event_id":"e3","scene_id":"sc1","kind":"like"},{"event_id":"e4","scene_id":"sc2","kind":"complete","completion_ratio":1.0}]}"#,
        )
        .unwrap();
        assert!(matches!(multi_batch, TelemetrySubmitPayload::Batch(_)));
        assert_eq!(multi_batch.into_events().len(), 2);

        // Malformed batch & non-object JSON payloads MUST fail closed and NEVER fall back to Single(Impression)
        for bad_payload in [
            "[]",
            r#"[[]]"#,
            r#"["a"]"#,
            r#""hello""#,
            "null",
            "123",
            "true",
            r#"{"events":null}"#,
            r#"{"events":123}"#,
            r#"{"events":[[]]}"#,
            r#"{"events":[["a"]]}"#,
            r#"{"events":[["a", 1, "s"]]}"#,
            r#"{"events":[null]}"#,
            r#"{"events":[123]}"#,
            r#"{"events":["like"]}"#,
            r#"{"events":[{"kind":"BAD"}]}"#,
            r#"{"events":[{"scene_id":"s1","kind":"TOTALLY_INVALID_KIND"}]}"#,
            r#"{"events":"not_an_array"}"#,
            r#"{"events":[{"scene_id":"s1","kind":"dwell","dwell_ms":-5}]}"#,
        ] {
            let res: Result<TelemetrySubmitPayload, _> = serde_json::from_str(bad_payload);
            assert!(
                res.is_err(),
                "malformed telemetry payload `{bad_payload}` must return Err, got {res:?}"
            );
            let batch_res: Result<TelemetryBatchRequest, _> = serde_json::from_str(bad_payload);
            assert!(
                batch_res.is_err(),
                "malformed TelemetryBatchRequest `{bad_payload}` must return Err, got {batch_res:?}"
            );
        }

        // Direct TelemetryBatchRequest rejects missing events field (`{}`) while accepting valid batches
        assert!(serde_json::from_str::<TelemetryBatchRequest>("{}").is_err());
        let direct_empty: TelemetryBatchRequest = serde_json::from_str(r#"{"events":[]}"#).unwrap();
        assert!(direct_empty.events.is_empty());
        let direct_multi: TelemetryBatchRequest = serde_json::from_str(
            r#"{"events":[{"event_id":"e5","scene_id":"sc1","kind":"like"},{"event_id":"e6","scene_id":"sc2","kind":"replay"}]}"#,
        )
        .unwrap();
        assert_eq!(direct_multi.events.len(), 2);

        // Verify Serialize preserves exact untagged wire shape for both variants
        let single_val = TelemetrySubmitPayload::Single(Box::new(SceneBehaviorEventDto {
            scene_id: "sc9".into(),
            kind: SceneBehaviorEventKind::Like,
            ..Default::default()
        }));
        let single_rt: TelemetrySubmitPayload =
            serde_json::from_str(&serde_json::to_string(&single_val).unwrap()).unwrap();
        assert_eq!(single_rt, single_val);

        let batch_val = TelemetrySubmitPayload::Batch(TelemetryBatchRequest {
            events: vec![SceneBehaviorEventDto {
                scene_id: "sc9".into(),
                kind: SceneBehaviorEventKind::Bookmark,
                ..Default::default()
            }],
        });
        let batch_rt: TelemetrySubmitPayload =
            serde_json::from_str(&serde_json::to_string(&batch_val).unwrap()).unwrap();
        assert_eq!(batch_rt, batch_val);

        let nan_ev = SceneBehaviorEventDto {
            completion_ratio: f64::NAN,
            game_score: Some(f64::INFINITY),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(nan_ev.completion_ratio, 0.0);
        assert_eq!(nan_ev.game_score, None);

        let ingest = TelemetryIngestResponse { accepted: 2, reward: 1.75, profile_updated: true };
        let back: TelemetryIngestResponse = serde_json::from_str(&serde_json::to_string(&ingest).unwrap()).unwrap();
        assert_eq!(back, ingest);
    }

    #[test]
    fn test_social_comments_validate_publish_and_ai_dtos_roundtrip() {
        let like: ToggleLikeResponse = serde_json::from_str(r#"{"liked":true,"likes_count":7}"#).unwrap();
        assert!(like.liked && like.likes_count == 7);
        let bm: ToggleBookmarkResponse = serde_json::from_str(r#"{"bookmarked":true,"bookmarks_count":2}"#).unwrap();
        assert!(bm.bookmarked && bm.bookmarks_count == 2);
        let fl: ToggleFollowResponse = serde_json::from_str(r#"{"following":true,"followers_count":99}"#).unwrap();
        assert!(fl.following && fl.followers_count == 99);

        let comment_resp = ListCodeSceneCommentsResponse {
            scene_id: "sc1".into(),
            comments: vec![CodeSceneCommentDto {
                comment_id: "c1".into(),
                scene_id: "sc1".into(),
                author_user_id: 100_001,
                author_username: "alice".into(),
                content: "太酷了".into(),
                parent_comment_id: None,
                likes_count: 3,
                viewer_liked: false,
                created_at_ms: 123,
            }],
            total: 1,
        };
        let back: ListCodeSceneCommentsResponse =
            serde_json::from_str(&serde_json::to_string(&comment_resp).unwrap()).unwrap();
        assert_eq!(back, comment_resp);

        let report = ValidationReportDto {
            ok: false,
            diagnostics: vec![ValidationDiagnosticDto {
                severity: ValidationSeverity::Error,
                stage: Some("parse".into()),
                code: "E_PARSE".into(),
                message: "unexpected token".into(),
                line: Some(3),
                column: Some(9),
            }],
            frames_executed: 0,
            draw_cmds_per_frame: vec![],
            has_pixel_output: false,
            peak_ops_used: 0,
            cover_hash: None,
            feature_tokens: vec![],
        };
        let report_back: ValidationReportDto =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(report_back, report);

        let pub_req: PublishCodeSceneRequest =
            serde_json::from_str(r#"{"title":"T","chat_id":500}"#).unwrap();
        assert_eq!(pub_req.share_to_chat_id, Some(500));
        assert_eq!(pub_req.code_format, "cs1");

        let iter_req: AiIterateSceneRequest =
            serde_json::from_str(r#"{"prompt":"faster","creator_bot_id":777000}"#).unwrap();
        assert_eq!(iter_req.instruction, "faster");
        assert_eq!(iter_req.bot_id, Some(777_000));

        let remix_req: RemixSceneRequest =
            serde_json::from_str(r#"{"scene_id":"seed_game_flappy_bird","prompt":"cyber","share_to_chat_id":42}"#).unwrap();
        assert_eq!(remix_req.parent_scene_id, "seed_game_flappy_bird");
        assert_eq!(remix_req.chat_id, Some(42));
    }

    #[test]
    fn test_code_scene_card_action_protocol() {
        for act in [
            CodeSceneCardAction::Open("seed_game_flappy_bird".into()),
            CodeSceneCardAction::Remix("seed_anim_aurora_plasma".into()),
            CodeSceneCardAction::Share("seed_video_cyber_dawn".into()),
        ] {
            let payload = act.to_payload();
            assert_eq!(CodeSceneCardAction::parse_payload(&payload), Some(act.clone()));
            let json = serde_json::to_string(&act).unwrap();
            let back: CodeSceneCardAction = serde_json::from_str(&json).unwrap();
            assert_eq!(back, act);
        }
        assert_eq!(CodeSceneCardAction::parse_payload("code_scene:open:"), None);
        assert_eq!(CodeSceneCardAction::parse_payload("code_scene:open:bad id"), None);
        assert_eq!(CodeSceneCardAction::parse_payload("other:open:abc"), None);
    }
}
