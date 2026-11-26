use std::collections::{BTreeSet, HashSet};

use crate::db::{ServerDatabase, SqliteResultExt};
use chrono::Utc;
use gpui_telegram_code_scene::{
    official_seed_assets, UserInterestProfile, OFFICIAL_CODE_SCENE_BOTS, OFFICIAL_FABUSHI_BOT_ID,
};
use gpui_telegram_core::error::Result;
use gpui_telegram_proto::code_scene_contracts::{
    AssetKind, AssetManifest, CodeSceneAssetDto, CodeSceneCommentDto, CodeSceneCommentSort,
    ListCodeSceneCommentsResponse, SceneBehaviorEventDto, SceneBehaviorEventKind,
    ToggleBookmarkResponse, ToggleFollowResponse, ToggleLikeResponse, ValidationReportDto,
    CODE_SCENE_FORMAT_CS1, CODE_SCENE_SCHEMA_VERSION_V1,
};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

pub const MIGRATION_V42_CODE_SCENE: &str = r#"
CREATE TABLE IF NOT EXISTS code_scenes (
    scene_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL CHECK (kind IN ('video', 'animation', 'game')),
    tags_json TEXT NOT NULL DEFAULT '[]',
    author_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    author_username TEXT NOT NULL DEFAULT '',
    creator_bot_id INTEGER,
    creator_bot_username TEXT,
    remix_parent_id TEXT REFERENCES code_scenes(scene_id) ON DELETE SET NULL,
    source_code TEXT NOT NULL,
    code_format TEXT NOT NULL DEFAULT 'cs1',
    manifest_json TEXT NOT NULL,
    controls_hint TEXT NOT NULL DEFAULT '',
    cover_hash TEXT NOT NULL DEFAULT '',
    feature_tokens_json TEXT NOT NULL DEFAULT '[]',
    latest_version INTEGER NOT NULL DEFAULT 1,
    plays_count INTEGER NOT NULL DEFAULT 0,
    likes_count INTEGER NOT NULL DEFAULT 0,
    bookmarks_count INTEGER NOT NULL DEFAULT 0,
    comments_count INTEGER NOT NULL DEFAULT 0,
    shares_count INTEGER NOT NULL DEFAULT 0,
    remixes_count INTEGER NOT NULL DEFAULT 0,
    is_published INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scenes_kind_created ON code_scenes(is_published, kind, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_code_scenes_author ON code_scenes(author_user_id, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_code_scenes_bot ON code_scenes(creator_bot_id, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_code_scenes_remix_parent ON code_scenes(remix_parent_id);

CREATE TABLE IF NOT EXISTS code_scene_versions (
    scene_id TEXT NOT NULL REFERENCES code_scenes(scene_id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    source_code TEXT NOT NULL,
    code_format TEXT NOT NULL DEFAULT 'cs1',
    manifest_json TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    cover_hash TEXT NOT NULL DEFAULT '',
    created_by_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (scene_id, version)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scene_versions_created ON code_scene_versions(scene_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS code_scene_interactions (
    interaction_id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    session_id TEXT NOT NULL DEFAULT '',
    scene_id TEXT NOT NULL DEFAULT '',
    event_kind TEXT NOT NULL,
    asset_kind TEXT,
    author_user_id INTEGER,
    tags_json TEXT NOT NULL DEFAULT '[]',
    dwell_ms INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    completion_ratio REAL NOT NULL DEFAULT 0.0,
    loop_count INTEGER NOT NULL DEFAULT 0,
    game_inputs INTEGER NOT NULL DEFAULT 0,
    game_play_ms INTEGER NOT NULL DEFAULT 0,
    game_restarts INTEGER NOT NULL DEFAULT 0,
    game_score REAL,
    reason TEXT,
    reward REAL NOT NULL DEFAULT 0.0,
    timestamp_ms INTEGER NOT NULL,
    recorded_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scene_interactions_user_ts ON code_scene_interactions(user_id, timestamp_ms DESC);
CREATE INDEX IF NOT EXISTS idx_code_scene_interactions_scene_ts ON code_scene_interactions(scene_id, timestamp_ms DESC);
CREATE INDEX IF NOT EXISTS idx_code_scene_interactions_event_id ON code_scene_interactions(user_id, event_id);

CREATE TABLE IF NOT EXISTS code_scene_likes (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    scene_id TEXT NOT NULL REFERENCES code_scenes(scene_id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, scene_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scene_likes_scene ON code_scene_likes(scene_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS code_scene_bookmarks (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    scene_id TEXT NOT NULL REFERENCES code_scenes(scene_id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, scene_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scene_bookmarks_scene ON code_scene_bookmarks(scene_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS code_scene_comments (
    comment_id TEXT PRIMARY KEY,
    scene_id TEXT NOT NULL REFERENCES code_scenes(scene_id) ON DELETE CASCADE,
    author_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    author_username TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    parent_comment_id TEXT REFERENCES code_scene_comments(comment_id) ON DELETE SET NULL,
    likes_count INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scene_comments_scene_hot ON code_scene_comments(scene_id, likes_count DESC, created_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_code_scene_comments_scene_new ON code_scene_comments(scene_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS code_scene_follows (
    follower_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    creator_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (follower_user_id, creator_user_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scene_follows_creator ON code_scene_follows(creator_user_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS code_scene_user_profiles (
    user_id INTEGER PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL DEFAULT 1,
    profile_json TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS code_scene_bot_drafts (
    draft_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    bot_id INTEGER,
    bot_username TEXT,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL CHECK (kind IN ('video', 'animation', 'game')),
    tags_json TEXT NOT NULL DEFAULT '[]',
    source_code TEXT NOT NULL,
    code_format TEXT NOT NULL DEFAULT 'cs1',
    manifest_json TEXT NOT NULL,
    controls_hint TEXT NOT NULL DEFAULT '',
    validation_json TEXT NOT NULL,
    engine_used TEXT NOT NULL DEFAULT '',
    repair_attempts INTEGER NOT NULL DEFAULT 0,
    remix_parent_id TEXT REFERENCES code_scenes(scene_id) ON DELETE SET NULL,
    prompt_history_json TEXT NOT NULL DEFAULT '[]',
    published_scene_id TEXT REFERENCES code_scenes(scene_id) ON DELETE SET NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_code_scene_bot_drafts_user ON code_scene_bot_drafts(user_id, updated_at_ms DESC);
"#;

#[derive(Debug, Clone)]
pub struct CodeSceneBotDraftRecord {
    pub draft_id: String,
    pub user_id: i64,
    pub bot_id: Option<i64>,
    pub bot_username: Option<String>,
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
    pub remix_parent_id: Option<String>,
    pub prompt_history: Vec<String>,
    pub published_scene_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone)]
pub enum UpsertCodeSceneOutcome {
    Created {
        asset: CodeSceneAssetDto,
        version: i64,
        parent_remixes_count: Option<u64>,
    },
    Updated {
        asset: CodeSceneAssetDto,
        version: i64,
    },
    ForbiddenOwner,
    ParentNotFound(String),
}

#[derive(Debug, Clone)]
pub enum CreateCodeSceneCommentOutcome {
    Created {
        comment: CodeSceneCommentDto,
        comments_count: u64,
    },
    SceneNotFound,
    ParentCommentNotFound,
}

fn decode_code_scene_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodeSceneAssetDto> {
    let scene_id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let description: String = row.get(2)?;
    let kind_str: String = row.get(3)?;
    let tags_json: String = row.get(4)?;
    let author_user_id: i64 = row.get(5)?;
    let stored_author_username: String = row.get(6)?;
    let creator_bot_id: Option<i64> = row.get(7)?;
    let creator_bot_username: Option<String> = row.get(8)?;
    let remix_parent_id: Option<String> = row.get(9)?;
    let source_code: String = row.get(10)?;
    let code_format: String = row.get(11)?;
    let manifest_json: String = row.get(12)?;
    let controls_hint: String = row.get(13)?;
    let plays_count: i64 = row.get(14)?;
    let likes_count: i64 = row.get(15)?;
    let bookmarks_count: i64 = row.get(16)?;
    let comments_count: i64 = row.get(17)?;
    let shares_count: i64 = row.get(18)?;
    let remixes_count: i64 = row.get(19)?;
    let created_at_ms: i64 = row.get(20)?;
    let updated_at_ms: i64 = row.get(21)?;
    let joined_username: Option<String> = row.get(22)?;

    let kind = AssetKind::parse(&kind_str).unwrap_or(AssetKind::Animation);
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    let mut manifest: AssetManifest =
        serde_json::from_str(&manifest_json).unwrap_or_else(|_| AssetManifest::for_kind(kind));
    manifest.kind = kind;
    manifest.clamp_in_place();

    let author_username = if !stored_author_username.trim().is_empty() {
        stored_author_username
    } else {
        joined_username
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("user_{author_user_id}"))
    };

    Ok(CodeSceneAssetDto {
        scene_id,
        title,
        description,
        kind,
        tags,
        author_user_id,
        author_username,
        creator_bot_id,
        creator_bot_username,
        remix_parent_id,
        source_code,
        code_format: if code_format.trim().is_empty() {
            CODE_SCENE_FORMAT_CS1.to_string()
        } else {
            code_format
        },
        manifest,
        controls_hint,
        plays_count: plays_count.max(0) as u64,
        likes_count: likes_count.max(0) as u64,
        bookmarks_count: bookmarks_count.max(0) as u64,
        comments_count: comments_count.max(0) as u64,
        shares_count: shares_count.max(0) as u64,
        remixes_count: remixes_count.max(0) as u64,
        viewer_liked: false,
        viewer_bookmarked: false,
        viewer_following_author: false,
        created_at_ms,
        updated_at_ms,
    })
}

const SELECT_CODE_SCENE_COLS: &str = r#"
    SELECT cs.scene_id, cs.title, cs.description, cs.kind, cs.tags_json,
           cs.author_user_id, cs.author_username, cs.creator_bot_id, cs.creator_bot_username,
           cs.remix_parent_id, cs.source_code, cs.code_format, cs.manifest_json,
           cs.controls_hint, cs.plays_count, cs.likes_count, cs.bookmarks_count,
           cs.comments_count, cs.shares_count, cs.remixes_count,
           cs.created_at_ms, cs.updated_at_ms, u.username
    FROM code_scenes cs
    LEFT JOIN users u ON u.user_id = cs.author_user_id
"#;

impl ServerDatabase {
    /// Idempotent FK-safe bootstrap: seeds all 5 `OFFICIAL_CODE_SCENE_BOTS` into `users` &
    /// `bots_registry` (`777_101..=777_104` as `is_standard_bot = 1`; `777_000` managed by
    /// `hydrate_all_fabushi_bots`) and all 18 `official_seed_assets()` into `code_scenes` &
    /// `code_scene_versions` using `INSERT OR IGNORE`.
    pub fn seed_official_code_scene_bots_and_assets(&self) -> Result<usize> {
        let now_ms = Utc::now().timestamp_millis();
        let now_sec = Utc::now().timestamp();
        let seeds = official_seed_assets();
        self.pool.with_transaction(|tx| {
            for bot in OFFICIAL_CODE_SCENE_BOTS {
                tx.execute(
                    r#"
                    INSERT OR IGNORE INTO users (
                        user_id, username, phone, first_name, last_name, bio,
                        password_hash, is_bot, is_verified, is_support, is_scam, is_fake,
                        profile_photo_file_id, account_generation, created_at_ms, updated_at_ms
                    ) VALUES (?1, ?2, NULL, ?3, NULL, ?4, NULL, 1, 1, 0, 0, 0, NULL, 1, ?5, ?5)
                    "#,
                    params![bot.user_id, bot.username, bot.display_name, bot.bio, now_ms],
                )
                .map_db()?;

                if bot.user_id != OFFICIAL_FABUSHI_BOT_ID {
                    let token = format!("{}:AAOfficialCodeSceneBotV1", bot.user_id);
                    let agent_id = bot.username.replace('_', "-");
                    tx.execute(
                        r#"
                        INSERT OR IGNORE INTO bots_registry (
                            bot_user_id, owner_user_id, username, display_name, bot_token,
                            description, about, ai_persona, is_official, is_standard_bot,
                            agent_id, avatar_shape, avatar_color, created_at, updated_at
                        ) VALUES (?1, 0, ?2, ?3, ?4, ?5, ?5, ?5, 1, 1, ?6, 'squircle', 'cyan', ?7, ?7)
                        "#,
                        params![
                            bot.user_id,
                            bot.username,
                            bot.display_name,
                            token,
                            bot.bio,
                            agent_id,
                            now_sec,
                        ],
                    )
                    .map_db()?;
                }
            }

            let mut inserted = 0usize;
            for asset in &seeds {
                let tags_json = serde_json::to_string(&asset.tags).unwrap_or_else(|_| "[]".into());
                let manifest_json =
                    serde_json::to_string(&asset.manifest).unwrap_or_else(|_| "{}".into());
                let rows = tx
                    .execute(
                        r#"
                        INSERT OR IGNORE INTO code_scenes (
                            scene_id, title, description, kind, tags_json,
                            author_user_id, author_username, creator_bot_id, creator_bot_username,
                            remix_parent_id, source_code, code_format, manifest_json,
                            controls_hint, cover_hash, feature_tokens_json, latest_version,
                            plays_count, likes_count, bookmarks_count, comments_count,
                            shares_count, remixes_count, is_published, created_at_ms, updated_at_ms
                        ) VALUES (
                            ?1, ?2, ?3, ?4, ?5,
                            ?6, ?7, ?8, ?9,
                            ?10, ?11, ?12, ?13,
                            ?14, '', '[]', 1,
                            ?15, ?16, ?17, ?18,
                            ?19, ?20, 1, ?21, ?22
                        )
                        "#,
                        params![
                            asset.scene_id,
                            asset.title,
                            asset.description,
                            asset.kind.as_str(),
                            tags_json,
                            asset.author_user_id,
                            asset.author_username,
                            asset.creator_bot_id,
                            asset.creator_bot_username,
                            asset.remix_parent_id,
                            asset.source_code,
                            asset.code_format,
                            manifest_json,
                            asset.controls_hint,
                            asset.plays_count as i64,
                            asset.likes_count as i64,
                            asset.bookmarks_count as i64,
                            asset.comments_count as i64,
                            asset.shares_count as i64,
                            asset.remixes_count as i64,
                            asset.created_at_ms,
                            asset.updated_at_ms,
                        ],
                    )
                    .map_db()?;
                if rows > 0 {
                    inserted += 1;
                }

                let sha = asset.content_sha256();
                tx.execute(
                    r#"
                    INSERT OR IGNORE INTO code_scene_versions (
                        scene_id, version, source_code, code_format, manifest_json,
                        content_sha256, cover_hash, created_by_user_id, created_at_ms
                    ) VALUES (?1, 1, ?2, ?3, ?4, ?5, '', ?6, ?7)
                    "#,
                    params![
                        asset.scene_id,
                        asset.source_code,
                        asset.code_format,
                        manifest_json,
                        sha,
                        asset.author_user_id,
                        asset.created_at_ms,
                    ],
                )
                .map_db()?;
            }
            Ok(inserted)
        })
    }

    /// Loads all published `code_scenes` joined with `users.username` for startup hydration.
    pub fn list_all_published_code_scenes(&self) -> Result<Vec<CodeSceneAssetDto>> {
        self.pool.with_reader(|conn| {
            let sql = format!(
                "{SELECT_CODE_SCENE_COLS} WHERE cs.is_published = 1 ORDER BY cs.created_at_ms DESC, cs.scene_id ASC"
            );
            let mut stmt = conn.prepare(&sql).map_db()?;
            let rows = stmt
                .query_map([], decode_code_scene_row)
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            Ok(rows)
        })
    }

    pub fn get_code_scene(&self, scene_id: &str) -> Result<Option<CodeSceneAssetDto>> {
        self.pool.with_reader(|conn| {
            let sql = format!("{SELECT_CODE_SCENE_COLS} WHERE cs.scene_id = ?1");
            conn.query_row(&sql, [scene_id], decode_code_scene_row)
                .optional()
                .map_db()
        })
    }

    /// Atomically creates or updates a scene + inserts a `code_scene_versions` row
    /// (+ increments parent `remixes_count` when creating a new remix, and marks optional draft published).
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_code_scene(
        &self,
        asset: &CodeSceneAssetDto,
        cover_hash: &str,
        feature_tokens: &[String],
        caller_user_id: i64,
        mark_draft_id: Option<&str>,
    ) -> Result<UpsertCodeSceneOutcome> {
        let now_ms = Utc::now().timestamp_millis();
        let tags_json = serde_json::to_string(&asset.tags).unwrap_or_else(|_| "[]".into());
        let manifest_json = serde_json::to_string(&asset.manifest).unwrap_or_else(|_| "{}".into());
        let tokens_json = serde_json::to_string(feature_tokens).unwrap_or_else(|_| "[]".into());
        let sha = asset.content_sha256();

        self.pool.with_transaction(|tx| {
            let existing: Option<(i64, i64, i64)> = tx
                .query_row(
                    "SELECT author_user_id, latest_version, created_at_ms FROM code_scenes WHERE scene_id = ?1",
                    [&asset.scene_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_db()?;

            if let Some((owner_uid, latest_ver, _created_at)) = existing {
                if owner_uid != caller_user_id {
                    return Ok(UpsertCodeSceneOutcome::ForbiddenOwner);
                }
                let next_ver = latest_ver + 1;
                tx.execute(
                    r#"
                    UPDATE code_scenes
                    SET title = ?2,
                        description = ?3,
                        kind = ?4,
                        tags_json = ?5,
                        author_username = ?6,
                        creator_bot_id = COALESCE(?7, creator_bot_id),
                        creator_bot_username = COALESCE(?8, creator_bot_username),
                        source_code = ?9,
                        code_format = ?10,
                        manifest_json = ?11,
                        controls_hint = ?12,
                        cover_hash = ?13,
                        feature_tokens_json = ?14,
                        latest_version = ?15,
                        is_published = 1,
                        updated_at_ms = ?16
                    WHERE scene_id = ?1
                    "#,
                    params![
                        asset.scene_id,
                        asset.title,
                        asset.description,
                        asset.kind.as_str(),
                        tags_json,
                        asset.author_username,
                        asset.creator_bot_id,
                        asset.creator_bot_username,
                        asset.source_code,
                        asset.code_format,
                        manifest_json,
                        asset.controls_hint,
                        cover_hash,
                        tokens_json,
                        next_ver,
                        now_ms,
                    ],
                )
                .map_db()?;

                tx.execute(
                    r#"
                    INSERT INTO code_scene_versions (
                        scene_id, version, source_code, code_format, manifest_json,
                        content_sha256, cover_hash, created_by_user_id, created_at_ms
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                    "#,
                    params![
                        asset.scene_id,
                        next_ver,
                        asset.source_code,
                        asset.code_format,
                        manifest_json,
                        sha,
                        cover_hash,
                        caller_user_id,
                        now_ms,
                    ],
                )
                .map_db()?;

                if let Some(did) = mark_draft_id {
                    tx.execute(
                        "UPDATE code_scene_bot_drafts SET published_scene_id = ?2, updated_at_ms = ?3 WHERE draft_id = ?1 AND user_id = ?4",
                        params![did, asset.scene_id, now_ms, caller_user_id],
                    )
                    .map_db()?;
                }

                let sql = format!("{SELECT_CODE_SCENE_COLS} WHERE cs.scene_id = ?1");
                let updated_asset = tx
                    .query_row(&sql, [&asset.scene_id], decode_code_scene_row)
                    .map_db()?;
                return Ok(UpsertCodeSceneOutcome::Updated {
                    asset: updated_asset,
                    version: next_ver,
                });
            }

            // New scene creation: validate remix_parent_id if supplied
            let mut parent_remixes_count = None;
            if let Some(ref parent_id) = asset.remix_parent_id {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM code_scenes WHERE scene_id = ?1)",
                        [parent_id],
                        |r| r.get(0),
                    )
                    .map_db()?;
                if !exists {
                    return Ok(UpsertCodeSceneOutcome::ParentNotFound(parent_id.clone()));
                }
            }

            let created_ts = if asset.created_at_ms > 0 {
                asset.created_at_ms
            } else {
                now_ms
            };

            tx.execute(
                r#"
                INSERT INTO code_scenes (
                    scene_id, title, description, kind, tags_json,
                    author_user_id, author_username, creator_bot_id, creator_bot_username,
                    remix_parent_id, source_code, code_format, manifest_json,
                    controls_hint, cover_hash, feature_tokens_json, latest_version,
                    plays_count, likes_count, bookmarks_count, comments_count,
                    shares_count, remixes_count, is_published, created_at_ms, updated_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5,
                    ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, 1,
                    0, 0, 0, 0,
                    0, 0, 1, ?17, ?18
                )
                "#,
                params![
                    asset.scene_id,
                    asset.title,
                    asset.description,
                    asset.kind.as_str(),
                    tags_json,
                    caller_user_id,
                    asset.author_username,
                    asset.creator_bot_id,
                    asset.creator_bot_username,
                    asset.remix_parent_id,
                    asset.source_code,
                    asset.code_format,
                    manifest_json,
                    asset.controls_hint,
                    cover_hash,
                    tokens_json,
                    created_ts,
                    now_ms,
                ],
            )
            .map_db()?;

            tx.execute(
                r#"
                INSERT INTO code_scene_versions (
                    scene_id, version, source_code, code_format, manifest_json,
                    content_sha256, cover_hash, created_by_user_id, created_at_ms
                ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
                params![
                    asset.scene_id,
                    asset.source_code,
                    asset.code_format,
                    manifest_json,
                    sha,
                    cover_hash,
                    caller_user_id,
                    now_ms,
                ],
            )
            .map_db()?;

            if let Some(ref parent_id) = asset.remix_parent_id {
                tx.execute(
                    "UPDATE code_scenes SET remixes_count = remixes_count + 1, updated_at_ms = ?2 WHERE scene_id = ?1",
                    params![parent_id, now_ms],
                )
                .map_db()?;
                let cnt: i64 = tx
                    .query_row(
                        "SELECT remixes_count FROM code_scenes WHERE scene_id = ?1",
                        [parent_id],
                        |r| r.get(0),
                    )
                    .map_db()?;
                parent_remixes_count = Some(cnt.max(0) as u64);
            }

            if let Some(did) = mark_draft_id {
                tx.execute(
                    "UPDATE code_scene_bot_drafts SET published_scene_id = ?2, updated_at_ms = ?3 WHERE draft_id = ?1 AND user_id = ?4",
                    params![did, asset.scene_id, now_ms, caller_user_id],
                )
                .map_db()?;
            }

            let sql = format!("{SELECT_CODE_SCENE_COLS} WHERE cs.scene_id = ?1");
            let saved = tx
                .query_row(&sql, [&asset.scene_id], decode_code_scene_row)
                .map_db()?;
            Ok(UpsertCodeSceneOutcome::Created {
                asset: saved,
                version: 1,
                parent_remixes_count,
            })
        })
    }

    /// Atomically toggles like & increments/decrements `code_scenes.likes_count`
    /// without clobbering baseline seed counts.
    pub fn toggle_code_scene_like(
        &self,
        user_id: i64,
        scene_id: &str,
    ) -> Result<Option<ToggleLikeResponse>> {
        let now_ms = Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM code_scenes WHERE scene_id = ?1)",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;
            if !exists {
                return Ok(None);
            }
            let deleted = tx
                .execute(
                    "DELETE FROM code_scene_likes WHERE user_id = ?1 AND scene_id = ?2",
                    params![user_id, scene_id],
                )
                .map_db()?;
            let liked = if deleted > 0 {
                tx.execute(
                    "UPDATE code_scenes SET likes_count = MAX(0, likes_count - 1), updated_at_ms = ?2 WHERE scene_id = ?1",
                    params![scene_id, now_ms],
                )
                .map_db()?;
                false
            } else {
                tx.execute(
                    "INSERT INTO code_scene_likes (user_id, scene_id, created_at_ms) VALUES (?1, ?2, ?3)",
                    params![user_id, scene_id, now_ms],
                )
                .map_db()?;
                tx.execute(
                    "UPDATE code_scenes SET likes_count = likes_count + 1, updated_at_ms = ?2 WHERE scene_id = ?1",
                    params![scene_id, now_ms],
                )
                .map_db()?;
                true
            };
            let count: i64 = tx
                .query_row(
                    "SELECT likes_count FROM code_scenes WHERE scene_id = ?1",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;
            Ok(Some(ToggleLikeResponse {
                liked,
                likes_count: count.max(0) as u64,
            }))
        })
    }

    /// Atomically toggles bookmark & increments/decrements `code_scenes.bookmarks_count`.
    pub fn toggle_code_scene_bookmark(
        &self,
        user_id: i64,
        scene_id: &str,
    ) -> Result<Option<ToggleBookmarkResponse>> {
        let now_ms = Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM code_scenes WHERE scene_id = ?1)",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;
            if !exists {
                return Ok(None);
            }
            let deleted = tx
                .execute(
                    "DELETE FROM code_scene_bookmarks WHERE user_id = ?1 AND scene_id = ?2",
                    params![user_id, scene_id],
                )
                .map_db()?;
            let bookmarked = if deleted > 0 {
                tx.execute(
                    "UPDATE code_scenes SET bookmarks_count = MAX(0, bookmarks_count - 1), updated_at_ms = ?2 WHERE scene_id = ?1",
                    params![scene_id, now_ms],
                )
                .map_db()?;
                false
            } else {
                tx.execute(
                    "INSERT INTO code_scene_bookmarks (user_id, scene_id, created_at_ms) VALUES (?1, ?2, ?3)",
                    params![user_id, scene_id, now_ms],
                )
                .map_db()?;
                tx.execute(
                    "UPDATE code_scenes SET bookmarks_count = bookmarks_count + 1, updated_at_ms = ?2 WHERE scene_id = ?1",
                    params![scene_id, now_ms],
                )
                .map_db()?;
                true
            };
            let count: i64 = tx
                .query_row(
                    "SELECT bookmarks_count FROM code_scenes WHERE scene_id = ?1",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;
            Ok(Some(ToggleBookmarkResponse {
                bookmarked,
                bookmarks_count: count.max(0) as u64,
            }))
        })
    }

    /// Atomically toggles follow in `code_scene_follows`. Returns `Ok(None)` if creator doesn't exist in `users`.
    pub fn toggle_code_scene_follow(
        &self,
        follower_user_id: i64,
        creator_user_id: i64,
    ) -> Result<Option<ToggleFollowResponse>> {
        let now_ms = Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let creator_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE user_id = ?1)",
                    [creator_user_id],
                    |r| r.get(0),
                )
                .map_db()?;
            if !creator_exists {
                return Ok(None);
            }
            let deleted = tx
                .execute(
                    "DELETE FROM code_scene_follows WHERE follower_user_id = ?1 AND creator_user_id = ?2",
                    params![follower_user_id, creator_user_id],
                )
                .map_db()?;
            let following = if deleted > 0 {
                false
            } else {
                tx.execute(
                    "INSERT INTO code_scene_follows (follower_user_id, creator_user_id, created_at_ms) VALUES (?1, ?2, ?3)",
                    params![follower_user_id, creator_user_id, now_ms],
                )
                .map_db()?;
                true
            };
            let followers_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM code_scene_follows WHERE creator_user_id = ?1",
                    [creator_user_id],
                    |r| r.get(0),
                )
                .map_db()?;
            Ok(Some(ToggleFollowResponse {
                following,
                followers_count: followers_count.max(0) as u64,
            }))
        })
    }

    /// Lists comments ordered by `Hot` or `Newest`. Returns `Ok(None)` if `scene_id` missing.
    pub fn list_code_scene_comments(
        &self,
        scene_id: &str,
        sort: CodeSceneCommentSort,
        limit: usize,
        offset: usize,
    ) -> Result<Option<ListCodeSceneCommentsResponse>> {
        let clamped_limit = limit.clamp(1, 200) as i64;
        let offset_i64 = i64::try_from(offset).unwrap_or(i64::MAX);
        self.pool.with_reader(|conn| {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM code_scenes WHERE scene_id = ?1)",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;
            if !exists {
                return Ok(None);
            }
            let total: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM code_scene_comments WHERE scene_id = ?1",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;
            let order_by = match sort {
                CodeSceneCommentSort::Hot => "likes_count DESC, created_at_ms DESC, rowid DESC",
                CodeSceneCommentSort::Newest => "created_at_ms DESC, rowid DESC",
            };
            let sql = format!(
                "SELECT comment_id, scene_id, author_user_id, author_username, content, parent_comment_id, likes_count, created_at_ms \
                 FROM code_scene_comments WHERE scene_id = ?1 ORDER BY {order_by} LIMIT ?2 OFFSET ?3"
            );
            let mut stmt = conn.prepare(&sql).map_db()?;
            let comments = stmt
                .query_map(params![scene_id, clamped_limit, offset_i64], |row| {
                    let likes: i64 = row.get(6)?;
                    Ok(CodeSceneCommentDto {
                        comment_id: row.get(0)?,
                        scene_id: row.get(1)?,
                        author_user_id: row.get(2)?,
                        author_username: row.get(3)?,
                        content: row.get(4)?,
                        parent_comment_id: row.get(5)?,
                        likes_count: likes.max(0) as u64,
                        viewer_liked: false,
                        created_at_ms: row.get(7)?,
                    })
                })
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            Ok(Some(ListCodeSceneCommentsResponse {
                scene_id: scene_id.to_string(),
                comments,
                total: total.max(0) as usize,
            }))
        })
    }

    /// Inserts a parameterized comment and atomically increments `code_scenes.comments_count`.
    pub fn create_code_scene_comment(
        &self,
        scene_id: &str,
        author_user_id: i64,
        author_username: &str,
        content: &str,
        parent_comment_id: Option<&str>,
    ) -> Result<CreateCodeSceneCommentOutcome> {
        let now_ms = Utc::now().timestamp_millis();
        let comment_id = format!("csc_{}", Uuid::new_v4().simple());
        self.pool.with_transaction(|tx| {
            let scene_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM code_scenes WHERE scene_id = ?1)",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;
            if !scene_exists {
                return Ok(CreateCodeSceneCommentOutcome::SceneNotFound);
            }
            if let Some(pid) = parent_comment_id {
                let parent_exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM code_scene_comments WHERE comment_id = ?1 AND scene_id = ?2)",
                        params![pid, scene_id],
                        |r| r.get(0),
                    )
                    .map_db()?;
                if !parent_exists {
                    return Ok(CreateCodeSceneCommentOutcome::ParentCommentNotFound);
                }
            }

            tx.execute(
                r#"
                INSERT INTO code_scene_comments (
                    comment_id, scene_id, author_user_id, author_username,
                    content, parent_comment_id, likes_count, created_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)
                "#,
                params![
                    comment_id,
                    scene_id,
                    author_user_id,
                    author_username,
                    content,
                    parent_comment_id,
                    now_ms,
                ],
            )
            .map_db()?;

            tx.execute(
                "UPDATE code_scenes SET comments_count = comments_count + 1, updated_at_ms = ?2 WHERE scene_id = ?1",
                params![scene_id, now_ms],
            )
            .map_db()?;

            let comments_count: i64 = tx
                .query_row(
                    "SELECT comments_count FROM code_scenes WHERE scene_id = ?1",
                    [scene_id],
                    |r| r.get(0),
                )
                .map_db()?;

            Ok(CreateCodeSceneCommentOutcome::Created {
                comment: CodeSceneCommentDto {
                    comment_id,
                    scene_id: scene_id.to_string(),
                    author_user_id,
                    author_username: author_username.to_string(),
                    content: content.to_string(),
                    parent_comment_id: parent_comment_id.map(str::to_string),
                    likes_count: 0,
                    viewer_liked: false,
                    created_at_ms: now_ms,
                },
                comments_count: comments_count.max(0) as u64,
            })
        })
    }

    /// Atomically appends behavioral telemetry rows, deletes `code_scene_follows` rows matching
    /// any `HideAuthor` targets, applies commutative plays/shares counter deltas, and upserts
    /// `code_scene_user_profiles` with a monotonic revision guard in one transaction.
    pub fn persist_code_scene_telemetry_batch(
        &self,
        events_with_rewards: &[(SceneBehaviorEventDto, f64)],
        updated_profile: Option<&UserInterestProfile>,
        scene_counter_deltas: &[(String, u64, u64)],
        hidden_author_ids_to_unfollow: &[i64],
    ) -> Result<()> {
        let now_ms = Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let mut has_hide_author_event = false;
            let mut unfollow_pairs: BTreeSet<(i64, i64)> = BTreeSet::new();

            for (ev, reward) in events_with_rewards {
                let tags_json = serde_json::to_string(&ev.tags).unwrap_or_else(|_| "[]".into());
                let event_id = if ev.event_id.trim().is_empty() {
                    format!("cs_ev_{}", Uuid::new_v4().simple())
                } else {
                    ev.event_id.clone()
                };
                let ts = if ev.timestamp_ms > 0 {
                    ev.timestamp_ms
                } else {
                    now_ms
                };
                tx.execute(
                    r#"
                    INSERT INTO code_scene_interactions (
                        event_id, user_id, session_id, scene_id, event_kind,
                        asset_kind, author_user_id, tags_json, dwell_ms, duration_ms,
                        completion_ratio, loop_count, game_inputs, game_play_ms,
                        game_restarts, game_score, reason, reward, timestamp_ms, recorded_at_ms
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5,
                        ?6, ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14,
                        ?15, ?16, ?17, ?18, ?19, ?20
                    )
                    "#,
                    params![
                        event_id,
                        ev.user_id,
                        ev.session_id,
                        ev.scene_id,
                        ev.kind.as_str(),
                        ev.asset_kind.map(|k| k.as_str()),
                        ev.author_user_id,
                        tags_json,
                        ev.dwell_ms as i64,
                        ev.duration_ms as i64,
                        ev.completion_ratio,
                        ev.loop_count as i64,
                        ev.game_inputs as i64,
                        ev.game_play_ms as i64,
                        ev.game_restarts as i64,
                        ev.game_score,
                        ev.reason,
                        reward,
                        ts,
                        now_ms,
                    ],
                )
                .map_db()?;

                if ev.kind == SceneBehaviorEventKind::HideAuthor {
                    has_hide_author_event = true;
                    if let Some(hidden_author) = ev.author_user_id.filter(|&id| id > 0) {
                        unfollow_pairs.insert((ev.user_id, hidden_author));
                    }
                }
            }

            if let Some(uid) = updated_profile
                .map(|p| p.user_id)
                .or_else(|| events_with_rewards.first().map(|(ev, _)| ev.user_id))
            {
                for &cid in hidden_author_ids_to_unfollow {
                    if cid > 0 {
                        unfollow_pairs.insert((uid, cid));
                    }
                }
            }
            if has_hide_author_event {
                if let Some(profile) = updated_profile {
                    for &cid in &profile.hidden_author_ids {
                        if cid > 0 {
                            unfollow_pairs.insert((profile.user_id, cid));
                        }
                    }
                }
            }

            for (follower_uid, creator_uid) in unfollow_pairs {
                tx.execute(
                    "DELETE FROM code_scene_follows WHERE follower_user_id = ?1 AND creator_user_id = ?2",
                    params![follower_uid, creator_uid],
                )
                .map_db()?;
            }

            for (scene_id, delta_plays, delta_shares) in scene_counter_deltas {
                if *delta_plays == 0 && *delta_shares == 0 {
                    continue;
                }
                tx.execute(
                    r#"
                    UPDATE code_scenes
                    SET plays_count = plays_count + ?2,
                        shares_count = shares_count + ?3,
                        updated_at_ms = ?4
                    WHERE scene_id = ?1
                    "#,
                    params![
                        scene_id,
                        *delta_plays as i64,
                        *delta_shares as i64,
                        now_ms,
                    ],
                )
                .map_db()?;
            }

            if let Some(profile) = updated_profile {
                let profile_json = serde_json::to_string(profile).unwrap_or_else(|_| "{}".into());
                tx.execute(
                    r#"
                    INSERT INTO code_scene_user_profiles (user_id, schema_version, profile_json, revision, updated_at_ms)
                    VALUES (?1, ?2, ?3, ?4, ?5)
                    ON CONFLICT(user_id) DO UPDATE SET
                        schema_version = excluded.schema_version,
                        profile_json = excluded.profile_json,
                        revision = excluded.revision,
                        updated_at_ms = excluded.updated_at_ms
                    WHERE excluded.revision >= code_scene_user_profiles.revision
                    "#,
                    params![
                        profile.user_id,
                        CODE_SCENE_SCHEMA_VERSION_V1 as i64,
                        profile_json,
                        profile.revision as i64,
                        now_ms,
                    ],
                )
                .map_db()?;
            }
            Ok(())
        })
    }

    /// Returns the subset of non-empty `event_ids` already persisted in `code_scene_interactions`
    /// for `user_id`, enabling idempotent outbox retry deduplication.
    pub fn find_existing_code_scene_event_ids(
        &self,
        user_id: i64,
        event_ids: &[String],
    ) -> Result<HashSet<String>> {
        let mut found = HashSet::new();
        if event_ids.is_empty() {
            return Ok(found);
        }
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT 1 FROM code_scene_interactions WHERE user_id = ?1 AND event_id = ?2 LIMIT 1",
                )
                .map_db()?;
            for eid in event_ids {
                let trimmed = eid.trim();
                if trimmed.is_empty() || found.contains(trimmed) {
                    continue;
                }
                let exists: Option<i64> = stmt
                    .query_row(params![user_id, trimmed], |r| r.get(0))
                    .optional()
                    .map_db()?;
                if exists.is_some() {
                    found.insert(trimmed.to_string());
                }
            }
            Ok(found)
        })
    }

    pub fn load_all_code_scene_user_profiles(&self) -> Result<Vec<UserInterestProfile>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare("SELECT profile_json FROM code_scene_user_profiles ORDER BY user_id ASC")
                .map_db()?;
            let jsons: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            let mut out = Vec::with_capacity(jsons.len());
            for j in jsons {
                if let Ok(mut p) = serde_json::from_str::<UserInterestProfile>(&j) {
                    p.sanitize();
                    out.push(p);
                }
            }
            Ok(out)
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn load_all_code_scene_social_edges(
        &self,
    ) -> Result<(Vec<(i64, String)>, Vec<(i64, String)>, Vec<(i64, i64)>)> {
        self.pool.with_reader(|conn| {
            let mut s1 = conn
                .prepare("SELECT user_id, scene_id FROM code_scene_likes")
                .map_db()?;
            let likes = s1
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            let mut s2 = conn
                .prepare("SELECT user_id, scene_id FROM code_scene_bookmarks")
                .map_db()?;
            let bookmarks = s2
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            let mut s3 = conn
                .prepare("SELECT follower_user_id, creator_user_id FROM code_scene_follows")
                .map_db()?;
            let follows = s3
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;

            Ok((likes, bookmarks, follows))
        })
    }

    pub fn upsert_code_scene_bot_draft(&self, draft: &CodeSceneBotDraftRecord) -> Result<()> {
        let tags_json = serde_json::to_string(&draft.tags).unwrap_or_else(|_| "[]".into());
        let manifest_json = serde_json::to_string(&draft.manifest).unwrap_or_else(|_| "{}".into());
        let validation_json =
            serde_json::to_string(&draft.validation).unwrap_or_else(|_| "{}".into());
        let history_json =
            serde_json::to_string(&draft.prompt_history).unwrap_or_else(|_| "[]".into());
        self.pool.with_writer(|conn| {
            conn.execute(
                r#"
                INSERT INTO code_scene_bot_drafts (
                    draft_id, user_id, bot_id, bot_username, title, description, kind,
                    tags_json, source_code, code_format, manifest_json, controls_hint,
                    validation_json, engine_used, repair_attempts, remix_parent_id,
                    prompt_history_json, published_scene_id, created_at_ms, updated_at_ms
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20
                )
                ON CONFLICT(draft_id) DO UPDATE SET
                    bot_id = excluded.bot_id,
                    bot_username = excluded.bot_username,
                    title = excluded.title,
                    description = excluded.description,
                    kind = excluded.kind,
                    tags_json = excluded.tags_json,
                    source_code = excluded.source_code,
                    code_format = excluded.code_format,
                    manifest_json = excluded.manifest_json,
                    controls_hint = excluded.controls_hint,
                    validation_json = excluded.validation_json,
                    engine_used = excluded.engine_used,
                    repair_attempts = excluded.repair_attempts,
                    remix_parent_id = COALESCE(excluded.remix_parent_id, code_scene_bot_drafts.remix_parent_id),
                    prompt_history_json = excluded.prompt_history_json,
                    published_scene_id = COALESCE(excluded.published_scene_id, code_scene_bot_drafts.published_scene_id),
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![
                    draft.draft_id,
                    draft.user_id,
                    draft.bot_id,
                    draft.bot_username,
                    draft.title,
                    draft.description,
                    draft.kind.as_str(),
                    tags_json,
                    draft.source_code,
                    draft.code_format,
                    manifest_json,
                    draft.controls_hint,
                    validation_json,
                    draft.engine_used,
                    draft.repair_attempts as i64,
                    draft.remix_parent_id,
                    history_json,
                    draft.published_scene_id,
                    draft.created_at_ms,
                    draft.updated_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_code_scene_bot_draft(&self, draft_id: &str) -> Result<Option<CodeSceneBotDraftRecord>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"
                SELECT draft_id, user_id, bot_id, bot_username, title, description, kind,
                       tags_json, source_code, code_format, manifest_json, controls_hint,
                       validation_json, engine_used, repair_attempts, remix_parent_id,
                       prompt_history_json, published_scene_id, created_at_ms, updated_at_ms
                FROM code_scene_bot_drafts
                WHERE draft_id = ?1
                "#,
                [draft_id],
                |row| {
                    let kind_str: String = row.get(6)?;
                    let kind = AssetKind::parse(&kind_str).unwrap_or(AssetKind::Animation);
                    let tags_json: String = row.get(7)?;
                    let manifest_json: String = row.get(10)?;
                    let validation_json: String = row.get(12)?;
                    let repair_attempts: i64 = row.get(14)?;
                    let history_json: String = row.get(16)?;
                    let mut manifest: AssetManifest = serde_json::from_str(&manifest_json)
                        .unwrap_or_else(|_| AssetManifest::for_kind(kind));
                    manifest.kind = kind;
                    manifest.clamp_in_place();
                    let validation: ValidationReportDto =
                        serde_json::from_str(&validation_json).unwrap_or_default();
                    Ok(CodeSceneBotDraftRecord {
                        draft_id: row.get(0)?,
                        user_id: row.get(1)?,
                        bot_id: row.get(2)?,
                        bot_username: row.get(3)?,
                        title: row.get(4)?,
                        description: row.get(5)?,
                        kind,
                        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                        source_code: row.get(8)?,
                        code_format: row.get(9)?,
                        manifest,
                        controls_hint: row.get(11)?,
                        validation,
                        engine_used: row.get(13)?,
                        repair_attempts: repair_attempts.max(0) as u32,
                        remix_parent_id: row.get(15)?,
                        prompt_history: serde_json::from_str(&history_json).unwrap_or_default(),
                        published_scene_id: row.get(17)?,
                        created_at_ms: row.get(18)?,
                        updated_at_ms: row.get(19)?,
                    })
                },
            )
            .optional()
            .map_db()
        })
    }
}
