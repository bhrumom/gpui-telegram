//! Shared contracts, payloads, REST DTOs, and review validation for the
//! Apple-style Sidebar & Creative Content Ecosystem (Books, Music, Games, App Store).

use gpui_telegram_core::core::deep_links::{
    format_apple_deep_link, AppleDeepLink, AppleDeepLinkSection,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

pub const CREATOR_STUDIO_BOT_USER_ID: i64 = 777_020;
pub const CREATOR_STUDIO_BOT_USERNAME: &str = "CreatorStudioBot";
pub const MAX_CREATION_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MIN_MUSIC_BPM: u16 = 30;
pub const MAX_MUSIC_BPM: u16 = 300;
pub const MIN_BOOK_READABLE_CHARS: usize = 50;
pub const MAX_CREATION_TITLE_CHARS: usize = 120;
pub const MIN_CREATION_SUMMARY_CHARS: usize = 10;
pub const MAX_CREATION_SUMMARY_CHARS: usize = 2_000;
pub const MAX_CREATION_AUTHOR_CHARS: usize = 120;
pub const MAX_CREATION_GENRES: usize = 5;
pub const MAX_CREATION_TAGS: usize = 12;

fn default_language() -> String {
    "zh-CN".to_string()
}

fn default_age_rating() -> String {
    "4+".to_string()
}

fn default_version() -> u32 {
    1
}

fn default_instrument_gain() -> f32 {
    0.8
}

fn default_note_velocity() -> u8 {
    96
}

fn default_chord_velocity() -> u8 {
    68
}

fn default_time_signature() -> String {
    "4/4".to_string()
}

fn default_question_points() -> u32 {
    100
}

fn default_true() -> bool {
    true
}

fn default_price_label() -> String {
    "Free".to_string()
}

/// Content vertical kind for user creations and public catalog entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreationKind {
    Book,
    Music,
    Game,
    App,
}

impl CreationKind {
    pub const ALL: [Self; 4] = [Self::Book, Self::Music, Self::Game, Self::App];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Book => "book",
            Self::Music => "music",
            Self::Game => "game",
            Self::App => "app",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("book") || s.eq_ignore_ascii_case("books") {
            Some(Self::Book)
        } else if s.eq_ignore_ascii_case("music") {
            Some(Self::Music)
        } else if s.eq_ignore_ascii_case("game") || s.eq_ignore_ascii_case("games") {
            Some(Self::Game)
        } else if s.eq_ignore_ascii_case("app") || s.eq_ignore_ascii_case("appstore") {
            Some(Self::App)
        } else {
            None
        }
    }

    pub fn apple_section_slug(self) -> &'static str {
        self.apple_section().as_slug()
    }

    pub fn apple_section(self) -> AppleDeepLinkSection {
        match self {
            Self::Book => AppleDeepLinkSection::Books,
            Self::Music => AppleDeepLinkSection::Music,
            Self::Game => AppleDeepLinkSection::Games,
            Self::App => AppleDeepLinkSection::AppStore,
        }
    }

    pub fn deep_link_uri(self, creation_id: &str) -> String {
        format_apple_deep_link(&AppleDeepLink::for_creation(self.apple_section(), creation_id))
    }
}

impl fmt::Display for CreationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Review and publishing lifecycle state for a user cloud creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CreationReviewStatus {
    #[default]
    Draft,
    PendingReview,
    Published,
    Rejected,
    Unpublished,
}

impl CreationReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::PendingReview => "pending_review",
            Self::Published => "published",
            Self::Rejected => "rejected",
            Self::Unpublished => "unpublished",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "draft" => Some(Self::Draft),
            "pending_review" => Some(Self::PendingReview),
            "published" => Some(Self::Published),
            "rejected" => Some(Self::Rejected),
            "unpublished" => Some(Self::Unpublished),
            _ => None,
        }
    }
}

impl fmt::Display for CreationReviewStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Visibility setting for a user cloud creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CreationVisibility {
    #[default]
    Private,
    Unlisted,
    Public,
}

impl CreationVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Unlisted => "unlisted",
            Self::Public => "public",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "private" => Some(Self::Private),
            "unlisted" => Some(Self::Unlisted),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

impl fmt::Display for CreationVisibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Sort order for public catalog queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSortMode {
    #[default]
    Featured,
    Popular,
    TopRated,
    Newest,
}

impl CatalogSortMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Featured => "featured",
            Self::Popular => "popular",
            Self::TopRated => "top_rated",
            Self::Newest => "newest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "featured" => Some(Self::Featured),
            "popular" => Some(Self::Popular),
            "top_rated" => Some(Self::TopRated),
            "newest" => Some(Self::Newest),
            _ => None,
        }
    }
}

impl fmt::Display for CatalogSortMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Moderation decision when reviewing a `PendingReview` creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    Reject,
}

impl ReviewDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "approve" => Some(Self::Approve),
            "reject" => Some(Self::Reject),
            _ => None,
        }
    }
}

impl fmt::Display for ReviewDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Procedural gradient / artwork metadata for a creation card or cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverArtwork {
    pub gradient_start_hex: String,
    pub gradient_end_hex: String,
    pub accent_hex: String,
    #[serde(default)]
    pub icon_emoji: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_data_url: Option<String>,
}

impl Default for CoverArtwork {
    fn default() -> Self {
        Self {
            gradient_start_hex: "#1D4ED8".to_string(),
            gradient_end_hex: "#0F172A".to_string(),
            accent_hex: "#60A5FA".to_string(),
            icon_emoji: "✨".to_string(),
            media_file_id: None,
            thumbnail_data_url: None,
        }
    }
}

/// Tagged payload carrying vertical-specific structured content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreationPayload {
    Book(BookPayload),
    Music(MusicPayload),
    Game(GamePayload),
    App(AppStoreListingPayload),
}

impl CreationPayload {
    pub fn kind(&self) -> CreationKind {
        match self {
            Self::Book(_) => CreationKind::Book,
            Self::Music(_) => CreationKind::Music,
            Self::Game(_) => CreationKind::Game,
            Self::App(_) => CreationKind::App,
        }
    }
}

/// Authoritative cloud envelope for a user creation across Draft, Review, and Published states.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserCreationDto {
    pub creation_id: String,
    pub owner_user_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_bot_user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_chat_id: Option<i64>,
    pub kind: CreationKind,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub summary: String,
    pub author_display_name: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub cover: CoverArtwork,
    #[serde(default = "default_age_rating")]
    pub age_rating: String,
    pub payload: CreationPayload,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub cloud_revision: i64,
    #[serde(default)]
    pub review_status: CreationReviewStatus,
    #[serde(default)]
    pub visibility: CreationVisibility,
    #[serde(default)]
    pub has_live_published_version: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at_ms: Option<i64>,
    #[serde(default)]
    pub created_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at_ms: Option<i64>,
    #[serde(default)]
    pub rating_avg: f32,
    #[serde(default)]
    pub rating_count: u32,
    #[serde(default)]
    pub engagement_count: u64,
    #[serde(default)]
    pub save_or_install_count: u64,
}

