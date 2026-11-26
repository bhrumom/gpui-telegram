use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::{
    validate_creation_for_review, BookRichBlock, CoverArtwork, CreateCreationRequest,
    CreateCreationReviewRequest, CreationAuditListResponse, CreationKind, CreationListResponse,
    CreationPayload, CreationReviewAuditDto, CreationReviewEntryDto, CreationReviewStatus,
    CreationReviewsResponse, CreationSyncResponse, CreationVisibility, GameLeaderboardEntryDto,
    GameLeaderboardResponse, GameSpec, ReviewDecision, ReviewDecisionRequest,
    SubmitGameScoreRequest, SubmitReviewRequest, UpdateCreationRequest, UpsertCreationStateRequest,
    UserCreationDto, UserCreationStateDto, UserCreationStatesResponse,
};
use rusqlite::{params, OptionalExtension, Row, Transaction};
use std::collections::HashSet;

/// Migration V43: Authoritative user cloud creations, physically isolated public catalog,
/// standalone FTS5 index with atomic triggers, immutable review audit log, per-user states,
/// public ratings/reviews, game leaderboards, and CAS public asset allowlist.
pub const MIGRATION_V43_CREATIONS_AND_CATALOG: &str = r#"
CREATE TABLE IF NOT EXISTS user_creations (
    creation_id TEXT PRIMARY KEY,
    owner_user_id INTEGER NOT NULL REFERENCES users(user_id),
    creator_bot_user_id INTEGER,
    creator_chat_id INTEGER,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    subtitle TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL,
    author_display_name TEXT NOT NULL,
    language TEXT NOT NULL DEFAULT 'zh-CN',
    genres_json TEXT NOT NULL DEFAULT '[]',
    tags_json TEXT NOT NULL DEFAULT '[]',
    cover_json TEXT NOT NULL DEFAULT '{}',
    age_rating TEXT NOT NULL DEFAULT '4+',
    payload_json TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    cloud_revision INTEGER NOT NULL,
    review_status TEXT NOT NULL DEFAULT 'draft',
    visibility TEXT NOT NULL DEFAULT 'private',
    requested_visibility TEXT,
    has_live_published_version INTEGER NOT NULL DEFAULT 0,
    published_version INTEGER,
    published_visibility TEXT,
    published_snapshot_json TEXT,
    rejection_reason TEXT,
    review_notes TEXT,
    submitted_at_ms INTEGER,
    reviewed_at_ms INTEGER,
    published_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    deleted_at_ms INTEGER,
    rating_avg REAL NOT NULL DEFAULT 0.0,
    rating_count INTEGER NOT NULL DEFAULT 0,
    engagement_count INTEGER NOT NULL DEFAULT 0,
    save_or_install_count INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX IF NOT EXISTS idx_user_creations_owner_rev
    ON user_creations(owner_user_id, cloud_revision);
CREATE INDEX IF NOT EXISTS idx_user_creations_owner_kind_updated
    ON user_creations(owner_user_id, kind, updated_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_user_creations_review_queue
    ON user_creations(review_status, submitted_at_ms ASC);

CREATE TABLE IF NOT EXISTS public_catalog_entries (
    creation_id TEXT PRIMARY KEY,
    owner_user_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    subtitle TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL,
    author_display_name TEXT NOT NULL,
    language TEXT NOT NULL DEFAULT 'zh-CN',
    genres_json TEXT NOT NULL DEFAULT '[]',
    tags_json TEXT NOT NULL DEFAULT '[]',
    genres_text TEXT NOT NULL DEFAULT '',
    tags_text TEXT NOT NULL DEFAULT '',
    searchable_text TEXT NOT NULL DEFAULT '',
    cover_json TEXT NOT NULL DEFAULT '{}',
    age_rating TEXT NOT NULL DEFAULT '4+',
    snapshot_json TEXT NOT NULL,
    published_version INTEGER NOT NULL DEFAULT 1,
    featured_rank INTEGER,
    rating_avg REAL NOT NULL DEFAULT 0.0,
    rating_count INTEGER NOT NULL DEFAULT 0,
    engagement_count INTEGER NOT NULL DEFAULT 0,
    save_or_install_count INTEGER NOT NULL DEFAULT 0,
    popularity_score REAL NOT NULL DEFAULT 0.0,
    published_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_public_catalog_kind_pub
    ON public_catalog_entries(kind, published_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_public_catalog_featured
    ON public_catalog_entries(featured_rank ASC, popularity_score DESC, published_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_public_catalog_rating
    ON public_catalog_entries(rating_avg DESC, rating_count DESC, published_at_ms DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS public_catalog_fts USING fts5(
    creation_id UNINDEXED,
    kind UNINDEXED,
    title,
    subtitle,
    summary,
    author_display_name,
    genres_text,
    tags_text,
    searchable_text,
    tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS trg_public_catalog_ai AFTER INSERT ON public_catalog_entries BEGIN
    DELETE FROM public_catalog_fts WHERE creation_id = new.creation_id;
    INSERT INTO public_catalog_fts(
        creation_id, kind, title, subtitle, summary,
        author_display_name, genres_text, tags_text, searchable_text
    ) VALUES (
        new.creation_id, new.kind, new.title, new.subtitle, new.summary,
        new.author_display_name, new.genres_text, new.tags_text, new.searchable_text
    );
END;

CREATE TRIGGER IF NOT EXISTS trg_public_catalog_au
AFTER UPDATE OF creation_id, kind, title, subtitle, summary, author_display_name, genres_text, tags_text, searchable_text
ON public_catalog_entries BEGIN
    DELETE FROM public_catalog_fts WHERE creation_id = old.creation_id;
    INSERT INTO public_catalog_fts(
        creation_id, kind, title, subtitle, summary,
        author_display_name, genres_text, tags_text, searchable_text
    ) VALUES (
        new.creation_id, new.kind, new.title, new.subtitle, new.summary,
        new.author_display_name, new.genres_text, new.tags_text, new.searchable_text
    );
END;

CREATE TRIGGER IF NOT EXISTS trg_public_catalog_ad AFTER DELETE ON public_catalog_entries BEGIN
    DELETE FROM public_catalog_fts WHERE creation_id = old.creation_id;
END;

CREATE TABLE IF NOT EXISTS creation_review_audit (
    audit_id INTEGER PRIMARY KEY AUTOINCREMENT,
    creation_id TEXT NOT NULL,
    owner_user_id INTEGER NOT NULL,
    actor_user_id INTEGER NOT NULL,
    from_status TEXT NOT NULL,
    to_status TEXT NOT NULL,
    action TEXT NOT NULL,
    submitted_version INTEGER NOT NULL DEFAULT 1,
    reason TEXT,
    notes TEXT,
    validation_errors_json TEXT NOT NULL DEFAULT '[]',
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_creation_review_audit_creation
    ON creation_review_audit(creation_id, audit_id ASC);

CREATE TABLE IF NOT EXISTS user_creation_states (
    user_id INTEGER NOT NULL,
    creation_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    in_library INTEGER NOT NULL DEFAULT 0,
    favorite INTEGER NOT NULL DEFAULT 0,
    progress_percent REAL NOT NULL DEFAULT 0.0,
    state_json TEXT NOT NULL DEFAULT '{}',
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, creation_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_user_creation_states_user_updated
    ON user_creation_states(user_id, updated_at_ms DESC);

CREATE TABLE IF NOT EXISTS creation_reviews (
    review_id INTEGER PRIMARY KEY AUTOINCREMENT,
    creation_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    rating INTEGER NOT NULL,
    title TEXT,
    body TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(creation_id, user_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_creation_reviews_creation_updated
    ON creation_reviews(creation_id, updated_at_ms DESC);

CREATE TABLE IF NOT EXISTS creation_game_scores (
    creation_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    best_score INTEGER NOT NULL,
    metadata_json TEXT,
    achieved_at_ms INTEGER NOT NULL,
    PRIMARY KEY (creation_id, user_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_creation_game_scores_rank
    ON creation_game_scores(creation_id, best_score DESC, achieved_at_ms ASC);

CREATE TABLE IF NOT EXISTS creation_public_assets (
    creation_id TEXT NOT NULL,
    media_id TEXT NOT NULL,
    asset_role TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (creation_id, media_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_creation_public_assets_media
    ON creation_public_assets(media_id);
"#;

/// Typed domain error for cloud creation, review FSM, state, review, and score operations.
#[derive(Debug)]
pub enum CreationOpError {
    NotFound,
    Forbidden(&'static str),
    RevisionConflict { expected: i64, current: i64 },
    InvalidState { code: &'static str, message: String },
    ValidationFailed(Vec<String>),
    BadRequest { code: &'static str, message: String },
    Db(TelegramError),
}

impl From<TelegramError> for CreationOpError {
    fn from(err: TelegramError) -> Self {
        Self::Db(err)
    }
}

/// Result of a creation mutation that may also mutate `public_catalog_entries`.
#[derive(Debug, Clone)]
pub struct CreationMutationResult {
    pub creation: UserCreationDto,
    /// `Some("published" | "updated" | "removed")` ONLY when `public_catalog_entries` changed.
    pub catalog_ws_action: Option<&'static str>,
}

const CREATION_COLS: &str = r#"
    creation_id, owner_user_id, creator_bot_user_id, creator_chat_id, kind,
    title, subtitle, summary, author_display_name, language,
    genres_json, tags_json, cover_json, age_rating, payload_json,
    version, cloud_revision, review_status, visibility, requested_visibility,
    has_live_published_version, published_version, published_visibility, published_snapshot_json,
    rejection_reason, review_notes, submitted_at_ms, reviewed_at_ms, published_at_ms,
    created_at_ms, updated_at_ms, deleted_at_ms,
    rating_avg, rating_count, engagement_count, save_or_install_count
"#;

#[derive(Debug, Clone)]
struct CreationDbRow {
    creation_id: String,
    owner_user_id: i64,
    creator_bot_user_id: Option<i64>,
    creator_chat_id: Option<i64>,
    kind: CreationKind,
    title: String,
    subtitle: String,
    summary: String,
    author_display_name: String,
    language: String,
    genres_json: String,
    tags_json: String,
    cover_json: String,
    age_rating: String,
    payload_json: String,
    version: u32,
    cloud_revision: i64,
    review_status: CreationReviewStatus,
    visibility: CreationVisibility,
    requested_visibility: Option<CreationVisibility>,
    has_live_published_version: bool,
    published_version: Option<u32>,
    published_visibility: Option<CreationVisibility>,
    published_snapshot_json: Option<String>,
    rejection_reason: Option<String>,
    review_notes: Option<String>,
    submitted_at_ms: Option<i64>,
    reviewed_at_ms: Option<i64>,
    published_at_ms: Option<i64>,
    created_at_ms: i64,
    updated_at_ms: i64,
    deleted_at_ms: Option<i64>,
    rating_avg: f32,
    rating_count: u32,
    engagement_count: u64,
    save_or_install_count: u64,
}

impl CreationDbRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let kind_str: String = row.get(4)?;
        let status_str: String = row.get(17)?;
        let vis_str: String = row.get(18)?;
        let req_vis_str: Option<String> = row.get(19)?;
        let pub_vis_str: Option<String> = row.get(22)?;
        Ok(Self {
            creation_id: row.get(0)?,
            owner_user_id: row.get(1)?,
            creator_bot_user_id: row.get(2)?,
            creator_chat_id: row.get(3)?,
            kind: CreationKind::parse(&kind_str).unwrap_or(CreationKind::Book),
            title: row.get(5)?,
            subtitle: row.get(6)?,
            summary: row.get(7)?,
            author_display_name: row.get(8)?,
            language: row.get(9)?,
            genres_json: row.get(10)?,
            tags_json: row.get(11)?,
            cover_json: row.get(12)?,
            age_rating: row.get(13)?,
            payload_json: row.get(14)?,
            version: row.get::<_, i64>(15)?.max(1) as u32,
            cloud_revision: row.get(16)?,
            review_status: CreationReviewStatus::parse(&status_str)
                .unwrap_or(CreationReviewStatus::Draft),
            visibility: CreationVisibility::parse(&vis_str)
                .unwrap_or(CreationVisibility::Private),
            requested_visibility: req_vis_str.as_deref().and_then(CreationVisibility::parse),
            has_live_published_version: row.get::<_, i64>(20)? != 0,
            published_version: row.get::<_, Option<i64>>(21)?.map(|v| v.max(1) as u32),
            published_visibility: pub_vis_str.as_deref().and_then(CreationVisibility::parse),
            published_snapshot_json: row.get(23)?,
            rejection_reason: row.get(24)?,
            review_notes: row.get(25)?,
            submitted_at_ms: row.get(26)?,
            reviewed_at_ms: row.get(27)?,
            published_at_ms: row.get(28)?,
            created_at_ms: row.get(29)?,
            updated_at_ms: row.get(30)?,
            deleted_at_ms: row.get(31)?,
            rating_avg: row.get::<_, f64>(32)? as f32,
            rating_count: row.get::<_, i64>(33)?.max(0) as u32,
            engagement_count: row.get::<_, i64>(34)?.max(0) as u64,
            save_or_install_count: row.get::<_, i64>(35)?.max(0) as u64,
        })
    }

    fn to_dto(&self) -> Result<UserCreationDto> {
        let genres: Vec<String> = serde_json::from_str(&self.genres_json).unwrap_or_default();
        let tags: Vec<String> = serde_json::from_str(&self.tags_json).unwrap_or_default();
        let cover: CoverArtwork = serde_json::from_str(&self.cover_json).unwrap_or_default();
        let payload: CreationPayload = serde_json::from_str(&self.payload_json)
            .map_err(|e| TelegramError::Database(format!("corrupt creation payload_json: {e}")))?;
        Ok(UserCreationDto {
            creation_id: self.creation_id.clone(),
            owner_user_id: self.owner_user_id,
            creator_bot_user_id: self.creator_bot_user_id,
            creator_chat_id: self.creator_chat_id,
            kind: self.kind,
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            summary: self.summary.clone(),
            author_display_name: self.author_display_name.clone(),
            language: self.language.clone(),
            genres,
            tags,
            cover,
            age_rating: self.age_rating.clone(),
            payload,
            version: self.version,
            cloud_revision: self.cloud_revision,
            review_status: self.review_status,
            visibility: self.visibility,
            has_live_published_version: self.has_live_published_version,
            published_version: self.published_version,
            rejection_reason: self.rejection_reason.clone(),
            review_notes: self.review_notes.clone(),
            submitted_at_ms: self.submitted_at_ms,
            reviewed_at_ms: self.reviewed_at_ms,
            published_at_ms: self.published_at_ms,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            deleted_at_ms: self.deleted_at_ms,
            rating_avg: self.rating_avg,
            rating_count: self.rating_count,
            engagement_count: self.engagement_count,
            save_or_install_count: self.save_or_install_count,
        })
    }

    fn to_live_published_snapshot_dto(&self) -> Result<Option<UserCreationDto>> {
        if self.deleted_at_ms.is_some() || !self.has_live_published_version {
            return Ok(None);
        }
        let Some(pub_vis) = self.published_visibility else {
            return Ok(None);
        };
        if !matches!(
            pub_vis,
            CreationVisibility::Public | CreationVisibility::Unlisted
        ) {
            return Ok(None);
        }
        let Some(ref snap_raw) = self.published_snapshot_json else {
            return Ok(None);
        };
        let mut dto: UserCreationDto = serde_json::from_str(snap_raw).map_err(|e| {
            TelegramError::Database(format!("corrupt published_snapshot_json: {e}"))
        })?;
        dto.review_status = CreationReviewStatus::Published;
        dto.visibility = pub_vis;
        dto.has_live_published_version = true;
        dto.published_version = self.published_version.or(Some(dto.version));
        dto.rejection_reason = None;
        dto.review_notes = None;
        dto.rating_avg = self.rating_avg;
        dto.rating_count = self.rating_count;
        dto.engagement_count = self.engagement_count;
        dto.save_or_install_count = self.save_or_install_count;
        Ok(Some(dto))
    }
}

fn fetch_creation_row_tx(
    tx: &Transaction<'_>,
    creation_id: &str,
) -> Result<Option<CreationDbRow>> {
    tx.query_row(
        &format!("SELECT {CREATION_COLS} FROM user_creations WHERE creation_id = ?1"),
        params![creation_id],
        CreationDbRow::from_row,
    )
    .optional()
    .map_db()
}

fn allocate_next_owner_revision_tx(tx: &Transaction<'_>, owner_user_id: i64) -> Result<i64> {
    tx.query_row(
        "SELECT COALESCE(MAX(cloud_revision), 0) + 1 FROM user_creations WHERE owner_user_id = ?1",
        params![owner_user_id],
        |r| r.get(0),
    )
    .map_db()
}

#[allow(clippy::too_many_arguments)]
fn record_audit_tx(
    tx: &Transaction<'_>,
    creation_id: &str,
    owner_user_id: i64,
    actor_user_id: i64,
    from_status: CreationReviewStatus,
    to_status: CreationReviewStatus,
    action: &str,
    submitted_version: u32,
    reason: Option<&str>,
    notes: Option<&str>,
    validation_errors: &[String],
    now_ms: i64,
) -> Result<()> {
    let errors_json = serde_json::to_string(validation_errors).unwrap_or_else(|_| "[]".to_string());
    tx.execute(
        r#"
        INSERT INTO creation_review_audit (
            creation_id, owner_user_id, actor_user_id, from_status, to_status,
            action, submitted_version, reason, notes, validation_errors_json, created_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        "#,
        params![
            creation_id,
            owner_user_id,
            actor_user_id,
            from_status.as_str(),
            to_status.as_str(),
            action,
            submitted_version as i64,
            reason,
            notes,
            errors_json,
            now_ms,
        ],
    )
    .map_db()?;
    Ok(())
}

pub(crate) fn format_user_display_name(
    first_name: Option<String>,
    last_name: Option<String>,
    username: Option<String>,
    user_id: i64,
) -> String {
    let first = first_name.unwrap_or_default();
    let first_t = first.trim();
    let last = last_name.unwrap_or_default();
    let last_t = last.trim();
    if !first_t.is_empty() && !last_t.is_empty() {
        return format!("{first_t} {last_t}");
    }
    if !first_t.is_empty() {
        return first_t.to_string();
    }
    if let Some(u) = username {
        let ut = u.trim();
        if !ut.is_empty() {
            return ut.to_string();
        }
    }
    format!("User {user_id}")
}

pub(crate) fn compute_popularity_score(
    rating_avg: f32,
    rating_count: u32,
    engagement_count: u64,
    save_or_install_count: u64,
) -> f64 {
    let bayesian = ((rating_avg as f64) * (rating_count as f64) + 3.5 * 2.0)
        / ((rating_count as f64) + 2.0);
    let activity = ((engagement_count.saturating_add(save_or_install_count.saturating_mul(2))) as f64).ln_1p();
    bayesian * 10.0 + activity * 5.0
}

fn push_bounded(buf: &mut String, piece: &str, max_bytes: usize) {
    let trimmed = piece.trim();
    if trimmed.is_empty() || buf.len() >= max_bytes {
        return;
    }
    if !buf.is_empty() {
        buf.push(' ');
    }
    let remaining = max_bytes.saturating_sub(buf.len());
    if trimmed.len() <= remaining {
        buf.push_str(trimmed);
    } else {
        let mut end = remaining;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        buf.push_str(&trimmed[..end]);
    }
}

fn extract_searchable_text(dto: &UserCreationDto) -> String {
    const MAX_SEARCHABLE_BYTES: usize = 32 * 1024;
    let mut buf = String::new();
    match &dto.payload {
        CreationPayload::Book(b) => {
            push_bounded(&mut buf, &b.author, MAX_SEARCHABLE_BYTES);
            if let Some(ref s) = b.series_name {
                push_bounded(&mut buf, s, MAX_SEARCHABLE_BYTES);
            }
            push_bounded(&mut buf, &b.blurb, MAX_SEARCHABLE_BYTES);
            for ch in &b.chapters {
                push_bounded(&mut buf, &ch.title, MAX_SEARCHABLE_BYTES);
                if let Some(ref sub) = ch.subtitle {
                    push_bounded(&mut buf, sub, MAX_SEARCHABLE_BYTES);
                }
                for page in &ch.pages {
                    for blk in &page.blocks {
                        match blk {
                            BookRichBlock::Heading { text, .. }
                            | BookRichBlock::Paragraph { text } => {
                                push_bounded(&mut buf, text, MAX_SEARCHABLE_BYTES);
                            }
                            BookRichBlock::BlockQuote { text, attribution } => {
                                push_bounded(&mut buf, text, MAX_SEARCHABLE_BYTES);
                                if let Some(attr) = attribution {
                                    push_bounded(&mut buf, attr, MAX_SEARCHABLE_BYTES);
                                }
                            }
                            BookRichBlock::Callout { title, text } => {
                                push_bounded(&mut buf, title, MAX_SEARCHABLE_BYTES);
                                push_bounded(&mut buf, text, MAX_SEARCHABLE_BYTES);
                            }
                            BookRichBlock::Illustration { caption, .. } => {
                                push_bounded(&mut buf, caption, MAX_SEARCHABLE_BYTES);
                            }
                            BookRichBlock::Divider => {}
                        }
                    }
                }
            }
        }
        CreationPayload::Music(m) => {
            push_bounded(&mut buf, &m.artist, MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, &m.album_title, MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, &m.primary_genre, MAX_SEARCHABLE_BYTES);
            for mood in &m.mood_tags {
                push_bounded(&mut buf, mood, MAX_SEARCHABLE_BYTES);
            }
            for tr in &m.tracks {
                push_bounded(&mut buf, &tr.title, MAX_SEARCHABLE_BYTES);
                for inst in &tr.instruments {
                    push_bounded(&mut buf, &inst.name, MAX_SEARCHABLE_BYTES);
                }
                for line in &tr.lyrics {
                    push_bounded(&mut buf, &line.text, MAX_SEARCHABLE_BYTES);
                }
            }
        }
        CreationPayload::Game(g) => {
            push_bounded(&mut buf, &g.developer, MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, g.category.as_str(), MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, &g.controls_hint, MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, &g.rules_markdown, MAX_SEARCHABLE_BYTES);
            for ach in &g.achievements {
                push_bounded(&mut buf, &ach.title, MAX_SEARCHABLE_BYTES);
                push_bounded(&mut buf, &ach.description, MAX_SEARCHABLE_BYTES);
            }
            match &g.spec {
                GameSpec::GridPuzzle { levels, .. } => {
                    for lv in levels {
                        push_bounded(&mut buf, &lv.title, MAX_SEARCHABLE_BYTES);
                    }
                }
                GameSpec::TriviaQuiz { questions, .. } => {
                    for q in questions {
                        push_bounded(&mut buf, &q.prompt, MAX_SEARCHABLE_BYTES);
                    }
                }
                GameSpec::InteractiveFiction { nodes, .. } => {
                    for n in nodes {
                        push_bounded(&mut buf, &n.title, MAX_SEARCHABLE_BYTES);
                        push_bounded(&mut buf, &n.body, MAX_SEARCHABLE_BYTES);
                    }
                }
                GameSpec::CardMatch { pairs, .. } => {
                    for p in pairs {
                        push_bounded(&mut buf, &p.label, MAX_SEARCHABLE_BYTES);
                    }
                }
                GameSpec::MiniAppHtml {
                    bot_username,
                    short_name,
                    ..
                } => {
                    push_bounded(&mut buf, bot_username, MAX_SEARCHABLE_BYTES);
                    push_bounded(&mut buf, short_name, MAX_SEARCHABLE_BYTES);
                }
                GameSpec::ArcadeDodge { .. } | GameSpec::SnakeGrid { .. } => {}
            }
        }
        CreationPayload::App(a) => {
            push_bounded(&mut buf, &a.developer_name, MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, &a.category, MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, &a.long_description, MAX_SEARCHABLE_BYTES);
            push_bounded(&mut buf, &a.release_notes, MAX_SEARCHABLE_BYTES);
            for shot in &a.screenshots {
                push_bounded(&mut buf, &shot.title, MAX_SEARCHABLE_BYTES);
                if let Some(ref c) = shot.caption {
                    push_bounded(&mut buf, c, MAX_SEARCHABLE_BYTES);
                }
            }
            if let Some(ref linked) = a.linked_miniapp {
                push_bounded(&mut buf, &linked.bot_username, MAX_SEARCHABLE_BYTES);
                push_bounded(&mut buf, &linked.short_name, MAX_SEARCHABLE_BYTES);
            }
        }
    }
    buf
}

fn normalize_media_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(trimmed.to_ascii_lowercase())
    } else {
        Some(trimmed.to_string())
    }
}

/// Extracts all 5 M1 `media_file_id` locations from a creation snapshot, normalized & deduplicated.
pub(crate) fn extract_creation_media_assets(dto: &UserCreationDto) -> Vec<(String, &'static str)> {
    let mut seen = HashSet::new();
    let mut assets = Vec::new();
    let mut push_opt = |opt: &Option<String>, role: &'static str| {
        if let Some(raw) = opt {
            if let Some(normalized) = normalize_media_id(raw) {
                if seen.insert(normalized.clone()) {
                    assets.push((normalized, role));
                }
            }
        }
    };

    // 1. CoverArtwork.media_file_id
    push_opt(&dto.cover.media_file_id, "cover");

    match &dto.payload {
        // 2. BookRichBlock::Illustration.media_file_id
        CreationPayload::Book(b) => {
            for ch in &b.chapters {
                for page in &ch.pages {
                    for blk in &page.blocks {
                        if let BookRichBlock::Illustration { media_file_id, .. } = blk {
                            push_opt(media_file_id, "book_illustration");
                        }
                    }
                }
            }
        }
        // 3. MusicTrack.audio_media_file_id
        CreationPayload::Music(m) => {
            for tr in &m.tracks {
                push_opt(&tr.audio_media_file_id, "music_audio");
            }
        }
        // 4. GameSpec::MiniAppHtml.html_bundle_media_id
        CreationPayload::Game(g) => {
            if let GameSpec::MiniAppHtml {
                html_bundle_media_id,
                ..
            } = &g.spec
            {
                push_opt(html_bundle_media_id, "game_html_bundle");
            }
        }
        // 5. ScreenshotAsset.media_file_id
        CreationPayload::App(a) => {
            for shot in &a.screenshots {
                push_opt(&shot.media_file_id, "app_screenshot");
            }
        }
    }
    assets
}

/// Cross-user media exfiltration guard (fail-closed):
/// Only an existing `media_files` row owned by (or chat-accessible to) the creation owner may be
/// authorized in `creation_public_assets`. Missing rows fail closed (`Ok(false)`) to prevent
/// pre-upload CAS ACL squatting on future SHA-256 digests.
fn owner_can_publish_media_tx(
    tx: &Transaction<'_>,
    owner_user_id: i64,
    media_id: &str,
) -> Result<bool> {
    let meta: Option<(i64, Option<i64>)> = tx
        .query_row(
            "SELECT uploader_id, chat_id FROM media_files WHERE LOWER(media_id) = LOWER(?1) ORDER BY CASE WHEN media_id = ?1 THEN 0 ELSE 1 END LIMIT 1",
            params![media_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_db()?;

    let Some((uploader_id, chat_id_opt)) = meta else {
        // Fail closed: never pre-register nonexistent or future SHA-256 digests.
        return Ok(false);
    };
    if uploader_id == owner_user_id {
        return Ok(true);
    }
    if let Some(chat_id) = chat_id_opt {
        let is_creator: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chats WHERE chat_id = ?1 AND creator_id = ?2)",
                params![chat_id, owner_user_id],
                |r| r.get(0),
            )
            .map_db()?;
        if is_creator {
            return Ok(true);
        }
        let is_member: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_members WHERE chat_id = ?1 AND user_id = ?2 AND LOWER(TRIM(role)) NOT IN ('kicked', 'banned', 'left'))",
                params![chat_id, owner_user_id],
                |r| r.get(0),
            )
            .map_db()?;
        return Ok(is_member);
    }
    Ok(false)
}

fn upsert_public_catalog_tx(
    tx: &Transaction<'_>,
    snapshot: &UserCreationDto,
    snapshot_json: &str,
    now_ms: i64,
) -> Result<()> {
    let genres_json = serde_json::to_string(&snapshot.genres).unwrap_or_else(|_| "[]".to_string());
    let tags_json = serde_json::to_string(&snapshot.tags).unwrap_or_else(|_| "[]".to_string());
    let cover_json = serde_json::to_string(&snapshot.cover).unwrap_or_else(|_| "{}".to_string());
    let genres_text = snapshot.genres.join(" ");
    let tags_text = snapshot.tags.join(" ");
    let searchable_text = extract_searchable_text(snapshot);
    let published_version = snapshot.published_version.unwrap_or(snapshot.version) as i64;
    let published_at_ms = snapshot.published_at_ms.unwrap_or(now_ms);
    let popularity = compute_popularity_score(
        snapshot.rating_avg,
        snapshot.rating_count,
        snapshot.engagement_count,
        snapshot.save_or_install_count,
    );

    // CRITICAL: Use ON CONFLICT DO UPDATE (never INSERT OR REPLACE) so FTS5 triggers stay exact.
    tx.execute(
        r#"
        INSERT INTO public_catalog_entries (
            creation_id, owner_user_id, kind, title, subtitle, summary,
            author_display_name, language, genres_json, tags_json,
            genres_text, tags_text, searchable_text, cover_json, age_rating,
            snapshot_json, published_version, featured_rank,
            rating_avg, rating_count, engagement_count, save_or_install_count,
            popularity_score, published_at_ms, updated_at_ms
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, NULL,
            ?18, ?19, ?20, ?21,
            ?22, ?23, ?24
        )
        ON CONFLICT(creation_id) DO UPDATE SET
            owner_user_id = excluded.owner_user_id,
            kind = excluded.kind,
            title = excluded.title,
            subtitle = excluded.subtitle,
            summary = excluded.summary,
            author_display_name = excluded.author_display_name,
            language = excluded.language,
            genres_json = excluded.genres_json,
            tags_json = excluded.tags_json,
            genres_text = excluded.genres_text,
            tags_text = excluded.tags_text,
            searchable_text = excluded.searchable_text,
            cover_json = excluded.cover_json,
            age_rating = excluded.age_rating,
            snapshot_json = excluded.snapshot_json,
            published_version = excluded.published_version,
            rating_avg = excluded.rating_avg,
            rating_count = excluded.rating_count,
            engagement_count = excluded.engagement_count,
            save_or_install_count = excluded.save_or_install_count,
            popularity_score = excluded.popularity_score,
            published_at_ms = excluded.published_at_ms,
            updated_at_ms = excluded.updated_at_ms
        "#,
        params![
            snapshot.creation_id,
            snapshot.owner_user_id,
            snapshot.kind.as_str(),
            snapshot.title,
            snapshot.subtitle,
            snapshot.summary,
            snapshot.author_display_name,
            snapshot.language,
            genres_json,
            tags_json,
            genres_text,
            tags_text,
            searchable_text,
            cover_json,
            snapshot.age_rating,
            snapshot_json,
            published_version,
            snapshot.rating_avg as f64,
            snapshot.rating_count as i64,
            snapshot.engagement_count as i64,
            snapshot.save_or_install_count as i64,
            popularity,
            published_at_ms,
            now_ms,
        ],
    )
    .map_db()?;
    Ok(())
}

fn sync_public_assets_tx(
    tx: &Transaction<'_>,
    creation_id: &str,
    owner_user_id: i64,
    snapshot: &UserCreationDto,
    now_ms: i64,
) -> Result<()> {
    tx.execute(
        "DELETE FROM creation_public_assets WHERE creation_id = ?1",
        params![creation_id],
    )
    .map_db()?;
    for (media_id, asset_role) in extract_creation_media_assets(snapshot) {
        if !owner_can_publish_media_tx(tx, owner_user_id, &media_id)? {
            continue;
        }
        tx.execute(
            "INSERT OR IGNORE INTO creation_public_assets (creation_id, media_id, asset_role, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![creation_id, media_id, asset_role, now_ms],
        )
        .map_db()?;
    }
    Ok(())
}

fn purge_public_catalog_and_assets_tx(tx: &Transaction<'_>, creation_id: &str) -> Result<bool> {
    let deleted = tx
        .execute(
            "DELETE FROM public_catalog_entries WHERE creation_id = ?1",
            params![creation_id],
        )
        .map_db()?;
    tx.execute(
        "DELETE FROM creation_public_assets WHERE creation_id = ?1",
        params![creation_id],
    )
    .map_db()?;
    Ok(deleted > 0)
}

impl ServerDatabase {
    pub fn create_user_creation(
        &self,
        owner_user_id: UserId,
        req: &CreateCreationRequest,
        now_ms: i64,
    ) -> std::result::Result<UserCreationDto, CreationOpError> {
        if req.kind != req.payload.kind() {
            return Err(CreationOpError::BadRequest {
                code: "kind_mismatch",
                message: format!(
                    "creation kind '{}' does not match payload kind '{}'",
                    req.kind.as_str(),
                    req.payload.kind().as_str()
                ),
            });
        }
        let owner_id = owner_user_id.0 as i64;
        let creation_id = format!("cr_{}", uuid::Uuid::new_v4().simple());
        let genres_json = serde_json::to_string(&req.genres).unwrap_or_else(|_| "[]".to_string());
        let tags_json = serde_json::to_string(&req.tags).unwrap_or_else(|_| "[]".to_string());
        let cover_json = serde_json::to_string(&req.cover).unwrap_or_else(|_| "{}".to_string());
        let payload_json = serde_json::to_string(&req.payload).map_err(|e| {
            CreationOpError::BadRequest {
                code: "invalid_payload",
                message: format!("failed to serialize creation payload: {e}"),
            }
        })?;
        let requested_visibility = match req.visibility {
            CreationVisibility::Unlisted => Some(CreationVisibility::Unlisted.as_str()),
            _ => None,
        };

        let row = self.pool.with_transaction(|tx| {
            let next_rev = allocate_next_owner_revision_tx(tx, owner_id)?;
            tx.execute(
                r#"
                INSERT INTO user_creations (
                    creation_id, owner_user_id, creator_bot_user_id, creator_chat_id, kind,
                    title, subtitle, summary, author_display_name, language,
                    genres_json, tags_json, cover_json, age_rating, payload_json,
                    version, cloud_revision, review_status, visibility, requested_visibility,
                    has_live_published_version, created_at_ms, updated_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5,
                    ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, ?15,
                    1, ?16, 'draft', 'private', ?17,
                    0, ?18, ?18
                )
                "#,
                params![
                    creation_id,
                    owner_id,
                    req.creator_bot_user_id,
                    req.creator_chat_id,
                    req.kind.as_str(),
                    req.title,
                    req.subtitle,
                    req.summary,
                    req.author_display_name,
                    req.language,
                    genres_json,
                    tags_json,
                    cover_json,
                    req.age_rating,
                    payload_json,
                    next_rev,
                    requested_visibility,
                    now_ms,
                ],
            )
            .map_db()?;
            fetch_creation_row_tx(tx, &creation_id)?
                .ok_or_else(|| TelegramError::Database("inserted creation not found".to_string()))
        })?;
        Ok(row.to_dto()?)
    }

    pub fn list_my_creations(
        &self,
        owner_user_id: UserId,
        kind_filter: Option<CreationKind>,
        status_filter: Option<CreationReviewStatus>,
        query_filter: Option<&str>,
    ) -> Result<CreationListResponse> {
        let owner_id = owner_user_id.0 as i64;
        if query_filter.is_some_and(|s| s.contains('\0')) {
            let latest_revision: i64 = self.pool.with_reader(|conn| {
                conn.query_row(
                    "SELECT COALESCE(MAX(cloud_revision), 0) FROM user_creations WHERE owner_user_id = ?1",
                    params![owner_id],
                    |r| r.get(0),
                )
                .map_db()
            })?;
            return Ok(CreationListResponse {
                items: Vec::new(),
                latest_revision,
            });
        }
        let q_lower = query_filter
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase());
        self.pool.with_reader(|conn| {
            let mut latest_revision: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(cloud_revision), 0) FROM user_creations WHERE owner_user_id = ?1",
                    params![owner_id],
                    |r| r.get(0),
                )
                .map_db()?;

            let mut sql = format!(
                "SELECT {CREATION_COLS} FROM user_creations WHERE owner_user_id = ?1 AND deleted_at_ms IS NULL"
            );
            let mut bind_vals: Vec<rusqlite::types::Value> = vec![owner_id.into()];
            if let Some(k) = kind_filter {
                sql.push_str(&format!(" AND kind = ?{}", bind_vals.len() + 1));
                bind_vals.push(k.as_str().to_string().into());
            }
            if let Some(st) = status_filter {
                sql.push_str(&format!(" AND review_status = ?{}", bind_vals.len() + 1));
                bind_vals.push(st.as_str().to_string().into());
            }
            sql.push_str(" ORDER BY updated_at_ms DESC, cloud_revision DESC");

            let mut stmt = conn.prepare(&sql).map_db()?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind_vals), CreationDbRow::from_row)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            let mut items = Vec::with_capacity(rows.len());
            for row in rows {
                latest_revision = latest_revision.max(row.cloud_revision);
                let dto = row.to_dto()?;
                if let Some(ref ql) = q_lower {
                    let matched = dto.title.to_lowercase().contains(ql)
                        || dto.subtitle.to_lowercase().contains(ql)
                        || dto.summary.to_lowercase().contains(ql)
                        || dto.author_display_name.to_lowercase().contains(ql)
                        || dto.genres.iter().any(|g| g.to_lowercase().contains(ql))
                        || dto.tags.iter().any(|t| t.to_lowercase().contains(ql));
                    if !matched {
                        continue;
                    }
                }
                items.push(dto);
            }
            latest_revision = items
                .iter()
                .map(|i| i.cloud_revision)
                .fold(latest_revision, i64::max);
            Ok(CreationListResponse {
                items,
                latest_revision,
            })
        })
    }

    pub fn sync_my_creations(
        &self,
        owner_user_id: UserId,
        since_revision: i64,
    ) -> Result<CreationSyncResponse> {
        let owner_id = owner_user_id.0 as i64;
        self.pool.with_reader(|conn| {
            let latest_revision: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(cloud_revision), 0) FROM user_creations WHERE owner_user_id = ?1",
                    params![owner_id],
                    |r| r.get(0),
                )
                .map_db()?;
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {CREATION_COLS} FROM user_creations WHERE owner_user_id = ?1 AND cloud_revision > ?2 ORDER BY cloud_revision ASC"
                ))
                .map_db()?;
            let rows = stmt
                .query_map(params![owner_id, since_revision], CreationDbRow::from_row)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            let mut items = Vec::with_capacity(rows.len());
            for row in rows {
                items.push(row.to_dto()?);
            }
            let latest_revision = items
                .iter()
                .map(|i| i.cloud_revision)
                .fold(latest_revision, i64::max);
            Ok(CreationSyncResponse {
                latest_revision,
                items,
            })
        })
    }

    pub fn get_creation_for_caller(
        &self,
        creation_id: &str,
        caller_user_id: UserId,
        caller_is_admin: bool,
    ) -> std::result::Result<UserCreationDto, CreationOpError> {
        let caller_id = caller_user_id.0 as i64;
        let row_opt = self.pool.with_reader(|conn| {
            conn.query_row(
                &format!("SELECT {CREATION_COLS} FROM user_creations WHERE creation_id = ?1"),
                params![creation_id],
                CreationDbRow::from_row,
            )
            .optional()
            .map_db()
        })?;

        let Some(row) = row_opt else {
            return Err(CreationOpError::NotFound);
        };
        if row.deleted_at_ms.is_some() {
            return Err(CreationOpError::NotFound);
        }
        if row.owner_user_id == caller_id {
            return Ok(row.to_dto()?);
        }
        if caller_is_admin && row.review_status == CreationReviewStatus::PendingReview {
            return Ok(row.to_dto()?);
        }
        if let Some(snapshot_dto) = row.to_live_published_snapshot_dto()? {
            return Ok(snapshot_dto);
        }
        Err(CreationOpError::Forbidden(
            "You do not have permission to view this creation",
        ))
    }
}

impl ServerDatabase {
    pub fn update_user_creation(
        &self,
        creation_id: &str,
        owner_user_id: UserId,
        req: &UpdateCreationRequest,
        now_ms: i64,
    ) -> std::result::Result<UserCreationDto, CreationOpError> {
        if let Some(exp) = req.expected_revision {
            if exp < 0 {
                return Err(CreationOpError::BadRequest {
                    code: "invalid_expected_revision",
                    message: "expected_revision must be >= 0".to_string(),
                });
            }
        }
        let owner_id = owner_user_id.0 as i64;
        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            if row.owner_user_id != owner_id {
                return Ok(Err(CreationOpError::Forbidden(
                    "Only the creation owner can edit this creation",
                )));
            }
            if row.review_status == CreationReviewStatus::PendingReview {
                return Ok(Err(CreationOpError::InvalidState {
                    code: "pending_review_locked",
                    message: "Cannot edit creation while pending review; withdraw review first"
                        .to_string(),
                }));
            }
            if let Some(exp) = req.expected_revision {
                if exp != row.cloud_revision {
                    return Ok(Err(CreationOpError::RevisionConflict {
                        expected: exp,
                        current: row.cloud_revision,
                    }));
                }
            }
            if let Some(ref payload) = req.payload {
                if payload.kind() != row.kind {
                    return Ok(Err(CreationOpError::BadRequest {
                        code: "kind_mismatch",
                        message: format!(
                            "creation kind '{}' does not match payload kind '{}'",
                            row.kind.as_str(),
                            payload.kind().as_str()
                        ),
                    }));
                }
            }

            let title = req.title.clone().unwrap_or(row.title);
            let subtitle = req.subtitle.clone().unwrap_or(row.subtitle);
            let summary = req.summary.clone().unwrap_or(row.summary);
            let author_display_name = req
                .author_display_name
                .clone()
                .unwrap_or(row.author_display_name);
            let language = req.language.clone().unwrap_or(row.language);
            let genres_json = match &req.genres {
                Some(g) => serde_json::to_string(g).unwrap_or_else(|_| "[]".to_string()),
                None => row.genres_json,
            };
            let tags_json = match &req.tags {
                Some(t) => serde_json::to_string(t).unwrap_or_else(|_| "[]".to_string()),
                None => row.tags_json,
            };
            let cover_json = match &req.cover {
                Some(c) => serde_json::to_string(c).unwrap_or_else(|_| "{}".to_string()),
                None => row.cover_json,
            };
            let age_rating = req.age_rating.clone().unwrap_or(row.age_rating);
            let payload_json = match &req.payload {
                Some(p) => match serde_json::to_string(p) {
                    Ok(s) => s,
                    Err(e) => {
                        return Ok(Err(CreationOpError::BadRequest {
                            code: "invalid_payload",
                            message: format!("failed to serialize creation payload: {e}"),
                        }));
                    }
                },
                None => row.payload_json,
            };

            let (next_status, next_rejection_reason, next_review_notes) = match row.review_status {
                CreationReviewStatus::Rejected => (CreationReviewStatus::Draft, None, None),
                other => (other, row.rejection_reason, row.review_notes),
            };
            let next_requested_vis = match req.visibility {
                Some(CreationVisibility::Unlisted) => Some(CreationVisibility::Unlisted.as_str()),
                Some(CreationVisibility::Public) => Some(CreationVisibility::Public.as_str()),
                Some(CreationVisibility::Private) => None,
                None => row.requested_visibility.map(|v| v.as_str()),
            };
            let next_version = row.version.saturating_add(1) as i64;
            let next_rev = allocate_next_owner_revision_tx(tx, owner_id)?;

            tx.execute(
                r#"
                UPDATE user_creations
                SET title = ?1, subtitle = ?2, summary = ?3, author_display_name = ?4,
                    language = ?5, genres_json = ?6, tags_json = ?7, cover_json = ?8,
                    age_rating = ?9, payload_json = ?10, version = ?11, cloud_revision = ?12,
                    review_status = ?13, requested_visibility = ?14, rejection_reason = ?15,
                    review_notes = ?16, updated_at_ms = ?17
                WHERE creation_id = ?18
                "#,
                params![
                    title,
                    subtitle,
                    summary,
                    author_display_name,
                    language,
                    genres_json,
                    tags_json,
                    cover_json,
                    age_rating,
                    payload_json,
                    next_version,
                    next_rev,
                    next_status.as_str(),
                    next_requested_vis,
                    next_rejection_reason,
                    next_review_notes,
                    now_ms,
                    creation_id,
                ],
            )
            .map_db()?;

            let updated = fetch_creation_row_tx(tx, creation_id)?
                .ok_or_else(|| TelegramError::Database("updated creation disappeared".to_string()))?;
            Ok(Ok(updated.to_dto()?))
        })?;
        outcome
    }

    pub fn submit_creation_for_review(
        &self,
        creation_id: &str,
        owner_user_id: UserId,
        req: &SubmitReviewRequest,
        now_ms: i64,
    ) -> std::result::Result<UserCreationDto, CreationOpError> {
        let owner_id = owner_user_id.0 as i64;
        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            if row.owner_user_id != owner_id {
                return Ok(Err(CreationOpError::Forbidden(
                    "Only the creation owner can submit for review",
                )));
            }
            if row.review_status == CreationReviewStatus::PendingReview {
                return Ok(Err(CreationOpError::InvalidState {
                    code: "already_pending_review",
                    message: "Creation is already pending review".to_string(),
                }));
            }

            if req.visibility == Some(CreationVisibility::Private) {
                return Ok(Err(CreationOpError::BadRequest {
                    code: "invalid_visibility",
                    message: "Submitted creation visibility cannot be private".to_string(),
                }));
            }

            let mut candidate = row.to_dto()?;
            // Clear stale moderation text before validating the new submission cycle.
            candidate.rejection_reason = None;
            candidate.review_notes = req.notes.clone();
            if let Err(errors) = validate_creation_for_review(&candidate) {
                return Ok(Err(CreationOpError::ValidationFailed(errors)));
            }

            let requested_vis = match req.visibility {
                Some(CreationVisibility::Unlisted) => Some(CreationVisibility::Unlisted.as_str()),
                Some(CreationVisibility::Public) => Some(CreationVisibility::Public.as_str()),
                _ => row.requested_visibility.map(|v| v.as_str()),
            };
            let next_rev = allocate_next_owner_revision_tx(tx, owner_id)?;

            tx.execute(
                r#"
                UPDATE user_creations
                SET review_status = 'pending_review',
                    requested_visibility = ?1,
                    rejection_reason = NULL,
                    review_notes = ?2,
                    submitted_at_ms = ?3,
                    updated_at_ms = ?3,
                    cloud_revision = ?4
                WHERE creation_id = ?5
                "#,
                params![
                    requested_vis,
                    candidate.review_notes,
                    now_ms,
                    next_rev,
                    creation_id,
                ],
            )
            .map_db()?;

            record_audit_tx(
                tx,
                creation_id,
                owner_id,
                owner_id,
                row.review_status,
                CreationReviewStatus::PendingReview,
                "submit_review",
                row.version,
                None,
                req.notes.as_deref(),
                &[],
                now_ms,
            )?;

            let updated = fetch_creation_row_tx(tx, creation_id)?
                .ok_or_else(|| TelegramError::Database("submitted creation missing".to_string()))?;
            Ok(Ok(updated.to_dto()?))
        })?;
        outcome
    }
}

