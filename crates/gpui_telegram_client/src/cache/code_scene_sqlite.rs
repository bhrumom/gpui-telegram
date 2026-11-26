use crate::cache::sqlite_cache::SqliteClientCache;
use gpui_telegram_code_scene::UserInterestProfile;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneResponse, CodeSceneAssetDto, CodeSceneCommentDto, SceneBehaviorEventDto,
    CODE_SCENE_SCHEMA_VERSION_V1,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn cs_db_err(e: rusqlite::Error) -> TelegramError {
    TelegramError::Database(e.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedCodeSceneRecord {
    pub asset: CodeSceneAssetDto,
    pub is_local_only: bool,
    pub cached_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryOutboxRecord {
    pub outbox_id: i64,
    pub event: SceneBehaviorEventDto,
    pub retry_count: u32,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalCodeSceneDraft {
    pub draft_id: String,
    pub user_id: i64,
    pub prompt: String,
    pub remix_parent_id: Option<String>,
    pub draft: AiGenerateSceneResponse,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSceneSocialRelationKind {
    Like,
    Bookmark,
    Follow,
    Dislike,
    HideAuthor,
    HideTag,
}

impl CodeSceneSocialRelationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Like => "like",
            Self::Bookmark => "bookmark",
            Self::Follow => "follow",
            Self::Dislike => "dislike",
            Self::HideAuthor => "hide_author",
            Self::HideTag => "hide_tag",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "like" => Some(Self::Like),
            "bookmark" => Some(Self::Bookmark),
            "follow" => Some(Self::Follow),
            "dislike" => Some(Self::Dislike),
            "hide_author" => Some(Self::HideAuthor),
            "hide_tag" => Some(Self::HideTag),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeSceneLocalSocialEntry {
    pub user_id: i64,
    pub relation_kind: CodeSceneSocialRelationKind,
    pub target_id: String,
    pub active: bool,
    pub pending_sync: bool,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedCodeSceneComment {
    pub comment: CodeSceneCommentDto,
    pub pending_sync: bool,
}

impl SqliteClientCache {
    pub(crate) fn clear_code_scene_tables(&self) {
        let conn = self.conn.lock();
        let _ = conn.execute_batch(
            "DELETE FROM code_scenes_cache;
             DELETE FROM code_scene_local_profile;
             DELETE FROM code_scene_telemetry_outbox;
             DELETE FROM code_scene_local_drafts;
             DELETE FROM code_scene_local_social;
             DELETE FROM code_scene_local_comments;",
        );
    }

    pub(crate) fn clear_code_scene_for_user(&self, user_id: i64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM code_scenes_cache WHERE author_user_id = ?1 AND is_local_only = 1",
            params![user_id],
        );
        let _ = conn.execute(
            "DELETE FROM code_scene_local_profile WHERE user_id = ?1",
            params![user_id],
        );
        let _ = conn.execute(
            "DELETE FROM code_scene_telemetry_outbox WHERE user_id = ?1",
            params![user_id],
        );
        let _ = conn.execute(
            "DELETE FROM code_scene_local_drafts WHERE user_id = ?1",
            params![user_id],
        );
        let _ = conn.execute(
            "DELETE FROM code_scene_local_social WHERE user_id = ?1",
            params![user_id],
        );
        let _ = conn.execute(
            "DELETE FROM code_scene_local_comments WHERE author_user_id = ?1",
            params![user_id],
        );
    }

    pub(crate) fn load_cached_code_scenes(&self) -> Result<HashMap<String, CachedCodeSceneRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json, is_local_only, cached_at_ms FROM code_scenes_cache")
            .map_err(cs_db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(cs_db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (json, local_flag, cached_at_ms) = row.map_err(cs_db_err)?;
            if let Ok(asset) = serde_json::from_str::<CodeSceneAssetDto>(&json) {
                out.insert(
                    asset.scene_id.clone(),
                    CachedCodeSceneRecord {
                        asset,
                        is_local_only: local_flag != 0,
                        cached_at_ms,
                    },
                );
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_cached_code_scene(&self, rec: &CachedCodeSceneRecord) {
        if let Ok(json) = serde_json::to_string(&rec.asset) {
            let sha = rec.asset.content_sha256();
            let conn = self.conn.lock();
            let _ = conn.execute(
                r#"
                INSERT OR REPLACE INTO code_scenes_cache (
                    scene_id, kind, author_user_id, creator_bot_id, title,
                    source_sha256, payload_json, is_local_only, updated_at_ms, cached_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                "#,
                params![
                    rec.asset.scene_id,
                    rec.asset.kind.as_str(),
                    rec.asset.author_user_id,
                    rec.asset.creator_bot_id,
                    rec.asset.title,
                    sha,
                    json,
                    if rec.is_local_only { 1i64 } else { 0i64 },
                    rec.asset.updated_at_ms,
                    rec.cached_at_ms,
                ],
            );
        }
    }

    pub(crate) fn delete_cached_code_scene(&self, scene_id: &str) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM code_scenes_cache WHERE scene_id = ?1",
            params![scene_id],
        );
    }

    pub(crate) fn load_code_scene_profiles(&self) -> Result<HashMap<i64, UserInterestProfile>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT user_id, profile_json FROM code_scene_local_profile")
            .map_err(cs_db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(cs_db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (uid, json) = row.map_err(cs_db_err)?;
            if let Ok(mut prof) = serde_json::from_str::<UserInterestProfile>(&json) {
                prof.sanitize();
                out.insert(uid, prof);
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_code_scene_profile(&self, profile: &UserInterestProfile, now_ms: i64) {
        if let Ok(json) = serde_json::to_string(profile) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                r#"
                INSERT OR REPLACE INTO code_scene_local_profile (
                    user_id, schema_version, revision, profile_json, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![
                    profile.user_id,
                    CODE_SCENE_SCHEMA_VERSION_V1 as i64,
                    profile.revision as i64,
                    json,
                    now_ms,
                ],
            );
        }
    }

    pub(crate) fn load_code_scene_telemetry_outbox(&self) -> Result<Vec<TelemetryOutboxRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT outbox_id, payload_json, retry_count, last_error, created_at_ms, updated_at_ms \
                 FROM code_scene_telemetry_outbox ORDER BY outbox_id ASC",
            )
            .map_err(cs_db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(cs_db_err)?;
        let mut out = Vec::new();
        for row in rows {
            let (outbox_id, json, retry_count, last_error, created_at_ms, updated_at_ms) =
                row.map_err(cs_db_err)?;
            if let Ok(event) = serde_json::from_str::<SceneBehaviorEventDto>(&json) {
                out.push(TelemetryOutboxRecord {
                    outbox_id,
                    event,
                    retry_count: retry_count.max(0) as u32,
                    last_error,
                    created_at_ms,
                    updated_at_ms,
                });
            }
        }
        Ok(out)
    }

    pub(crate) fn enqueue_code_scene_telemetry(
        &self,
        event: &SceneBehaviorEventDto,
        now_ms: i64,
    ) -> Option<TelemetryOutboxRecord> {
        let json = serde_json::to_string(event).ok()?;
        let conn = self.conn.lock();
        let _ = conn.execute(
            r#"
            INSERT OR IGNORE INTO code_scene_telemetry_outbox (
                event_id, user_id, scene_id, event_kind, payload_json,
                retry_count, last_error, created_at_ms, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, 0, NULL, ?6, ?6)
            "#,
            params![
                event.event_id,
                event.user_id,
                event.scene_id,
                event.kind.as_str(),
                json,
                now_ms,
            ],
        );
        conn.query_row(
            "SELECT outbox_id, payload_json, retry_count, last_error, created_at_ms, updated_at_ms \
             FROM code_scene_telemetry_outbox WHERE event_id = ?1",
            params![event.event_id],
            |r| {
                let id: i64 = r.get(0)?;
                let payload: String = r.get(1)?;
                let retry: i64 = r.get(2)?;
                let err: Option<String> = r.get(3)?;
                let created: i64 = r.get(4)?;
                let updated: i64 = r.get(5)?;
                Ok((id, payload, retry, err, created, updated))
            },
        )
        .ok()
        .and_then(|(outbox_id, payload, retry, last_error, created_at_ms, updated_at_ms)| {
            serde_json::from_str::<SceneBehaviorEventDto>(&payload)
                .ok()
                .map(|ev| TelemetryOutboxRecord {
                    outbox_id,
                    event: ev,
                    retry_count: retry.max(0) as u32,
                    last_error,
                    created_at_ms,
                    updated_at_ms,
                })
        })
    }

    pub(crate) fn ack_code_scene_telemetry_outbox(&self, outbox_ids: &[i64]) {
        if outbox_ids.is_empty() {
            return;
        }
        let conn = self.conn.lock();
        for &id in outbox_ids {
            let _ = conn.execute(
                "DELETE FROM code_scene_telemetry_outbox WHERE outbox_id = ?1",
                params![id],
            );
        }
    }

    pub(crate) fn mark_code_scene_telemetry_outbox_failed(
        &self,
        outbox_ids: &[i64],
        err_msg: &str,
        now_ms: i64,
    ) {
        if outbox_ids.is_empty() {
            return;
        }
        let conn = self.conn.lock();
        for &id in outbox_ids {
            let _ = conn.execute(
                "UPDATE code_scene_telemetry_outbox \
                 SET retry_count = retry_count + 1, last_error = ?2, updated_at_ms = ?3 \
                 WHERE outbox_id = ?1",
                params![id, err_msg, now_ms],
            );
        }
    }

    pub(crate) fn load_code_scene_drafts(&self) -> Result<HashMap<String, LocalCodeSceneDraft>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM code_scene_local_drafts")
            .map_err(cs_db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(cs_db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(cs_db_err)?;
            if let Ok(draft) = serde_json::from_str::<LocalCodeSceneDraft>(&json) {
                out.insert(draft.draft_id.clone(), draft);
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_code_scene_draft(&self, draft: &LocalCodeSceneDraft) {
        if let Ok(json) = serde_json::to_string(draft) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                r#"
                INSERT OR REPLACE INTO code_scene_local_drafts (
                    draft_id, user_id, kind, title, remix_parent_id, prompt,
                    payload_json, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
                params![
                    draft.draft_id,
                    draft.user_id,
                    draft.draft.kind.as_str(),
                    draft.draft.title,
                    draft.remix_parent_id,
                    draft.prompt,
                    json,
                    draft.created_at_ms,
                    draft.updated_at_ms,
                ],
            );
        }
    }

    pub(crate) fn delete_code_scene_draft(&self, draft_id: &str) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM code_scene_local_drafts WHERE draft_id = ?1",
            params![draft_id],
        );
    }

    pub(crate) fn load_code_scene_social(
        &self,
    ) -> Result<HashMap<(i64, CodeSceneSocialRelationKind, String), CodeSceneLocalSocialEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT user_id, relation_kind, target_id, active, pending_sync, updated_at_ms \
                 FROM code_scene_local_social",
            )
            .map_err(cs_db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(cs_db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (user_id, kind_s, target_id, active, pending, updated_at_ms) =
                row.map_err(cs_db_err)?;
            if let Some(relation_kind) = CodeSceneSocialRelationKind::parse(&kind_s) {
                out.insert(
                    (user_id, relation_kind, target_id.clone()),
                    CodeSceneLocalSocialEntry {
                        user_id,
                        relation_kind,
                        target_id,
                        active: active != 0,
                        pending_sync: pending != 0,
                        updated_at_ms,
                    },
                );
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_code_scene_social(&self, entry: &CodeSceneLocalSocialEntry) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            r#"
            INSERT OR REPLACE INTO code_scene_local_social (
                user_id, relation_kind, target_id, active, pending_sync, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                entry.user_id,
                entry.relation_kind.as_str(),
                entry.target_id,
                if entry.active { 1i64 } else { 0i64 },
                if entry.pending_sync { 1i64 } else { 0i64 },
                entry.updated_at_ms,
            ],
        );
    }

    pub(crate) fn load_code_scene_comments(
        &self,
    ) -> Result<HashMap<String, Vec<CachedCodeSceneComment>>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT scene_id, pending_sync, payload_json \
                 FROM code_scene_local_comments ORDER BY created_at_ms DESC, comment_id ASC",
            )
            .map_err(cs_db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(cs_db_err)?;
        let mut out: HashMap<String, Vec<CachedCodeSceneComment>> = HashMap::new();
        for row in rows {
            let (scene_id, pending, json) = row.map_err(cs_db_err)?;
            if let Ok(comment) = serde_json::from_str::<CodeSceneCommentDto>(&json) {
                out.entry(scene_id).or_default().push(CachedCodeSceneComment {
                    comment,
                    pending_sync: pending != 0,
                });
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_code_scene_comment(&self, record: &CachedCodeSceneComment) {
        if let Ok(json) = serde_json::to_string(&record.comment) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                r#"
                INSERT OR REPLACE INTO code_scene_local_comments (
                    comment_id, scene_id, author_user_id, parent_comment_id,
                    likes_count, created_at_ms, pending_sync, payload_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
                params![
                    record.comment.comment_id,
                    record.comment.scene_id,
                    record.comment.author_user_id,
                    record.comment.parent_comment_id,
                    record.comment.likes_count as i64,
                    record.comment.created_at_ms,
                    if record.pending_sync { 1i64 } else { 0i64 },
                    json,
                ],
            );
        }
    }

    pub(crate) fn delete_code_scene_comment(&self, comment_id: &str) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM code_scene_local_comments WHERE comment_id = ?1",
            params![comment_id],
        );
    }

    pub(crate) fn replace_synced_code_scene_comments(
        &self,
        scene_id: &str,
        records: &[CachedCodeSceneComment],
    ) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM code_scene_local_comments WHERE scene_id = ?1 AND pending_sync = 0",
            params![scene_id],
        );
        for rec in records {
            if let Ok(json) = serde_json::to_string(&rec.comment) {
                let _ = conn.execute(
                    r#"
                    INSERT OR REPLACE INTO code_scene_local_comments (
                        comment_id, scene_id, author_user_id, parent_comment_id,
                        likes_count, created_at_ms, pending_sync, payload_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    "#,
                    params![
                        rec.comment.comment_id,
                        rec.comment.scene_id,
                        rec.comment.author_user_id,
                        rec.comment.parent_comment_id,
                        rec.comment.likes_count as i64,
                        rec.comment.created_at_ms,
                        if rec.pending_sync { 1i64 } else { 0i64 },
                        json,
                    ],
                );
            }
        }
    }
}