// ============================================================================
// Apple Books Payload & User State Contracts
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BookRichBlock {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph {
        text: String,
    },
    BlockQuote {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attribution: Option<String>,
    },
    Callout {
        title: String,
        text: String,
    },
    Illustration {
        caption: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_file_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        palette_hex: Option<String>,
    },
    Divider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookPage {
    pub page_number: u32,
    #[serde(default)]
    pub blocks: Vec<BookRichBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookChapter {
    pub chapter_id: String,
    pub chapter_number: u32,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub pages: Vec<BookPage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookPayload {
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_index: Option<u32>,
    #[serde(default)]
    pub blurb: String,
    #[serde(default)]
    pub estimated_read_mins: u32,
    #[serde(default)]
    pub word_count: u32,
    #[serde(default)]
    pub chapters: Vec<BookChapter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookBookmark {
    pub bookmark_id: String,
    pub chapter_index: u32,
    pub page_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookHighlight {
    pub highlight_id: String,
    pub chapter_index: u32,
    pub page_index: u32,
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BookUserState {
    #[serde(default)]
    pub current_chapter_index: u32,
    #[serde(default)]
    pub current_page_index: u32,
    #[serde(default)]
    pub progress_percent: f32,
    #[serde(default)]
    pub bookmarks: Vec<BookBookmark>,
    #[serde(default)]
    pub highlights: Vec<BookHighlight>,
    #[serde(default)]
    pub in_library: bool,
    #[serde(default)]
    pub finished: bool,
    #[serde(default)]
    pub last_read_at_ms: i64,
}

// ============================================================================
// Apple Music Payload & User State Contracts
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SynthWaveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    Piano,
    Pad,
    Bass,
    Percussion,
}

impl SynthWaveform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sine => "sine",
            Self::Triangle => "triangle",
            Self::Saw => "saw",
            Self::Square => "square",
            Self::Piano => "piano",
            Self::Pad => "pad",
            Self::Bass => "bass",
            Self::Percussion => "percussion",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sine" => Some(Self::Sine),
            "triangle" => Some(Self::Triangle),
            "saw" => Some(Self::Saw),
            "square" => Some(Self::Square),
            "piano" => Some(Self::Piano),
            "pad" => Some(Self::Pad),
            "bass" => Some(Self::Bass),
            "percussion" => Some(Self::Percussion),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentSpec {
    pub channel: u8,
    pub name: String,
    pub waveform: SynthWaveform,
    #[serde(default = "default_instrument_gain")]
    pub gain: f32,
    #[serde(default)]
    pub pan: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteEvent {
    pub midi_note: u8,
    pub start_beat: f32,
    pub duration_beats: f32,
    #[serde(default = "default_note_velocity")]
    pub velocity: u8,
    #[serde(default)]
    pub channel: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChordEvent {
    pub symbol: String,
    pub start_beat: f32,
    pub duration_beats: f32,
    #[serde(default = "default_chord_velocity")]
    pub velocity: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedLyricLine {
    pub timestamp_ms: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicTrack {
    pub track_id: String,
    #[serde(default = "default_version")]
    pub track_number: u32,
    pub title: String,
    pub duration_secs: u32,
    pub bpm: u16,
    #[serde(default)]
    pub musical_key: String,
    #[serde(default)]
    pub instruments: Vec<InstrumentSpec>,
    #[serde(default)]
    pub melody_notes: Vec<NoteEvent>,
    #[serde(default)]
    pub chord_progression: Vec<ChordEvent>,
    #[serde(default)]
    pub lyrics: Vec<SyncedLyricLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_media_file_id: Option<String>,
    #[serde(default)]
    pub waveform_5bit: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicPayload {
    pub artist: String,
    pub album_title: String,
    #[serde(default)]
    pub release_year: u16,
    #[serde(default)]
    pub primary_genre: String,
    #[serde(default)]
    pub mood_tags: Vec<String>,
    pub bpm: u16,
    #[serde(default)]
    pub musical_key: String,
    #[serde(default = "default_time_signature")]
    pub time_signature: String,
    #[serde(default)]
    pub total_duration_secs: u32,
    #[serde(default)]
    pub tracks: Vec<MusicTrack>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MusicUserState {
    #[serde(default)]
    pub loved: bool,
    #[serde(default)]
    pub in_library: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_track_id: Option<String>,
    #[serde(default)]
    pub last_position_ms: u64,
    #[serde(default)]
    pub play_count: u64,
    #[serde(default)]
    pub last_played_at_ms: i64,
}

// ============================================================================
// Apple Games / Arcade Payload & User State Contracts
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GameCategory {
    #[default]
    Puzzle,
    Arcade,
    Trivia,
    TextAdventure,
    Strategy,
    Rhythm,
    CardMatch,
}

impl GameCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Puzzle => "puzzle",
            Self::Arcade => "arcade",
            Self::Trivia => "trivia",
            Self::TextAdventure => "text_adventure",
            Self::Strategy => "strategy",
            Self::Rhythm => "rhythm",
            Self::CardMatch => "card_match",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "puzzle" => Some(Self::Puzzle),
            "arcade" => Some(Self::Arcade),
            "trivia" => Some(Self::Trivia),
            "text_adventure" => Some(Self::TextAdventure),
            "strategy" => Some(Self::Strategy),
            "rhythm" => Some(Self::Rhythm),
            "card_match" => Some(Self::CardMatch),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameEngineKind {
    GridPuzzle,
    TriviaQuiz,
    InteractiveFiction,
    #[serde(alias = "arcade_runner")]
    ArcadeDodge,
    SnakeGrid,
    CardMatch,
    MiniAppHtml,
}

impl GameEngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GridPuzzle => "grid_puzzle",
            Self::TriviaQuiz => "trivia_quiz",
            Self::InteractiveFiction => "interactive_fiction",
            Self::ArcadeDodge => "arcade_dodge",
            Self::SnakeGrid => "snake_grid",
            Self::CardMatch => "card_match",
            Self::MiniAppHtml => "mini_app_html",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "grid_puzzle" => Some(Self::GridPuzzle),
            "trivia_quiz" => Some(Self::TriviaQuiz),
            "interactive_fiction" => Some(Self::InteractiveFiction),
            "arcade_dodge" | "arcade_runner" => Some(Self::ArcadeDodge),
            "snake_grid" => Some(Self::SnakeGrid),
            "card_match" => Some(Self::CardMatch),
            "mini_app_html" => Some(Self::MiniAppHtml),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridPuzzleLevel {
    pub level_id: String,
    pub title: String,
    pub initial_grid: Vec<Vec<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_grid: Option<Vec<Vec<i32>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_moves: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriviaQuestion {
    pub question_id: String,
    pub prompt: String,
    pub choices: Vec<String>,
    pub correct_index: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    #[serde(default = "default_question_points")]
    pub points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoryChoice {
    pub label: String,
    pub target_node_id: String,
    #[serde(default)]
    pub score_delta: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoryNode {
    pub node_id: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub choices: Vec<StoryChoice>,
    #[serde(default)]
    pub is_ending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending_badge: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardPair {
    pub pair_id: String,
    pub label: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameSpec {
    GridPuzzle {
        rows: u8,
        cols: u8,
        levels: Vec<GridPuzzleLevel>,
    },
    TriviaQuiz {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_limit_secs: Option<u32>,
        questions: Vec<TriviaQuestion>,
    },
    InteractiveFiction {
        start_node_id: String,
        nodes: Vec<StoryNode>,
    },
    #[serde(alias = "arcade_runner")]
    ArcadeDodge {
        tick_ms: u32,
        arena_width: u32,
        arena_height: u32,
        player_speed: f32,
        obstacle_speed: f32,
        lives: u8,
        target_score: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        theme_accent_hex: Option<String>,
    },
    SnakeGrid {
        grid_width: u16,
        grid_height: u16,
        tick_ms: u32,
        #[serde(default)]
        wrap_walls: bool,
        target_score: u32,
    },
    CardMatch {
        pairs: Vec<CardPair>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_limit_secs: Option<u32>,
    },
    MiniAppHtml {
        #[serde(default)]
        bot_username: String,
        #[serde(default)]
        short_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        html_bundle_media_id: Option<String>,
    },
}

impl GameSpec {
    pub fn engine_kind(&self) -> GameEngineKind {
        match self {
            Self::GridPuzzle { .. } => GameEngineKind::GridPuzzle,
            Self::TriviaQuiz { .. } => GameEngineKind::TriviaQuiz,
            Self::InteractiveFiction { .. } => GameEngineKind::InteractiveFiction,
            Self::ArcadeDodge { .. } => GameEngineKind::ArcadeDodge,
            Self::SnakeGrid { .. } => GameEngineKind::SnakeGrid,
            Self::CardMatch { .. } => GameEngineKind::CardMatch,
            Self::MiniAppHtml { .. } => GameEngineKind::MiniAppHtml,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameAchievementDef {
    pub achievement_id: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub icon_emoji: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_score: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GamePayload {
    pub developer: String,
    #[serde(default)]
    pub category: GameCategory,
    #[serde(default)]
    pub controls_hint: String,
    pub rules_markdown: String,
    pub engine_kind: GameEngineKind,
    #[serde(default = "default_true")]
    pub supports_leaderboard: bool,
    #[serde(default)]
    pub estimated_session_mins: u32,
    pub spec: GameSpec,
    #[serde(default)]
    pub achievements: Vec<GameAchievementDef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GameUserState {
    #[serde(default)]
    pub high_score: i64,
    #[serde(default)]
    pub play_count: u64,
    #[serde(default)]
    pub unlocked_achievements: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_state_json: Option<serde_json::Value>,
    #[serde(default)]
    pub in_library: bool,
    #[serde(default)]
    pub last_played_at_ms: i64,
}

// ============================================================================
// Apple App Store Listing Payload Contracts
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotAsset {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_file_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LinkedMiniAppRef {
    #[serde(default)]
    pub bot_username: String,
    #[serde(default)]
    pub short_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_param: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppStoreListingPayload {
    pub developer_name: String,
    pub version_string: String,
    #[serde(default)]
    pub release_notes: String,
    #[serde(default)]
    pub category: String,
    pub long_description: String,
    #[serde(default)]
    pub screenshots: Vec<ScreenshotAsset>,
    #[serde(default)]
    pub size_label: String,
    #[serde(default = "default_price_label")]
    pub price_label: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_miniapp: Option<LinkedMiniAppRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_schema_json: Option<serde_json::Value>,
}

// ============================================================================
// Public Catalog, Review Audit, User States, Reviews, Leaderboards & Bot Recs
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicCatalogItemDto {
    pub creation_id: String,
    pub owner_user_id: i64,
    pub kind: CreationKind,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub summary: String,
    pub author_display_name: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub cover: CoverArtwork,
    #[serde(default = "default_age_rating")]
    pub age_rating: String,
    #[serde(default = "default_version")]
    pub published_version: u32,
    #[serde(default)]
    pub published_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default)]
    pub rating_avg: f32,
    #[serde(default)]
    pub rating_count: u32,
    #[serde(default)]
    pub engagement_count: u64,
    #[serde(default)]
    pub save_or_install_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub featured_rank: Option<u32>,
    #[serde(default)]
    pub deep_link: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<CreationPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationReviewAuditDto {
    pub audit_id: i64,
    pub creation_id: String,
    pub actor_user_id: i64,
    pub from_status: CreationReviewStatus,
    pub to_status: CreationReviewStatus,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub validation_errors: Vec<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserCreationStateDto {
    pub creation_id: String,
    pub user_id: i64,
    pub kind: CreationKind,
    #[serde(default)]
    pub in_library: bool,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub progress_percent: f32,
    #[serde(default)]
    pub state_json: serde_json::Value,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationReviewEntryDto {
    pub review_id: i64,
    pub creation_id: String,
    pub user_id: i64,
    #[serde(default)]
    pub author_display_name: String,
    pub rating: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub body: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameLeaderboardEntryDto {
    pub rank: u32,
    pub creation_id: String,
    pub user_id: i64,
    #[serde(default)]
    pub display_name: String,
    pub score: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    pub achieved_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotRecommendationItem {
    pub creation_id: String,
    pub kind: CreationKind,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub summary: String,
    pub author_display_name: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub cover: CoverArtwork,
    #[serde(default)]
    pub rating_avg: f32,
    #[serde(default)]
    pub rating_count: u32,
    #[serde(default)]
    pub score: f32,
    #[serde(default)]
    pub reason: String,
    pub deep_link: String,
}

// ============================================================================
// REST API Request / Response Contracts (/api/v1/creations/* & /api/v1/catalog/*)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateCreationRequest {
    pub kind: CreationKind,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    pub summary: String,
    pub author_display_name: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub cover: CoverArtwork,
    #[serde(default = "default_age_rating")]
    pub age_rating: String,
    #[serde(default)]
    pub visibility: CreationVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_bot_user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator_chat_id: Option<i64>,
    pub payload: CreationPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpdateCreationRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genres: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<CoverArtwork>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_rating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<CreationVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<CreationPayload>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreationListResponse {
    pub items: Vec<UserCreationDto>,
    #[serde(default)]
    pub latest_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreationSyncResponse {
    pub latest_revision: i64,
    pub items: Vec<UserCreationDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SubmitReviewRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<CreationVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitReviewResponse {
    pub creation: UserCreationDto,
    #[serde(default)]
    pub validation_errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDecisionRequest {
    pub decision: ReviewDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<CreationVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CreationAuditListResponse {
    pub items: Vec<CreationReviewAuditDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CatalogQueryResponse {
    pub items: Vec<PublicCatalogItemDto>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStoreMiniAppEntry {
    pub bot_user_id: i64,
    pub bot_username: String,
    pub short_name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub web_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub deep_link: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AppStoreCatalogResponse {
    #[serde(default)]
    pub featured: Vec<PublicCatalogItemDto>,
    #[serde(default)]
    pub apps: Vec<PublicCatalogItemDto>,
    #[serde(default)]
    pub mini_apps: Vec<AppStoreMiniAppEntry>,
    #[serde(default)]
    pub cross_vertical: Vec<PublicCatalogItemDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CatalogRecommendResponse {
    pub items: Vec<BotRecommendationItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UpsertCreationStateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_library: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_json: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UserCreationStatesResponse {
    pub items: Vec<UserCreationStateDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCreationReviewRequest {
    pub rating: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreationReviewsResponse {
    pub items: Vec<CreationReviewEntryDto>,
    pub rating_avg: f32,
    pub rating_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitGameScoreRequest {
    pub score: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GameLeaderboardResponse {
    pub items: Vec<GameLeaderboardEntryDto>,
}

// ============================================================================
// Pure Review Validator (`validate_creation_for_review`)
// ============================================================================

/// Validates a `UserCreationDto` before transitioning to `PendingReview` / publishing.
/// Collects all structural, safety, and vertical-specific violations into `Err(Vec<String>)`.
pub fn validate_creation_for_review(creation: &UserCreationDto) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    validate_common_fields(creation, &mut errors);

    if creation.kind != creation.payload.kind() {
        errors.push(format!(
            "creation kind '{}' does not match payload kind '{}'",
            creation.kind,
            creation.payload.kind()
        ));
    }

    match &creation.payload {
        CreationPayload::Book(book) => validate_book_payload(book, &mut errors),
        CreationPayload::Music(music) => validate_music_payload(music, &mut errors),
        CreationPayload::Game(game) => validate_game_payload(game, &mut errors),
        CreationPayload::App(app) => validate_app_payload(app, &mut errors),
    }

    match serde_json::to_vec(creation) {
        Ok(bytes) => {
            if bytes.len() > MAX_CREATION_PAYLOAD_BYTES {
                errors.push(format!(
                    "serialized creation size {} bytes exceeds maximum {} bytes",
                    bytes.len(),
                    MAX_CREATION_PAYLOAD_BYTES
                ));
            } else if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                let mut catch_all = Vec::new();
                check_safe_json_value(&mut catch_all, "creation", &val);
                if !catch_all.is_empty()
                    && !errors
                        .iter()
                        .any(|e| e.contains("disallowed script") || e.contains("script URI"))
                {
                    errors.extend(catch_all);
                }
            }
        }
        Err(e) => errors.push(format!("creation payload cannot be serialized as JSON: {e}")),
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn is_valid_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

fn contains_xss_pattern(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.contains("<script")
        || lower.contains("javascript:")
        || lower.contains("vbscript:")
    {
        return true;
    }
    let stripped: String = lower
        .chars()
        .filter(|&c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{FEFF}'
                )
        })
        .collect();
    stripped.contains("<script")
        || stripped.contains("javascript:")
        || stripped.contains("vbscript:")
}

fn has_disallowed_url_chars(value: &str) -> bool {
    value.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
    })
}

fn check_safe_text(errors: &mut Vec<String>, field: &str, value: &str) {
    if contains_xss_pattern(value) {
        errors.push(format!("{field} contains disallowed script or unsafe URI scheme"));
    }
}

fn check_safe_optional_text(errors: &mut Vec<String>, field: &str, value: Option<&str>) {
    if let Some(v) = value {
        check_safe_text(errors, field, v);
    }
}

fn check_safe_web_url(errors: &mut Vec<String>, field: &str, value: &str) {
    if has_disallowed_url_chars(value) || contains_xss_pattern(value) {
        errors.push(format!("{field} contains disallowed characters or script URI"));
        return;
    }
    let trimmed = value.trim_matches(' ');
    if trimmed.is_empty() {
        return;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        errors.push(format!("{field} must use https:// or http://"));
    }
}

fn check_safe_thumbnail_url(errors: &mut Vec<String>, field: &str, value: &str) {
    if has_disallowed_url_chars(value) || contains_xss_pattern(value) {
        errors.push(format!("{field} contains disallowed characters or script URI"));
        return;
    }
    let trimmed = value.trim_matches(' ');
    if trimmed.is_empty() {
        return;
    }
    let lower = trimmed.to_ascii_lowercase();
    let allowed = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("data:image/png;")
        || lower.starts_with("data:image/jpeg;")
        || lower.starts_with("data:image/webp;");
    if !allowed {
        errors.push(format!(
            "{field} must be an https/http URL or png/jpeg/webp data URL"
        ));
    }
}

fn check_safe_json_value(errors: &mut Vec<String>, field: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::String(s) => check_safe_text(errors, field, s),
        serde_json::Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                check_safe_json_value(errors, &format!("{field}[{idx}]"), item);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                check_safe_text(errors, &format!("{field}.{k}"), k);
                check_safe_json_value(errors, &format!("{field}.{k}"), v);
            }
        }
        _ => {}
    }
}

fn validate_common_fields(creation: &UserCreationDto, errors: &mut Vec<String>) {
    check_safe_text(errors, "creation_id", &creation.creation_id);
    check_safe_optional_text(
        errors,
        "rejection_reason",
        creation.rejection_reason.as_deref(),
    );
    check_safe_optional_text(errors, "review_notes", creation.review_notes.as_deref());

    let title_trimmed = creation.title.trim();
    let title_chars = title_trimmed.chars().count();
    if title_chars == 0 || title_chars > MAX_CREATION_TITLE_CHARS {
        errors.push(format!(
            "title must be 1..={MAX_CREATION_TITLE_CHARS} characters"
        ));
    }
    check_safe_text(errors, "title", &creation.title);

    if creation.subtitle.trim().chars().count() > 200 {
        errors.push("subtitle must be at most 200 characters".to_string());
    }
    check_safe_text(errors, "subtitle", &creation.subtitle);

    let summary_trimmed = creation.summary.trim();
    let summary_chars = summary_trimmed.chars().count();
    if !(MIN_CREATION_SUMMARY_CHARS..=MAX_CREATION_SUMMARY_CHARS).contains(&summary_chars) {
        errors.push(format!(
            "summary must be {MIN_CREATION_SUMMARY_CHARS}..={MAX_CREATION_SUMMARY_CHARS} characters"
        ));
    }
    check_safe_text(errors, "summary", &creation.summary);

    let author_trimmed = creation.author_display_name.trim();
    let author_chars = author_trimmed.chars().count();
    if author_chars == 0 || author_chars > MAX_CREATION_AUTHOR_CHARS {
        errors.push(format!(
            "author_display_name must be 1..={MAX_CREATION_AUTHOR_CHARS} characters"
        ));
    }
    check_safe_text(errors, "author_display_name", &creation.author_display_name);
    check_safe_text(errors, "language", &creation.language);
    check_safe_text(errors, "age_rating", &creation.age_rating);

    if creation.genres.len() > MAX_CREATION_GENRES {
        errors.push(format!(
            "genres may contain at most {MAX_CREATION_GENRES} entries"
        ));
    }
    for (i, genre) in creation.genres.iter().enumerate() {
        if genre.trim().is_empty() {
            errors.push(format!("genres[{i}] must not be blank"));
        }
        check_safe_text(errors, &format!("genres[{i}]"), genre);
    }

    if creation.tags.len() > MAX_CREATION_TAGS {
        errors.push(format!(
            "tags may contain at most {MAX_CREATION_TAGS} entries"
        ));
    }
    for (i, tag) in creation.tags.iter().enumerate() {
        if tag.trim().is_empty() {
            errors.push(format!("tags[{i}] must not be blank"));
        }
        check_safe_text(errors, &format!("tags[{i}]"), tag);
    }

    if !is_valid_hex_color(&creation.cover.gradient_start_hex) {
        errors.push("cover.gradient_start_hex must be a #RRGGBB hex color".to_string());
    }
    if !is_valid_hex_color(&creation.cover.gradient_end_hex) {
        errors.push("cover.gradient_end_hex must be a #RRGGBB hex color".to_string());
    }
    if !is_valid_hex_color(&creation.cover.accent_hex) {
        errors.push("cover.accent_hex must be a #RRGGBB hex color".to_string());
    }
    check_safe_text(errors, "cover.icon_emoji", &creation.cover.icon_emoji);
    check_safe_optional_text(
        errors,
        "cover.media_file_id",
        creation.cover.media_file_id.as_deref(),
    );
    if let Some(thumb) = creation.cover.thumbnail_data_url.as_deref() {
        check_safe_thumbnail_url(errors, "cover.thumbnail_data_url", thumb);
    }

    if !creation.rating_avg.is_finite() || creation.rating_avg < 0.0 || creation.rating_avg > 5.0 {
        errors.push("rating_avg must be finite and within 0.0..=5.0".to_string());
    }
}

fn count_non_whitespace_chars(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

fn validate_book_payload(book: &BookPayload, errors: &mut Vec<String>) {
    check_safe_text(errors, "book.author", &book.author);
    check_safe_optional_text(errors, "book.series_name", book.series_name.as_deref());
    check_safe_text(errors, "book.blurb", &book.blurb);

    if book.chapters.is_empty() {
        errors.push("book must contain at least 1 chapter".to_string());
        return;
    }

    let mut readable_chars = 0usize;
    for (ci, chapter) in book.chapters.iter().enumerate() {
        check_safe_text(
            errors,
            &format!("book.chapters[{ci}].chapter_id"),
            &chapter.chapter_id,
        );
        if chapter.title.trim().is_empty() {
            errors.push(format!("book.chapters[{ci}].title must not be empty"));
        }
        check_safe_text(errors, &format!("book.chapters[{ci}].title"), &chapter.title);
        check_safe_optional_text(
            errors,
            &format!("book.chapters[{ci}].subtitle"),
            chapter.subtitle.as_deref(),
        );

        if chapter.pages.is_empty() {
            errors.push(format!(
                "book.chapters[{ci}] must contain at least 1 page"
            ));
            continue;
        }

        for (pi, page) in chapter.pages.iter().enumerate() {
            for (bi, block) in page.blocks.iter().enumerate() {
                let prefix = format!("book.chapters[{ci}].pages[{pi}].blocks[{bi}]");
                match block {
                    BookRichBlock::Heading { text, .. } => {
                        check_safe_text(errors, &format!("{prefix}.text"), text);
                        readable_chars += count_non_whitespace_chars(text);
                    }
                    BookRichBlock::Paragraph { text } => {
                        check_safe_text(errors, &format!("{prefix}.text"), text);
                        readable_chars += count_non_whitespace_chars(text);
                    }
                    BookRichBlock::BlockQuote { text, attribution } => {
                        check_safe_text(errors, &format!("{prefix}.text"), text);
                        check_safe_optional_text(
                            errors,
                            &format!("{prefix}.attribution"),
                            attribution.as_deref(),
                        );
                        readable_chars += count_non_whitespace_chars(text);
                        if let Some(attr) = attribution.as_deref() {
                            readable_chars += count_non_whitespace_chars(attr);
                        }
                    }
                    BookRichBlock::Callout { title, text } => {
                        check_safe_text(errors, &format!("{prefix}.title"), title);
                        check_safe_text(errors, &format!("{prefix}.text"), text);
                        readable_chars += count_non_whitespace_chars(title);
                        readable_chars += count_non_whitespace_chars(text);
                    }
                    BookRichBlock::Illustration {
                        caption,
                        media_file_id,
                        palette_hex,
                    } => {
                        check_safe_text(errors, &format!("{prefix}.caption"), caption);
                        check_safe_optional_text(
                            errors,
                            &format!("{prefix}.media_file_id"),
                            media_file_id.as_deref(),
                        );
                        if let Some(hex) = palette_hex.as_deref() {
                            if !hex.is_empty() && !is_valid_hex_color(hex) {
                                errors.push(format!(
                                    "{prefix}.palette_hex must be a #RRGGBB hex color"
                                ));
                            }
                        }
                        readable_chars += count_non_whitespace_chars(caption);
                    }
                    BookRichBlock::Divider => {}
                }
            }
        }
    }

    if readable_chars < MIN_BOOK_READABLE_CHARS {
        errors.push(format!(
            "book must contain at least {MIN_BOOK_READABLE_CHARS} readable non-whitespace characters (found {readable_chars})"
        ));
    }
}

fn validate_music_payload(music: &MusicPayload, errors: &mut Vec<String>) {
    check_safe_text(errors, "music.artist", &music.artist);
    check_safe_text(errors, "music.album_title", &music.album_title);
    check_safe_text(errors, "music.primary_genre", &music.primary_genre);
    check_safe_text(errors, "music.musical_key", &music.musical_key);
    check_safe_text(errors, "music.time_signature", &music.time_signature);
    for (i, mood) in music.mood_tags.iter().enumerate() {
        check_safe_text(errors, &format!("music.mood_tags[{i}]"), mood);
    }

    if !(MIN_MUSIC_BPM..=MAX_MUSIC_BPM).contains(&music.bpm) {
        errors.push(format!(
            "music.bpm must be within {MIN_MUSIC_BPM}..={MAX_MUSIC_BPM}"
        ));
    }
    if music.tracks.is_empty() {
        errors.push("music must contain at least 1 track".to_string());
        return;
    }

    for (ti, track) in music.tracks.iter().enumerate() {
        let p = format!("music.tracks[{ti}]");
        check_safe_text(errors, &format!("{p}.track_id"), &track.track_id);
        if track.title.trim().is_empty() {
            errors.push(format!("{p}.title must not be empty"));
        }
        check_safe_text(errors, &format!("{p}.title"), &track.title);
        check_safe_text(errors, &format!("{p}.musical_key"), &track.musical_key);
        check_safe_optional_text(
            errors,
            &format!("{p}.audio_media_file_id"),
            track.audio_media_file_id.as_deref(),
        );

        if !(MIN_MUSIC_BPM..=MAX_MUSIC_BPM).contains(&track.bpm) {
            errors.push(format!(
                "{p}.bpm must be within {MIN_MUSIC_BPM}..={MAX_MUSIC_BPM}"
            ));
        }
        if track.duration_secs < 3 {
            errors.push(format!("{p}.duration_secs must be at least 3"));
        }

        for (ii, inst) in track.instruments.iter().enumerate() {
            check_safe_text(errors, &format!("{p}.instruments[{ii}].name"), &inst.name);
            if !inst.gain.is_finite() || inst.gain < 0.0 {
                errors.push(format!("{p}.instruments[{ii}].gain must be finite and >= 0.0"));
            }
            if !inst.pan.is_finite() || inst.pan < -1.0 || inst.pan > 1.0 {
                errors.push(format!("{p}.instruments[{ii}].pan must be finite and in -1.0..=1.0"));
            }
        }

        for (ci, chord) in track.chord_progression.iter().enumerate() {
            check_safe_text(
                errors,
                &format!("{p}.chord_progression[{ci}].symbol"),
                &chord.symbol,
            );
            if !chord.start_beat.is_finite() || chord.start_beat < 0.0 {
                errors.push(format!(
                    "{p}.chord_progression[{ci}].start_beat must be finite and >= 0.0"
                ));
            }
            if !chord.duration_beats.is_finite() || chord.duration_beats <= 0.0 {
                errors.push(format!(
                    "{p}.chord_progression[{ci}].duration_beats must be finite and > 0.0"
                ));
            }
        }

        for (li, lyric) in track.lyrics.iter().enumerate() {
            check_safe_text(errors, &format!("{p}.lyrics[{li}].text"), &lyric.text);
        }

        for (ni, note) in track.melody_notes.iter().enumerate() {
            if note.midi_note > 127 {
                errors.push(format!("{p}.melody_notes[{ni}].midi_note must be <= 127"));
            }
            if !note.start_beat.is_finite() || note.start_beat < 0.0 {
                errors.push(format!(
                    "{p}.melody_notes[{ni}].start_beat must be finite and >= 0.0"
                ));
            }
            if !note.duration_beats.is_finite() || note.duration_beats <= 0.0 {
                errors.push(format!(
                    "{p}.melody_notes[{ni}].duration_beats must be finite and > 0.0"
                ));
            }
        }

        let has_audio_file = track
            .audio_media_file_id
            .as_deref()
            .map(|id| !id.trim().is_empty())
            .unwrap_or(false);
        if !has_audio_file && track.melody_notes.len() < 4 {
            errors.push(format!(
                "{p} must provide either an audio_media_file_id or at least 4 melody_notes"
            ));
        }
    }
}

fn validate_game_payload(game: &GamePayload, errors: &mut Vec<String>) {
    if game.developer.trim().is_empty() {
        errors.push("game.developer must not be empty".to_string());
    }
    check_safe_text(errors, "game.developer", &game.developer);
    check_safe_text(errors, "game.controls_hint", &game.controls_hint);

    if game.rules_markdown.trim().is_empty() {
        errors.push("game.rules_markdown must not be empty".to_string());
    }
    check_safe_text(errors, "game.rules_markdown", &game.rules_markdown);

    for (ai, ach) in game.achievements.iter().enumerate() {
        check_safe_text(
            errors,
            &format!("game.achievements[{ai}].achievement_id"),
            &ach.achievement_id,
        );
        check_safe_text(errors, &format!("game.achievements[{ai}].title"), &ach.title);
        check_safe_text(
            errors,
            &format!("game.achievements[{ai}].description"),
            &ach.description,
        );
        check_safe_text(
            errors,
            &format!("game.achievements[{ai}].icon_emoji"),
            &ach.icon_emoji,
        );
    }

    if game.engine_kind != game.spec.engine_kind() {
        errors.push(format!(
            "game.engine_kind '{}' does not match game.spec '{}'",
            game.engine_kind.as_str(),
            game.spec.engine_kind().as_str()
        ));
    }

    match &game.spec {
        GameSpec::GridPuzzle { rows, cols, levels } => {
            if *rows < 2 || *cols < 2 {
                errors.push("game.spec.grid_puzzle requires rows >= 2 and cols >= 2".to_string());
            }
            if levels.is_empty() {
                errors.push("game.spec.grid_puzzle requires at least 1 level".to_string());
            }
            let expected_rows = usize::from(*rows);
            let expected_cols = usize::from(*cols);
            for (li, lvl) in levels.iter().enumerate() {
                check_safe_text(
                    errors,
                    &format!("game.spec.levels[{li}].level_id"),
                    &lvl.level_id,
                );
                if lvl.title.trim().is_empty() {
                    errors.push(format!("game.spec.levels[{li}].title must not be empty"));
                }
                check_safe_text(errors, &format!("game.spec.levels[{li}].title"), &lvl.title);
                check_safe_optional_text(
                    errors,
                    &format!("game.spec.levels[{li}].hint"),
                    lvl.hint.as_deref(),
                );
                if lvl.initial_grid.is_empty() || lvl.initial_grid.iter().any(|r| r.is_empty()) {
                    errors.push(format!(
                        "game.spec.levels[{li}].initial_grid must be non-empty"
                    ));
                } else if lvl.initial_grid.len() != expected_rows
                    || lvl.initial_grid.iter().any(|r| r.len() != expected_cols)
                {
                    errors.push(format!(
                        "game.spec.levels[{li}].initial_grid must be a rectangular {rows}x{cols} grid"
                    ));
                }
                if let Some(tg) = lvl.target_grid.as_ref() {
                    if tg.is_empty() || tg.iter().any(|r| r.is_empty()) {
                        errors.push(format!(
                            "game.spec.levels[{li}].target_grid must be non-empty when present"
                        ));
                    } else if tg.len() != expected_rows
                        || tg.iter().any(|r| r.len() != expected_cols)
                    {
                        errors.push(format!(
                            "game.spec.levels[{li}].target_grid must be a rectangular {rows}x{cols} grid when present"
                        ));
                    }
                }
            }
        }
        GameSpec::TriviaQuiz { questions, .. } => {
            if questions.is_empty() {
                errors.push("game.spec.trivia_quiz requires at least 1 question".to_string());
            }
            for (qi, q) in questions.iter().enumerate() {
                check_safe_text(
                    errors,
                    &format!("game.spec.questions[{qi}].question_id"),
                    &q.question_id,
                );
                if q.prompt.trim().is_empty() {
                    errors.push(format!("game.spec.questions[{qi}].prompt must not be empty"));
                }
                check_safe_text(errors, &format!("game.spec.questions[{qi}].prompt"), &q.prompt);
                check_safe_optional_text(
                    errors,
                    &format!("game.spec.questions[{qi}].explanation"),
                    q.explanation.as_deref(),
                );
                if q.choices.len() < 2 {
                    errors.push(format!(
                        "game.spec.questions[{qi}].choices must have at least 2 entries"
                    ));
                }
                for (ci, choice) in q.choices.iter().enumerate() {
                    if choice.trim().is_empty() {
                        errors.push(format!(
                            "game.spec.questions[{qi}].choices[{ci}] must not be blank"
                        ));
                    }
                    check_safe_text(
                        errors,
                        &format!("game.spec.questions[{qi}].choices[{ci}]"),
                        choice,
                    );
                }
                if (q.correct_index as usize) >= q.choices.len() {
                    errors.push(format!(
                        "game.spec.questions[{qi}].correct_index {} out of bounds for {} choices",
                        q.correct_index,
                        q.choices.len()
                    ));
                }
            }
        }
        other => validate_game_spec_extended(other, errors),
    }
}

fn validate_game_spec_extended(spec: &GameSpec, errors: &mut Vec<String>) {
    match spec {
        GameSpec::InteractiveFiction {
            start_node_id,
            nodes,
        } => {
            check_safe_text(
                errors,
                "game.spec.interactive_fiction.start_node_id",
                start_node_id,
            );
            if nodes.is_empty() {
                errors.push("game.spec.interactive_fiction requires at least 1 node".to_string());
                return;
            }
            let mut node_map: HashMap<&str, &StoryNode> = HashMap::new();
            for (ni, node) in nodes.iter().enumerate() {
                check_safe_text(errors, &format!("game.spec.nodes[{ni}].node_id"), &node.node_id);
                let id = node.node_id.trim();
                if id.is_empty() {
                    errors.push(format!("game.spec.nodes[{ni}].node_id must not be empty"));
                } else if node_map.insert(id, node).is_some() {
                    errors.push(format!("game.spec.nodes duplicate node_id '{id}'"));
                }
                check_safe_text(errors, &format!("game.spec.nodes[{ni}].title"), &node.title);
                check_safe_text(errors, &format!("game.spec.nodes[{ni}].body"), &node.body);
                check_safe_optional_text(
                    errors,
                    &format!("game.spec.nodes[{ni}].ending_badge"),
                    node.ending_badge.as_deref(),
                );
                for (ci, choice) in node.choices.iter().enumerate() {
                    check_safe_text(
                        errors,
                        &format!("game.spec.nodes[{ni}].choices[{ci}].label"),
                        &choice.label,
                    );
                    check_safe_text(
                        errors,
                        &format!("game.spec.nodes[{ni}].choices[{ci}].target_node_id"),
                        &choice.target_node_id,
                    );
                }
            }

            let start_key = start_node_id.trim();
            if !node_map.contains_key(start_key) {
                errors.push(format!(
                    "game.spec.interactive_fiction start_node_id '{start_node_id}' does not exist"
                ));
            }
            for (ni, node) in nodes.iter().enumerate() {
                for (ci, choice) in node.choices.iter().enumerate() {
                    if !node_map.contains_key(choice.target_node_id.trim()) {
                        errors.push(format!(
                            "game.spec.nodes[{ni}].choices[{ci}].target_node_id '{}' does not exist",
                            choice.target_node_id
                        ));
                    }
                }
            }

            // BFS from start_node_id must reach at least one ending node
            if node_map.contains_key(start_key) {
                let mut visited: HashSet<&str> = HashSet::new();
                let mut queue: VecDeque<&str> = VecDeque::new();
                visited.insert(start_key);
                queue.push_back(start_key);
                let mut reached_ending = false;
                while let Some(curr_id) = queue.pop_front() {
                    if let Some(node) = node_map.get(curr_id) {
                        if node.is_ending {
                            reached_ending = true;
                            break;
                        }
                        for choice in &node.choices {
                            let target = choice.target_node_id.trim();
                            if node_map.contains_key(target) && visited.insert(target) {
                                queue.push_back(target);
                            }
                        }
                    }
                }
                if !reached_ending {
                    errors.push(
                        "game.spec.interactive_fiction has no ending reachable from start_node_id"
                            .to_string(),
                    );
                }
            }
        }
        GameSpec::ArcadeDodge {
            tick_ms,
            arena_width,
            arena_height,
            player_speed,
            obstacle_speed,
            lives,
            target_score,
            theme_accent_hex,
        } => {
            if *arena_width < 100 || *arena_height < 100 {
                errors.push(
                    "game.spec.arcade_dodge requires arena_width >= 100 and arena_height >= 100"
                        .to_string(),
                );
            }
            if *tick_ms < 10 {
                errors.push("game.spec.arcade_dodge requires tick_ms >= 10".to_string());
            }
            if *lives < 1 {
                errors.push("game.spec.arcade_dodge requires lives >= 1".to_string());
            }
            if *target_score < 1 {
                errors.push("game.spec.arcade_dodge requires target_score >= 1".to_string());
            }
            if !player_speed.is_finite() || *player_speed <= 0.0 {
                errors.push(
                    "game.spec.arcade_dodge requires finite player_speed > 0.0".to_string(),
                );
            }
            if !obstacle_speed.is_finite() || *obstacle_speed <= 0.0 {
                errors.push(
                    "game.spec.arcade_dodge requires finite obstacle_speed > 0.0".to_string(),
                );
            }
            if let Some(hex) = theme_accent_hex.as_deref() {
                if !hex.is_empty() && !is_valid_hex_color(hex) {
                    errors.push(
                        "game.spec.arcade_dodge.theme_accent_hex must be a #RRGGBB hex color"
                            .to_string(),
                    );
                }
            }
        }
        GameSpec::SnakeGrid {
            grid_width,
            grid_height,
            tick_ms,
            target_score,
            ..
        } => {
            if *grid_width < 5 || *grid_height < 5 {
                errors.push(
                    "game.spec.snake_grid requires grid_width >= 5 and grid_height >= 5".to_string(),
                );
            }
            if *tick_ms < 20 {
                errors.push("game.spec.snake_grid requires tick_ms >= 20".to_string());
            }
            if *target_score < 1 {
                errors.push("game.spec.snake_grid requires target_score >= 1".to_string());
            }
        }
        GameSpec::CardMatch { pairs, .. } => {
            if pairs.len() < 2 {
                errors.push("game.spec.card_match requires at least 2 pairs".to_string());
            }
            let mut seen_ids: HashSet<&str> = HashSet::new();
            for (pi, pair) in pairs.iter().enumerate() {
                check_safe_text(errors, &format!("game.spec.pairs[{pi}].pair_id"), &pair.pair_id);
                let id = pair.pair_id.trim();
                if id.is_empty() {
                    errors.push(format!("game.spec.pairs[{pi}].pair_id must not be empty"));
                } else if !seen_ids.insert(id) {
                    errors.push(format!("game.spec.pairs duplicate pair_id '{id}'"));
                }
                if pair.label.trim().is_empty() {
                    errors.push(format!("game.spec.pairs[{pi}].label must not be empty"));
                }
                check_safe_text(errors, &format!("game.spec.pairs[{pi}].label"), &pair.label);
                check_safe_text(
                    errors,
                    &format!("game.spec.pairs[{pi}].symbol"),
                    &pair.symbol,
                );
                if let Some(hex) = pair.accent_hex.as_deref() {
                    if !hex.is_empty() && !is_valid_hex_color(hex) {
                        errors.push(format!(
                            "game.spec.pairs[{pi}].accent_hex must be a #RRGGBB hex color"
                        ));
                    }
                }
            }
        }
        GameSpec::MiniAppHtml {
            bot_username,
            short_name,
            html_bundle_media_id,
        } => {
            check_safe_text(errors, "game.spec.mini_app_html.bot_username", bot_username);
            check_safe_text(errors, "game.spec.mini_app_html.short_name", short_name);
            check_safe_optional_text(
                errors,
                "game.spec.mini_app_html.html_bundle_media_id",
                html_bundle_media_id.as_deref(),
            );
            let has_miniapp = !bot_username.trim().is_empty() && !short_name.trim().is_empty();
            let has_bundle = html_bundle_media_id
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !has_miniapp && !has_bundle {
                errors.push(
                    "game.spec.mini_app_html requires (bot_username and short_name) or html_bundle_media_id"
                        .to_string(),
                );
            }
        }
        GameSpec::GridPuzzle { .. } | GameSpec::TriviaQuiz { .. } => {}
    }
}

fn validate_app_payload(app: &AppStoreListingPayload, errors: &mut Vec<String>) {
    if app.developer_name.trim().is_empty() {
        errors.push("app.developer_name must not be empty".to_string());
    }
    check_safe_text(errors, "app.developer_name", &app.developer_name);

    if app.version_string.trim().is_empty() {
        errors.push("app.version_string must not be empty".to_string());
    }
    check_safe_text(errors, "app.version_string", &app.version_string);
    check_safe_text(errors, "app.release_notes", &app.release_notes);
    check_safe_text(errors, "app.category", &app.category);

    if app.long_description.trim().is_empty() {
        errors.push("app.long_description must not be empty".to_string());
    }
    check_safe_text(errors, "app.long_description", &app.long_description);
    check_safe_text(errors, "app.size_label", &app.size_label);
    check_safe_text(errors, "app.price_label", &app.price_label);

    for (i, perm) in app.permissions.iter().enumerate() {
        check_safe_text(errors, &format!("app.permissions[{i}]"), perm);
    }
    for (i, shot) in app.screenshots.iter().enumerate() {
        check_safe_text(errors, &format!("app.screenshots[{i}].title"), &shot.title);
        check_safe_optional_text(
            errors,
            &format!("app.screenshots[{i}].caption"),
            shot.caption.as_deref(),
        );
        check_safe_optional_text(
            errors,
            &format!("app.screenshots[{i}].media_file_id"),
            shot.media_file_id.as_deref(),
        );
        if let Some(hex) = shot.accent_hex.as_deref() {
            if !hex.is_empty() && !is_valid_hex_color(hex) {
                errors.push(format!(
                    "app.screenshots[{i}].accent_hex must be a #RRGGBB hex color"
                ));
            }
        }
    }

    let mut has_valid_miniapp = false;
    if let Some(link) = app.linked_miniapp.as_ref() {
        check_safe_text(errors, "app.linked_miniapp.bot_username", &link.bot_username);
        check_safe_text(errors, "app.linked_miniapp.short_name", &link.short_name);
        check_safe_optional_text(
            errors,
            "app.linked_miniapp.launch_param",
            link.launch_param.as_deref(),
        );
        let url_before = errors.len();
        if let Some(url) = link.web_url.as_deref() {
            check_safe_web_url(errors, "app.linked_miniapp.web_url", url);
        }
        let has_bot_pair =
            !link.bot_username.trim().is_empty() && !link.short_name.trim().is_empty();
        let has_safe_url = link
            .web_url
            .as_deref()
            .map(|u| !u.trim().is_empty())
            .unwrap_or(false)
            && errors.len() == url_before;
        if has_bot_pair || has_safe_url {
            has_valid_miniapp = true;
        } else {
            errors.push(
                "app.linked_miniapp must specify (bot_username and short_name) or a valid web_url"
                    .to_string(),
            );
        }
    }

    if let Some(schema) = app.ui_schema_json.as_ref() {
        check_safe_json_value(errors, "app.ui_schema_json", schema);
    }

    let has_valid_ui_schema = app
        .ui_schema_json
        .as_ref()
        .is_some_and(serde_json::Value::is_object);

    if !has_valid_miniapp && !has_valid_ui_schema {
        errors.push(
            "app must provide either a valid linked_miniapp or a JSON object ui_schema_json"
                .to_string(),
        );
    }
}