impl ServerDatabase {
    pub fn withdraw_creation_review(
        &self,
        creation_id: &str,
        owner_user_id: UserId,
        now_ms: i64,
    ) -> std::result::Result<UserCreationDto, CreationOpError> {
        let owner_id = owner_user_id.0 as i64;
        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            if row.owner_user_id != owner_id {
                return Ok(Err(CreationOpError::Forbidden(
                    "Only the creation owner can withdraw review",
                )));
            }
            if row.review_status != CreationReviewStatus::PendingReview {
                return Ok(Err(CreationOpError::InvalidState {
                    code: "not_pending_review",
                    message: "Only pending_review creations can be withdrawn".to_string(),
                }));
            }

            let next_rev = allocate_next_owner_revision_tx(tx, owner_id)?;
            tx.execute(
                r#"
                UPDATE user_creations
                SET review_status = 'draft',
                    updated_at_ms = ?1,
                    cloud_revision = ?2
                WHERE creation_id = ?3
                "#,
                params![now_ms, next_rev, creation_id],
            )
            .map_db()?;

            record_audit_tx(
                tx,
                creation_id,
                owner_id,
                owner_id,
                CreationReviewStatus::PendingReview,
                CreationReviewStatus::Draft,
                "withdraw_review",
                row.version,
                None,
                None,
                &[],
                now_ms,
            )?;

            let updated = fetch_creation_row_tx(tx, creation_id)?
                .ok_or_else(|| TelegramError::Database("withdrawn creation missing".to_string()))?;
            Ok(Ok(updated.to_dto()?))
        })?;
        outcome
    }

    pub fn list_creation_review_queue(
        &self,
        status_filter: Option<CreationReviewStatus>,
        kind_filter: Option<CreationKind>,
    ) -> Result<CreationListResponse> {
        self.pool.with_reader(|conn| {
            let mut sql = format!(
                "SELECT {CREATION_COLS} FROM user_creations WHERE deleted_at_ms IS NULL"
            );
            let mut bind_vals: Vec<rusqlite::types::Value> = Vec::new();
            if let Some(st) = status_filter {
                sql.push_str(&format!(" AND review_status = ?{}", bind_vals.len() + 1));
                bind_vals.push(st.as_str().to_string().into());
            }
            if let Some(k) = kind_filter {
                sql.push_str(&format!(" AND kind = ?{}", bind_vals.len() + 1));
                bind_vals.push(k.as_str().to_string().into());
            }
            sql.push_str(" ORDER BY COALESCE(submitted_at_ms, updated_at_ms) ASC, creation_id ASC");

            let mut stmt = conn.prepare(&sql).map_db()?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind_vals), CreationDbRow::from_row)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            let mut items = Vec::with_capacity(rows.len());
            for row in rows {
                items.push(row.to_dto()?);
            }
            Ok(CreationListResponse {
                items,
                latest_revision: 0,
            })
        })
    }

    pub fn review_creation_decision(
        &self,
        creation_id: &str,
        admin_user_id: UserId,
        req: &ReviewDecisionRequest,
        now_ms: i64,
    ) -> std::result::Result<CreationMutationResult, CreationOpError> {
        let admin_id = admin_user_id.0 as i64;
        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            if row.review_status != CreationReviewStatus::PendingReview {
                return Ok(Err(CreationOpError::InvalidState {
                    code: "not_pending_review",
                    message: format!(
                        "Creation must be in pending_review to review (current: '{}')",
                        row.review_status.as_str()
                    ),
                }));
            }

            match req.decision {
                ReviewDecision::Approve => {
                    if req.visibility == Some(CreationVisibility::Private) {
                        return Ok(Err(CreationOpError::BadRequest {
                            code: "invalid_visibility",
                            message: "Approved creation visibility cannot be private".to_string(),
                        }));
                    }
                    let target_vis = req.visibility.unwrap_or_else(|| {
                        match row.requested_visibility.or(row.published_visibility) {
                            Some(CreationVisibility::Unlisted) => CreationVisibility::Unlisted,
                            _ => CreationVisibility::Public,
                        }
                    });
                    let mut candidate = row.to_dto()?;
                    candidate.rejection_reason = None;
                    let next_notes = req.notes.clone().or(candidate.review_notes.clone());
                    candidate.review_notes = next_notes.clone();
                    if let Err(errors) = validate_creation_for_review(&candidate) {
                        return Ok(Err(CreationOpError::ValidationFailed(errors)));
                    }

                    let next_rev = allocate_next_owner_revision_tx(tx, row.owner_user_id)?;
                    let mut snapshot = candidate.clone();
                    snapshot.review_status = CreationReviewStatus::Published;
                    snapshot.visibility = target_vis;
                    snapshot.has_live_published_version = true;
                    snapshot.published_version = Some(row.version);
                    snapshot.rejection_reason = None;
                    snapshot.review_notes = None;
                    snapshot.reviewed_at_ms = Some(now_ms);
                    snapshot.published_at_ms = Some(now_ms);
                    snapshot.updated_at_ms = now_ms;
                    snapshot.cloud_revision = next_rev;
                    let snapshot_json = serde_json::to_string(&snapshot)
                        .map_err(|e| TelegramError::Database(format!("snapshot encode failed: {e}")))?;

                    let was_in_public: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM public_catalog_entries WHERE creation_id = ?1)",
                            params![creation_id],
                            |r| r.get(0),
                        )
                        .map_db()?;

                    tx.execute(
                        r#"
                        UPDATE user_creations
                        SET review_status = 'published',
                            visibility = ?1,
                            requested_visibility = NULL,
                            has_live_published_version = 1,
                            published_version = ?2,
                            published_visibility = ?1,
                            published_snapshot_json = ?3,
                            rejection_reason = NULL,
                            review_notes = ?4,
                            reviewed_at_ms = ?5,
                            published_at_ms = ?5,
                            updated_at_ms = ?5,
                            cloud_revision = ?6
                        WHERE creation_id = ?7
                        "#,
                        params![
                            target_vis.as_str(),
                            row.version as i64,
                            snapshot_json,
                            next_notes,
                            now_ms,
                            next_rev,
                            creation_id,
                        ],
                    )
                    .map_db()?;

                    let catalog_ws_action = if target_vis == CreationVisibility::Public {
                        upsert_public_catalog_tx(tx, &snapshot, &snapshot_json, now_ms)?;
                        sync_public_assets_tx(tx, creation_id, row.owner_user_id, &snapshot, now_ms)?;
                        Some(if was_in_public { "updated" } else { "published" })
                    } else {
                        let removed = purge_public_catalog_and_assets_tx(tx, creation_id)?;
                        if removed { Some("removed") } else { None }
                    };

                    record_audit_tx(
                        tx,
                        creation_id,
                        row.owner_user_id,
                        admin_id,
                        CreationReviewStatus::PendingReview,
                        CreationReviewStatus::Published,
                        "approve",
                        row.version,
                        None,
                        req.notes.as_deref(),
                        &[],
                        now_ms,
                    )?;

                    let updated = fetch_creation_row_tx(tx, creation_id)?
                        .ok_or_else(|| TelegramError::Database("approved creation missing".to_string()))?;
                    Ok(Ok(CreationMutationResult {
                        creation: updated.to_dto()?,
                        catalog_ws_action,
                    }))
                }
                ReviewDecision::Reject => {
                    let reason = req
                        .reason
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let Some(reason_str) = reason else {
                        return Ok(Err(CreationOpError::BadRequest {
                            code: "missing_rejection_reason",
                            message: "A non-empty rejection reason is required".to_string(),
                        }));
                    };
                    let next_rev = allocate_next_owner_revision_tx(tx, row.owner_user_id)?;
                    let next_notes = req.notes.clone().or(row.review_notes.clone());

                    tx.execute(
                        r#"
                        UPDATE user_creations
                        SET review_status = 'rejected',
                            rejection_reason = ?1,
                            review_notes = ?2,
                            reviewed_at_ms = ?3,
                            updated_at_ms = ?3,
                            cloud_revision = ?4
                        WHERE creation_id = ?5
                        "#,
                        params![reason_str, next_notes, now_ms, next_rev, creation_id],
                    )
                    .map_db()?;

                    record_audit_tx(
                        tx,
                        creation_id,
                        row.owner_user_id,
                        admin_id,
                        CreationReviewStatus::PendingReview,
                        CreationReviewStatus::Rejected,
                        "reject",
                        row.version,
                        Some(reason_str),
                        req.notes.as_deref(),
                        &[],
                        now_ms,
                    )?;

                    let updated = fetch_creation_row_tx(tx, creation_id)?
                        .ok_or_else(|| TelegramError::Database("rejected creation missing".to_string()))?;
                    Ok(Ok(CreationMutationResult {
                        creation: updated.to_dto()?,
                        catalog_ws_action: None,
                    }))
                }
            }
        })?;
        outcome
    }
}

impl ServerDatabase {
    pub fn unpublish_creation(
        &self,
        creation_id: &str,
        caller_user_id: UserId,
        caller_is_admin: bool,
        now_ms: i64,
    ) -> std::result::Result<CreationMutationResult, CreationOpError> {
        let caller_id = caller_user_id.0 as i64;
        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            if row.owner_user_id != caller_id && !caller_is_admin {
                return Ok(Err(CreationOpError::Forbidden(
                    "Only the creation owner or an admin can unpublish this creation",
                )));
            }
            if row.review_status != CreationReviewStatus::Published
                && !row.has_live_published_version
            {
                return Ok(Err(CreationOpError::InvalidState {
                    code: "not_published",
                    message: "Creation does not have a live published version".to_string(),
                }));
            }

            let next_rev = allocate_next_owner_revision_tx(tx, row.owner_user_id)?;
            tx.execute(
                r#"
                UPDATE user_creations
                SET review_status = 'unpublished',
                    visibility = 'private',
                    requested_visibility = NULL,
                    has_live_published_version = 0,
                    published_visibility = NULL,
                    published_snapshot_json = NULL,
                    updated_at_ms = ?1,
                    cloud_revision = ?2
                WHERE creation_id = ?3
                "#,
                params![now_ms, next_rev, creation_id],
            )
            .map_db()?;

            let was_in_public = purge_public_catalog_and_assets_tx(tx, creation_id)?;

            record_audit_tx(
                tx,
                creation_id,
                row.owner_user_id,
                caller_id,
                row.review_status,
                CreationReviewStatus::Unpublished,
                "unpublish",
                row.version,
                None,
                None,
                &[],
                now_ms,
            )?;

            let updated = fetch_creation_row_tx(tx, creation_id)?
                .ok_or_else(|| TelegramError::Database("unpublished creation missing".to_string()))?;
            Ok(Ok(CreationMutationResult {
                creation: updated.to_dto()?,
                catalog_ws_action: if was_in_public { Some("removed") } else { None },
            }))
        })?;
        outcome
    }

    pub fn soft_delete_creation(
        &self,
        creation_id: &str,
        owner_user_id: UserId,
        now_ms: i64,
    ) -> std::result::Result<CreationMutationResult, CreationOpError> {
        let owner_id = owner_user_id.0 as i64;
        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            if row.owner_user_id != owner_id {
                return Ok(Err(CreationOpError::Forbidden(
                    "Only the creation owner can delete this creation",
                )));
            }

            let next_status = if row.review_status == CreationReviewStatus::Published
                || row.has_live_published_version
            {
                CreationReviewStatus::Unpublished
            } else {
                row.review_status
            };
            let next_rev = allocate_next_owner_revision_tx(tx, owner_id)?;

            tx.execute(
                r#"
                UPDATE user_creations
                SET review_status = ?1,
                    visibility = 'private',
                    requested_visibility = NULL,
                    has_live_published_version = 0,
                    published_visibility = NULL,
                    published_snapshot_json = NULL,
                    deleted_at_ms = ?2,
                    updated_at_ms = ?2,
                    cloud_revision = ?3
                WHERE creation_id = ?4
                "#,
                params![next_status.as_str(), now_ms, next_rev, creation_id],
            )
            .map_db()?;

            let was_in_public = purge_public_catalog_and_assets_tx(tx, creation_id)?;

            record_audit_tx(
                tx,
                creation_id,
                owner_id,
                owner_id,
                row.review_status,
                next_status,
                "delete",
                row.version,
                None,
                None,
                &[],
                now_ms,
            )?;

            let updated = fetch_creation_row_tx(tx, creation_id)?
                .ok_or_else(|| TelegramError::Database("deleted creation missing".to_string()))?;
            Ok(Ok(CreationMutationResult {
                creation: updated.to_dto()?,
                catalog_ws_action: if was_in_public { Some("removed") } else { None },
            }))
        })?;
        outcome
    }

    pub fn list_creation_audit(
        &self,
        creation_id: &str,
        caller_user_id: UserId,
        caller_is_admin: bool,
    ) -> std::result::Result<CreationAuditListResponse, CreationOpError> {
        let caller_id = caller_user_id.0 as i64;
        self.pool.with_reader(|conn| {
            // Note: DO NOT filter deleted_at_ms IS NULL so audit trail stays readable after soft-delete.
            let owner_opt: Option<i64> = conn
                .query_row(
                    "SELECT owner_user_id FROM user_creations WHERE creation_id = ?1",
                    params![creation_id],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            let Some(owner_id) = owner_opt else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if caller_id != owner_id && !caller_is_admin {
                return Ok(Err(CreationOpError::Forbidden(
                    "Only the creation owner or an admin can inspect review audit history",
                )));
            }

            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT audit_id, creation_id, actor_user_id, from_status, to_status,
                           action, reason, notes, validation_errors_json, created_at_ms
                    FROM creation_review_audit
                    WHERE creation_id = ?1
                    ORDER BY audit_id ASC
                    "#,
                )
                .map_db()?;
            let items = stmt
                .query_map(params![creation_id], |r| {
                    let from_s: String = r.get(3)?;
                    let to_s: String = r.get(4)?;
                    let errs_s: String = r.get(8)?;
                    Ok(CreationReviewAuditDto {
                        audit_id: r.get(0)?,
                        creation_id: r.get(1)?,
                        actor_user_id: r.get(2)?,
                        from_status: CreationReviewStatus::parse(&from_s)
                            .unwrap_or(CreationReviewStatus::Draft),
                        to_status: CreationReviewStatus::parse(&to_s)
                            .unwrap_or(CreationReviewStatus::Draft),
                        action: r.get(5)?,
                        reason: r.get(6)?,
                        notes: r.get(7)?,
                        validation_errors: serde_json::from_str(&errs_s).unwrap_or_default(),
                        created_at_ms: r.get(9)?,
                    })
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(Ok(CreationAuditListResponse { items }))
        })?
    }
}

fn sync_catalog_metrics_tx(tx: &Transaction<'_>, creation_id: &str) -> Result<()> {
    let metrics: Option<(f64, i64, i64, i64)> = tx
        .query_row(
            "SELECT rating_avg, rating_count, engagement_count, save_or_install_count FROM user_creations WHERE creation_id = ?1",
            params![creation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_db()?;
    if let Some((rating_avg, rating_count, engagement_count, save_count)) = metrics {
        let popularity = compute_popularity_score(
            rating_avg as f32,
            rating_count.max(0) as u32,
            engagement_count.max(0) as u64,
            save_count.max(0) as u64,
        );
        tx.execute(
            r#"
            UPDATE public_catalog_entries
            SET rating_avg = ?1,
                rating_count = ?2,
                engagement_count = ?3,
                save_or_install_count = ?4,
                popularity_score = ?5
            WHERE creation_id = ?6
            "#,
            params![
                rating_avg,
                rating_count,
                engagement_count,
                save_count,
                popularity,
                creation_id,
            ],
        )
        .map_db()?;
    }
    Ok(())
}

impl ServerDatabase {
    pub fn upsert_user_creation_state(
        &self,
        creation_id: &str,
        user_id: UserId,
        req: &UpsertCreationStateRequest,
        now_ms: i64,
    ) -> std::result::Result<UserCreationStateDto, CreationOpError> {
        if let Some(p) = req.progress_percent {
            if !p.is_finite() || !(0.0..=100.0).contains(&p) {
                return Err(CreationOpError::BadRequest {
                    code: "invalid_progress_percent",
                    message: "progress_percent must be a finite number in 0.0..=100.0".to_string(),
                });
            }
        }
        let uid = user_id.0 as i64;
        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            let is_live_accessible = row.has_live_published_version
                && matches!(
                    row.published_visibility,
                    Some(CreationVisibility::Public | CreationVisibility::Unlisted)
                );
            if row.owner_user_id != uid && !is_live_accessible {
                return Ok(Err(CreationOpError::Forbidden(
                    "Creation is not published and cannot store external user state",
                )));
            }

            let existing: Option<(bool, bool, f32, serde_json::Value)> = tx
                .query_row(
                    "SELECT in_library, favorite, progress_percent, state_json FROM user_creation_states WHERE user_id = ?1 AND creation_id = ?2",
                    params![uid, creation_id],
                    |r| {
                        let raw_json: String = r.get(3)?;
                        Ok((
                            r.get::<_, i64>(0)? != 0,
                            r.get::<_, i64>(1)? != 0,
                            r.get::<_, f64>(2)? as f32,
                            serde_json::from_str(&raw_json).unwrap_or_else(|_| serde_json::json!({})),
                        ))
                    },
                )
                .optional()
                .map_db()?;

            let (prev_lib, prev_fav, prev_prog, prev_json) =
                existing.unwrap_or((false, false, 0.0, serde_json::json!({})));
            let in_library = req.in_library.unwrap_or(prev_lib);
            let favorite = req.favorite.unwrap_or(prev_fav);
            let progress_percent = req.progress_percent.unwrap_or(prev_prog);
            let state_json = req.state_json.clone().unwrap_or(prev_json);
            let state_json_str =
                serde_json::to_string(&state_json).unwrap_or_else(|_| "{}".to_string());

            tx.execute(
                r#"
                INSERT INTO user_creation_states (
                    user_id, creation_id, kind, in_library, favorite, progress_percent, state_json, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(user_id, creation_id) DO UPDATE SET
                    kind = excluded.kind,
                    in_library = excluded.in_library,
                    favorite = excluded.favorite,
                    progress_percent = excluded.progress_percent,
                    state_json = excluded.state_json,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![
                    uid,
                    creation_id,
                    row.kind.as_str(),
                    in_library as i64,
                    favorite as i64,
                    progress_percent as f64,
                    state_json_str,
                    now_ms,
                ],
            )
            .map_db()?;

            let lib_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM user_creation_states WHERE creation_id = ?1 AND in_library = 1",
                    params![creation_id],
                    |r| r.get(0),
                )
                .map_db()?;
            tx.execute(
                r#"
                UPDATE user_creations
                SET save_or_install_count = ?1,
                    engagement_count = engagement_count + 1
                WHERE creation_id = ?2
                "#,
                params![lib_count, creation_id],
            )
            .map_db()?;
            sync_catalog_metrics_tx(tx, creation_id)?;

            Ok(Ok(UserCreationStateDto {
                creation_id: creation_id.to_string(),
                user_id: uid,
                kind: row.kind,
                in_library,
                favorite,
                progress_percent,
                state_json,
                updated_at_ms: now_ms,
            }))
        })?;
        outcome
    }

    pub fn list_user_creation_states(
        &self,
        user_id: UserId,
        kind_filter: Option<CreationKind>,
    ) -> Result<UserCreationStatesResponse> {
        let uid = user_id.0 as i64;
        self.pool.with_reader(|conn| {
            let mut sql = String::from(
                "SELECT creation_id, user_id, kind, in_library, favorite, progress_percent, state_json, updated_at_ms FROM user_creation_states WHERE user_id = ?1",
            );
            let mut bind_vals: Vec<rusqlite::types::Value> = vec![uid.into()];
            if let Some(k) = kind_filter {
                sql.push_str(" AND kind = ?2");
                bind_vals.push(k.as_str().to_string().into());
            }
            sql.push_str(" ORDER BY updated_at_ms DESC, creation_id ASC");
            let mut stmt = conn.prepare(&sql).map_db()?;
            let items = stmt
                .query_map(rusqlite::params_from_iter(bind_vals), |r| {
                    let kind_s: String = r.get(2)?;
                    let raw_json: String = r.get(6)?;
                    Ok(UserCreationStateDto {
                        creation_id: r.get(0)?,
                        user_id: r.get(1)?,
                        kind: CreationKind::parse(&kind_s).unwrap_or(CreationKind::Book),
                        in_library: r.get::<_, i64>(3)? != 0,
                        favorite: r.get::<_, i64>(4)? != 0,
                        progress_percent: r.get::<_, f64>(5)? as f32,
                        state_json: serde_json::from_str(&raw_json)
                            .unwrap_or_else(|_| serde_json::json!({})),
                        updated_at_ms: r.get(7)?,
                    })
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(UserCreationStatesResponse { items })
        })
    }
}

const MAX_REVIEW_TITLE_CHARS: usize = 120;
const MAX_REVIEW_BODY_CHARS: usize = 4_000;

fn contains_review_xss_pattern(value: &str) -> bool {
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

impl ServerDatabase {
    pub fn upsert_creation_review(
        &self,
        creation_id: &str,
        user_id: UserId,
        req: &CreateCreationReviewRequest,
        now_ms: i64,
    ) -> std::result::Result<CreationReviewEntryDto, CreationOpError> {
        if !(1..=5).contains(&req.rating) {
            return Err(CreationOpError::BadRequest {
                code: "invalid_rating",
                message: "rating must be an integer between 1 and 5".to_string(),
            });
        }
        if let Some(raw_title) = req.title.as_deref() {
            if contains_review_xss_pattern(raw_title) {
                return Err(CreationOpError::BadRequest {
                    code: "unsafe_review_content",
                    message: "Review title contains disallowed script or unsafe URI scheme"
                        .to_string(),
                });
            }
        }
        if contains_review_xss_pattern(&req.body) {
            return Err(CreationOpError::BadRequest {
                code: "unsafe_review_content",
                message: "Review body contains disallowed script or unsafe URI scheme".to_string(),
            });
        }
        let title_clean = req
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        if let Some(ref title) = title_clean {
            if title.chars().count() > MAX_REVIEW_TITLE_CHARS {
                return Err(CreationOpError::BadRequest {
                    code: "review_title_too_long",
                    message: format!(
                        "Review title must be at most {MAX_REVIEW_TITLE_CHARS} characters"
                    ),
                });
            }
        }
        let body_clean = req.body.trim().to_string();
        if body_clean.is_empty() {
            return Err(CreationOpError::BadRequest {
                code: "empty_review_body",
                message: "Review body must not be empty".to_string(),
            });
        }
        if body_clean.chars().count() > MAX_REVIEW_BODY_CHARS {
            return Err(CreationOpError::BadRequest {
                code: "review_body_too_long",
                message: format!(
                    "Review body must be at most {MAX_REVIEW_BODY_CHARS} characters"
                ),
            });
        }
        let uid = user_id.0 as i64;

        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            let is_live = row.has_live_published_version
                && matches!(
                    row.published_visibility,
                    Some(CreationVisibility::Public | CreationVisibility::Unlisted)
                );
            if !is_live {
                return Ok(Err(CreationOpError::InvalidState {
                    code: "creation_not_published",
                    message: "Only published creations can be reviewed".to_string(),
                }));
            }

            let existed: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM creation_reviews WHERE creation_id = ?1 AND user_id = ?2)",
                    params![creation_id, uid],
                    |r| r.get(0),
                )
                .map_db()?;

            tx.execute(
                r#"
                INSERT INTO creation_reviews (
                    creation_id, user_id, rating, title, body, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                ON CONFLICT(creation_id, user_id) DO UPDATE SET
                    rating = excluded.rating,
                    title = excluded.title,
                    body = excluded.body,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![
                    creation_id,
                    uid,
                    req.rating as i64,
                    title_clean,
                    body_clean,
                    now_ms,
                ],
            )
            .map_db()?;

            let (count, avg): (i64, f64) = tx
                .query_row(
                    "SELECT COUNT(*), COALESCE(AVG(CAST(rating AS REAL)), 0.0) FROM creation_reviews WHERE creation_id = ?1",
                    params![creation_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_db()?;

            let eng_inc: i64 = if existed { 0 } else { 1 };
            tx.execute(
                r#"
                UPDATE user_creations
                SET rating_avg = ?1,
                    rating_count = ?2,
                    engagement_count = engagement_count + ?3
                WHERE creation_id = ?4
                "#,
                params![avg, count, eng_inc, creation_id],
            )
            .map_db()?;
            sync_catalog_metrics_tx(tx, creation_id)?;

            let entry = tx
                .query_row(
                    r#"
                    SELECT r.review_id, r.creation_id, r.user_id, r.rating, r.title, r.body,
                           r.created_at_ms, r.updated_at_ms, u.first_name, u.last_name, u.username
                    FROM creation_reviews r
                    LEFT JOIN users u ON u.user_id = r.user_id
                    WHERE r.creation_id = ?1 AND r.user_id = ?2
                    "#,
                    params![creation_id, uid],
                    |r| {
                        let reviewer_id: i64 = r.get(2)?;
                        Ok(CreationReviewEntryDto {
                            review_id: r.get(0)?,
                            creation_id: r.get(1)?,
                            user_id: reviewer_id,
                            author_display_name: format_user_display_name(
                                r.get(8)?,
                                r.get(9)?,
                                r.get(10)?,
                                reviewer_id,
                            ),
                            rating: r.get::<_, i64>(3)? as u8,
                            title: r.get(4)?,
                            body: r.get(5)?,
                            created_at_ms: r.get(6)?,
                            updated_at_ms: r.get(7)?,
                        })
                    },
                )
                .map_db()?;
            Ok(Ok(entry))
        })?;
        outcome
    }

    pub fn list_creation_reviews(
        &self,
        creation_id: &str,
        limit: usize,
        offset: usize,
    ) -> std::result::Result<CreationReviewsResponse, CreationOpError> {
        let clamped_limit = limit.min(200) as i64;
        let offset_i64 = offset as i64;
        self.pool.with_reader(|conn| {
            let meta_opt: Option<(bool, Option<String>, Option<i64>, f64, i64)> = conn
                .query_row(
                    "SELECT has_live_published_version, published_visibility, deleted_at_ms, rating_avg, rating_count FROM user_creations WHERE creation_id = ?1",
                    params![creation_id],
                    |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()
                .map_db()?;
            let Some((has_live, pub_vis_s, deleted_at, rating_avg, rating_count)) = meta_opt else {
                return Ok(Err(CreationOpError::NotFound));
            };
            let vis = pub_vis_s.as_deref().and_then(CreationVisibility::parse);
            if deleted_at.is_some()
                || !has_live
                || !matches!(
                    vis,
                    Some(CreationVisibility::Public | CreationVisibility::Unlisted)
                )
            {
                return Ok(Err(CreationOpError::NotFound));
            }

            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT r.review_id, r.creation_id, r.user_id, r.rating, r.title, r.body,
                           r.created_at_ms, r.updated_at_ms, u.first_name, u.last_name, u.username
                    FROM creation_reviews r
                    LEFT JOIN users u ON u.user_id = r.user_id
                    WHERE r.creation_id = ?1
                    ORDER BY r.updated_at_ms DESC, r.review_id DESC
                    LIMIT ?2 OFFSET ?3
                    "#,
                )
                .map_db()?;
            let items = stmt
                .query_map(params![creation_id, clamped_limit, offset_i64], |r| {
                    let reviewer_id: i64 = r.get(2)?;
                    Ok(CreationReviewEntryDto {
                        review_id: r.get(0)?,
                        creation_id: r.get(1)?,
                        user_id: reviewer_id,
                        author_display_name: format_user_display_name(
                            r.get(8)?,
                            r.get(9)?,
                            r.get(10)?,
                            reviewer_id,
                        ),
                        rating: r.get::<_, i64>(3)? as u8,
                        title: r.get(4)?,
                        body: r.get(5)?,
                        created_at_ms: r.get(6)?,
                        updated_at_ms: r.get(7)?,
                    })
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            Ok(Ok(CreationReviewsResponse {
                items,
                rating_avg: rating_avg as f32,
                rating_count: rating_count.max(0) as u32,
            }))
        })?
    }
}

impl ServerDatabase {
    pub fn submit_creation_game_score(
        &self,
        creation_id: &str,
        user_id: UserId,
        req: &SubmitGameScoreRequest,
        now_ms: i64,
    ) -> std::result::Result<GameLeaderboardEntryDto, CreationOpError> {
        let uid = user_id.0 as i64;
        let incoming_meta_str = req
            .metadata
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());

        let outcome = self.pool.with_transaction(|tx| {
            let Some(row) = fetch_creation_row_tx(tx, creation_id)? else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if row.deleted_at_ms.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            let is_live = row.has_live_published_version
                && matches!(
                    row.published_visibility,
                    Some(CreationVisibility::Public | CreationVisibility::Unlisted)
                );
            if !is_live {
                return Ok(Err(CreationOpError::InvalidState {
                    code: "creation_not_published",
                    message: "Scores can only be submitted for published games".to_string(),
                }));
            }
            if row.kind != CreationKind::Game {
                return Ok(Err(CreationOpError::BadRequest {
                    code: "not_a_game",
                    message: "Scores can only be submitted for game creations".to_string(),
                }));
            }

            let existing: Option<(i64, Option<String>, i64)> = tx
                .query_row(
                    "SELECT best_score, metadata_json, achieved_at_ms FROM creation_game_scores WHERE creation_id = ?1 AND user_id = ?2",
                    params![creation_id, uid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_db()?;

            let (final_score, final_meta_str, final_achieved_at) = match existing {
                None => {
                    tx.execute(
                        "INSERT INTO creation_game_scores (creation_id, user_id, best_score, metadata_json, achieved_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![creation_id, uid, req.score, incoming_meta_str, now_ms],
                    )
                    .map_db()?;
                    (req.score, incoming_meta_str.clone(), now_ms)
                }
                Some((prev_score, prev_meta, prev_at)) => {
                    if req.score > prev_score {
                        tx.execute(
                            "UPDATE creation_game_scores SET best_score = ?1, metadata_json = ?2, achieved_at_ms = ?3 WHERE creation_id = ?4 AND user_id = ?5",
                            params![req.score, incoming_meta_str, now_ms, creation_id, uid],
                        )
                        .map_db()?;
                        (req.score, incoming_meta_str.clone(), now_ms)
                    } else if req.score == prev_score && incoming_meta_str.is_some() {
                        tx.execute(
                            "UPDATE creation_game_scores SET metadata_json = ?1 WHERE creation_id = ?2 AND user_id = ?3",
                            params![incoming_meta_str, creation_id, uid],
                        )
                        .map_db()?;
                        (prev_score, incoming_meta_str.clone(), prev_at)
                    } else {
                        (prev_score, prev_meta, prev_at)
                    }
                }
            };

            tx.execute(
                "UPDATE user_creations SET engagement_count = engagement_count + 1 WHERE creation_id = ?1",
                params![creation_id],
            )
            .map_db()?;
            sync_catalog_metrics_tx(tx, creation_id)?;

            let rank: i64 = tx
                .query_row(
                    r#"
                    SELECT 1 + COUNT(*)
                    FROM creation_game_scores
                    WHERE creation_id = ?1
                      AND (
                          best_score > ?2
                          OR (best_score = ?2 AND achieved_at_ms < ?3)
                          OR (best_score = ?2 AND achieved_at_ms = ?3 AND user_id < ?4)
                      )
                    "#,
                    params![creation_id, final_score, final_achieved_at, uid],
                    |r| r.get(0),
                )
                .map_db()?;

            let user_names: Option<(Option<String>, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT first_name, last_name, username FROM users WHERE user_id = ?1",
                    params![uid],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_db()?;
            let display_name = match user_names {
                Some((f, l, u)) => format_user_display_name(f, l, u, uid),
                None => format!("User {uid}"),
            };

            Ok(Ok(GameLeaderboardEntryDto {
                rank: rank.max(1) as u32,
                creation_id: creation_id.to_string(),
                user_id: uid,
                display_name,
                score: final_score,
                metadata: final_meta_str.and_then(|s| serde_json::from_str(&s).ok()),
                achieved_at_ms: final_achieved_at,
            }))
        })?;
        outcome
    }

    pub fn get_creation_game_leaderboard(
        &self,
        creation_id: &str,
        limit: usize,
    ) -> std::result::Result<GameLeaderboardResponse, CreationOpError> {
        let clamped_limit = limit.min(200) as i64;
        self.pool.with_reader(|conn| {
            let meta_opt: Option<(String, bool, Option<String>, Option<i64>)> = conn
                .query_row(
                    "SELECT kind, has_live_published_version, published_visibility, deleted_at_ms FROM user_creations WHERE creation_id = ?1",
                    params![creation_id],
                    |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_db()?;
            let Some((kind_s, has_live, pub_vis_s, deleted_at)) = meta_opt else {
                return Ok(Err(CreationOpError::NotFound));
            };
            if deleted_at.is_some() {
                return Ok(Err(CreationOpError::NotFound));
            }
            let vis = pub_vis_s.as_deref().and_then(CreationVisibility::parse);
            if !has_live
                || !matches!(
                    vis,
                    Some(CreationVisibility::Public | CreationVisibility::Unlisted)
                )
            {
                return Ok(Err(CreationOpError::NotFound));
            }
            if CreationKind::parse(&kind_s) != Some(CreationKind::Game) {
                return Ok(Err(CreationOpError::BadRequest {
                    code: "not_a_game",
                    message: "Leaderboards are only available for game creations".to_string(),
                }));
            }

            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT s.creation_id, s.user_id, s.best_score, s.metadata_json, s.achieved_at_ms,
                           u.first_name, u.last_name, u.username
                    FROM creation_game_scores s
                    LEFT JOIN users u ON u.user_id = s.user_id
                    WHERE s.creation_id = ?1
                    ORDER BY s.best_score DESC, s.achieved_at_ms ASC, s.user_id ASC
                    LIMIT ?2
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![creation_id, clamped_limit], |r| {
                    let uid: i64 = r.get(1)?;
                    let meta_str: Option<String> = r.get(3)?;
                    Ok(GameLeaderboardEntryDto {
                        rank: 0,
                        creation_id: r.get(0)?,
                        user_id: uid,
                        display_name: format_user_display_name(
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                            uid,
                        ),
                        score: r.get(2)?,
                        metadata: meta_str.and_then(|s| serde_json::from_str(&s).ok()),
                        achieved_at_ms: r.get(4)?,
                    })
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            let items = rows
                .into_iter()
                .enumerate()
                .map(|(idx, mut entry)| {
                    entry.rank = (idx + 1) as u32;
                    entry
                })
                .collect();
            Ok(Ok(GameLeaderboardResponse { items }))
        })?
    }
}
