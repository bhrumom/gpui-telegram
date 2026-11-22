use crate::db::mcp_oauth_store::{ct_str_eq, sha256_hex};
use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const ASSIST_TEMP_CODE_TTL_MS: i64 = 15 * 60_000; // 15 minutes
pub const ASSIST_LOCKOUT_MS: i64 = 5 * 60_000; // 5 minutes
pub const ASSIST_MAX_FAILED_ATTEMPTS: i64 = 5;

pub const MIGRATION_V40_REMOTE_ASSIST: &str = r#"
CREATE TABLE IF NOT EXISTS remote_assist_devices (
    user_id INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    assist_id TEXT NOT NULL UNIQUE,
    allow_assist INTEGER NOT NULL DEFAULT 1,
    code_mode TEXT NOT NULL DEFAULT 'temporary',
    code_hash TEXT NOT NULL DEFAULT '',
    temp_code TEXT,
    failed_attempts INTEGER NOT NULL DEFAULT 0,
    locked_until_ms INTEGER,
    expires_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, device_id)
) STRICT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_remote_assist_id ON remote_assist_devices(assist_id);

CREATE TABLE IF NOT EXISTS remote_sessions (
    session_id TEXT PRIMARY KEY,
    owner_user_id INTEGER NOT NULL,
    controller_user_id INTEGER,
    controller_kind TEXT NOT NULL,
    controller_name TEXT NOT NULL,
    controller_platform TEXT NOT NULL DEFAULT '',
    controller_id TEXT NOT NULL DEFAULT '',
    target_node_id TEXT NOT NULL,
    target_device_id TEXT NOT NULL DEFAULT '',
    mode TEXT NOT NULL DEFAULT 'desktop',
    active_tool TEXT,
    relay_mode TEXT NOT NULL DEFAULT 'http_fallback',
    rtt_ms INTEGER,
    fps INTEGER,
    temp_relay_token_hash TEXT,
    started_at_ms INTEGER NOT NULL,
    last_activity_ms INTEGER NOT NULL,
    ended_at_ms INTEGER,
    end_reason TEXT
) STRICT;
CREATE INDEX IF NOT EXISTS idx_remote_sessions_owner_active ON remote_sessions(owner_user_id, ended_at_ms, started_at_ms DESC);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAssistDeviceRecord {
    pub user_id: u64,
    pub device_id: String,
    pub assist_id: String,
    pub allow_assist: bool,
    pub code_mode: String,
    pub code_hash: String,
    pub temp_code: Option<String>,
    pub failed_attempts: i64,
    pub locked_until_ms: Option<i64>,
    pub expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSessionRecord {
    pub session_id: String,
    pub owner_user_id: u64,
    pub controller_user_id: Option<u64>,
    pub controller_kind: String,
    pub controller_name: String,
    pub controller_platform: String,
    pub controller_id: String,
    pub target_node_id: String,
    pub target_device_id: String,
    pub mode: String,
    pub active_tool: Option<String>,
    pub relay_mode: String,
    pub rtt_ms: Option<u32>,
    pub fps: Option<u32>,
    pub temp_relay_token_hash: Option<String>,
    pub started_at_ms: i64,
    pub last_activity_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub end_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistVerifyOutcome {
    Authorized(RemoteAssistDeviceRecord),
    NotFound,
    AssistDisabled,
    CodeExpired,
    InvalidCode {
        remaining_attempts: u32,
        locked_until_ms: Option<i64>,
    },
    RateLimited {
        locked_until_ms: i64,
    },
}

pub fn normalize_assist_id(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Validates a custom remote assist code: 8..=16 ASCII alphanumeric with at least one letter and one digit.
pub fn is_valid_custom_assist_code(code: &str) -> bool {
    let trimmed = code.trim();
    let len = trimmed.len();
    if !(8..=16).contains(&len) {
        return false;
    }
    let mut has_letter = false;
    let mut has_digit = false;
    for b in trimmed.bytes() {
        if b.is_ascii_alphabetic() {
            has_letter = true;
        } else if b.is_ascii_digit() {
            has_digit = true;
        } else {
            return false;
        }
    }
    has_letter && has_digit
}

pub fn generate_9_digit_assist_id() -> String {
    let mut bytes = [0u8; 8];
    if getrandom::getrandom(&mut bytes).is_err() {
        let u = uuid::Uuid::new_v4();
        bytes.copy_from_slice(&u.as_bytes()[..8]);
    }
    let num = u64::from_le_bytes(bytes);
    let nine_digits = 100_000_000 + (num % 900_000_000);
    format!("{nine_digits:09}")
}

pub fn generate_temp_assist_code() -> String {
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz";
    const DIGITS: &[u8] = b"23456789";
    const ALL: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789";
    let mut raw = [0u8; 10];
    if getrandom::getrandom(&mut raw).is_err() {
        let u = uuid::Uuid::new_v4();
        raw.copy_from_slice(&u.as_bytes()[..10]);
    }
    let mut chars = Vec::with_capacity(8);
    chars.push(LETTERS[(raw[0] as usize) % LETTERS.len()]);
    chars.push(DIGITS[(raw[1] as usize) % DIGITS.len()]);
    for &b in &raw[2..8] {
        chars.push(ALL[(b as usize) % ALL.len()]);
    }
    // Deterministically mix positions using remaining random bytes so letter/digit aren't always first.
    for i in (1..chars.len()).rev() {
        let j = (raw[8 + (i % 2)] as usize + i * 7) % (i + 1);
        chars.swap(i, j);
    }
    String::from_utf8(chars).unwrap_or_else(|_| "Fabu2026".to_string())
}

fn map_assist_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteAssistDeviceRecord> {
    Ok(RemoteAssistDeviceRecord {
        user_id: r.get::<_, i64>(0)?.max(0) as u64,
        device_id: r.get(1)?,
        assist_id: r.get(2)?,
        allow_assist: r.get::<_, i64>(3)? != 0,
        code_mode: r.get(4)?,
        code_hash: r.get(5)?,
        temp_code: r.get(6)?,
        failed_attempts: r.get(7)?,
        locked_until_ms: r.get(8)?,
        expires_at_ms: r.get(9)?,
        created_at_ms: r.get(10)?,
        updated_at_ms: r.get(11)?,
    })
}

fn map_remote_session_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteSessionRecord> {
    Ok(RemoteSessionRecord {
        session_id: r.get(0)?,
        owner_user_id: r.get::<_, i64>(1)?.max(0) as u64,
        controller_user_id: r.get::<_, Option<i64>>(2)?.map(|v| v.max(0) as u64),
        controller_kind: r.get(3)?,
        controller_name: r.get(4)?,
        controller_platform: r.get(5)?,
        controller_id: r.get(6)?,
        target_node_id: r.get(7)?,
        target_device_id: r.get(8)?,
        mode: r.get(9)?,
        active_tool: r.get(10)?,
        relay_mode: r.get(11)?,
        rtt_ms: r.get::<_, Option<i64>>(12)?.and_then(|v| u32::try_from(v).ok()),
        fps: r.get::<_, Option<i64>>(13)?.and_then(|v| u32::try_from(v).ok()),
        temp_relay_token_hash: r.get(14)?,
        started_at_ms: r.get(15)?,
        last_activity_ms: r.get(16)?,
        ended_at_ms: r.get(17)?,
        end_reason: r.get(18)?,
    })
}

impl ServerDatabase {
    pub fn get_or_init_assist_device(
        &self,
        user_id: u64,
        device_id: &str,
        now_ms: i64,
    ) -> Result<RemoteAssistDeviceRecord> {
        let dev = device_id.trim();
        let dev = if dev.is_empty() { "default" } else { dev };
        self.pool.with_transaction(|tx| {
            let existing: Option<RemoteAssistDeviceRecord> = tx
                .query_row(
                    r#"
                    SELECT user_id, device_id, assist_id, allow_assist, code_mode, code_hash,
                           temp_code, failed_attempts, locked_until_ms, expires_at_ms,
                           created_at_ms, updated_at_ms
                    FROM remote_assist_devices
                    WHERE user_id = ?1 AND device_id = ?2
                    LIMIT 1
                    "#,
                    params![user_id as i64, dev],
                    map_assist_row,
                )
                .optional()
                .map_db()?;

            if let Some(mut rec) = existing {
                let needs_temp_refresh = rec.code_mode == "temporary"
                    && (rec.temp_code.is_none()
                        || rec.code_hash.is_empty()
                        || rec.expires_at_ms.is_some_and(|e| now_ms > e));
                if needs_temp_refresh {
                    let fresh = generate_temp_assist_code();
                    let fresh_hash = sha256_hex(&fresh);
                    let exp = now_ms.saturating_add(ASSIST_TEMP_CODE_TTL_MS);
                    tx.execute(
                        r#"
                        UPDATE remote_assist_devices
                        SET code_hash = ?1,
                            temp_code = ?2,
                            expires_at_ms = ?3,
                            updated_at_ms = ?4
                        WHERE user_id = ?5 AND device_id = ?6
                        "#,
                        params![&fresh_hash, &fresh, exp, now_ms, user_id as i64, dev],
                    )
                    .map_db()?;
                    rec.code_hash = fresh_hash;
                    rec.temp_code = Some(fresh);
                    rec.expires_at_ms = Some(exp);
                    rec.updated_at_ms = now_ms;
                }
                return Ok(rec);
            }

            let mut chosen_id = String::new();
            for _ in 0..32 {
                let cand = generate_9_digit_assist_id();
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM remote_assist_devices WHERE assist_id = ?1)",
                        params![&cand],
                        |r| r.get(0),
                    )
                    .map_db()?;
                if !exists {
                    chosen_id = cand;
                    break;
                }
            }
            if chosen_id.is_empty() {
                return Err(TelegramError::Internal(
                    "Failed to allocate unique 9-digit assist_id".to_string(),
                ));
            }

            let fresh = generate_temp_assist_code();
            let fresh_hash = sha256_hex(&fresh);
            let exp = now_ms.saturating_add(ASSIST_TEMP_CODE_TTL_MS);
            tx.execute(
                r#"
                INSERT INTO remote_assist_devices (
                    user_id, device_id, assist_id, allow_assist, code_mode, code_hash,
                    temp_code, failed_attempts, locked_until_ms, expires_at_ms,
                    created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, 1, 'temporary', ?4, ?5, 0, NULL, ?6, ?7, ?7)
                "#,
                params![
                    user_id as i64,
                    dev,
                    &chosen_id,
                    &fresh_hash,
                    &fresh,
                    exp,
                    now_ms,
                ],
            )
            .map_db()?;

            Ok(RemoteAssistDeviceRecord {
                user_id,
                device_id: dev.to_string(),
                assist_id: chosen_id,
                allow_assist: true,
                code_mode: "temporary".to_string(),
                code_hash: fresh_hash,
                temp_code: Some(fresh),
                failed_attempts: 0,
                locked_until_ms: None,
                expires_at_ms: Some(exp),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
        })
    }

    pub fn reset_assist_temp_code(
        &self,
        user_id: u64,
        device_id: &str,
        now_ms: i64,
    ) -> Result<RemoteAssistDeviceRecord> {
        let mut rec = self.get_or_init_assist_device(user_id, device_id, now_ms)?;
        let fresh = generate_temp_assist_code();
        let fresh_hash = sha256_hex(&fresh);
        let exp = now_ms.saturating_add(ASSIST_TEMP_CODE_TTL_MS);
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                UPDATE remote_assist_devices
                SET code_mode = 'temporary',
                    code_hash = ?1,
                    temp_code = ?2,
                    failed_attempts = 0,
                    locked_until_ms = NULL,
                    expires_at_ms = ?3,
                    updated_at_ms = ?4
                WHERE user_id = ?5 AND device_id = ?6
                "#,
                params![&fresh_hash, &fresh, exp, now_ms, user_id as i64, &rec.device_id],
            )
            .map_db()?;
            Ok(())
        })?;
        rec.code_mode = "temporary".to_string();
        rec.code_hash = fresh_hash;
        rec.temp_code = Some(fresh);
        rec.failed_attempts = 0;
        rec.locked_until_ms = None;
        rec.expires_at_ms = Some(exp);
        rec.updated_at_ms = now_ms;
        Ok(rec)
    }

    pub fn set_assist_custom_code(
        &self,
        user_id: u64,
        device_id: &str,
        custom_code: &str,
        now_ms: i64,
    ) -> Result<RemoteAssistDeviceRecord> {
        let trimmed = custom_code.trim();
        if !is_valid_custom_assist_code(trimmed) {
            return Err(TelegramError::Validation(
                "Custom assist code must be 8-16 alphanumeric characters containing both letters and digits"
                    .to_string(),
            ));
        }
        let mut rec = self.get_or_init_assist_device(user_id, device_id, now_ms)?;
        let code_hash = sha256_hex(trimmed);
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                UPDATE remote_assist_devices
                SET code_mode = 'custom',
                    code_hash = ?1,
                    temp_code = NULL,
                    failed_attempts = 0,
                    locked_until_ms = NULL,
                    expires_at_ms = NULL,
                    updated_at_ms = ?2
                WHERE user_id = ?3 AND device_id = ?4
                "#,
                params![&code_hash, now_ms, user_id as i64, &rec.device_id],
            )
            .map_db()?;
            Ok(())
        })?;
        rec.code_mode = "custom".to_string();
        rec.code_hash = code_hash;
        rec.temp_code = None;
        rec.failed_attempts = 0;
        rec.locked_until_ms = None;
        rec.expires_at_ms = None;
        rec.updated_at_ms = now_ms;
        Ok(rec)
    }

    pub fn set_assist_allow(
        &self,
        user_id: u64,
        device_id: &str,
        allow_assist: bool,
        now_ms: i64,
    ) -> Result<RemoteAssistDeviceRecord> {
        let mut rec = self.get_or_init_assist_device(user_id, device_id, now_ms)?;
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE remote_assist_devices SET allow_assist = ?1, updated_at_ms = ?2 WHERE user_id = ?3 AND device_id = ?4",
                params![
                    if allow_assist { 1i64 } else { 0i64 },
                    now_ms,
                    user_id as i64,
                    &rec.device_id,
                ],
            )
            .map_db()?;
            Ok(())
        })?;
        rec.allow_assist = allow_assist;
        rec.updated_at_ms = now_ms;
        Ok(rec)
    }

    pub fn verify_assist_start(
        &self,
        raw_assist_id: &str,
        raw_code: &str,
        now_ms: i64,
    ) -> Result<AssistVerifyOutcome> {
        let assist_id = normalize_assist_id(raw_assist_id);
        if assist_id.len() != 9 {
            return Ok(AssistVerifyOutcome::NotFound);
        }
        let code_trimmed = raw_code.trim();
        let candidate_hash = sha256_hex(code_trimmed);

        self.pool.with_transaction(|tx| {
            let rec: Option<RemoteAssistDeviceRecord> = tx
                .query_row(
                    r#"
                    SELECT user_id, device_id, assist_id, allow_assist, code_mode, code_hash,
                           temp_code, failed_attempts, locked_until_ms, expires_at_ms,
                           created_at_ms, updated_at_ms
                    FROM remote_assist_devices
                    WHERE assist_id = ?1
                    LIMIT 1
                    "#,
                    params![&assist_id],
                    map_assist_row,
                )
                .optional()
                .map_db()?;

            let Some(mut rec) = rec else {
                return Ok(AssistVerifyOutcome::NotFound);
            };

            if !rec.allow_assist {
                return Ok(AssistVerifyOutcome::AssistDisabled);
            }

            if let Some(until) = rec.locked_until_ms {
                if now_ms < until {
                    return Ok(AssistVerifyOutcome::RateLimited {
                        locked_until_ms: until,
                    });
                }
                // Lockout window elapsed; reset counter.
                rec.failed_attempts = 0;
                rec.locked_until_ms = None;
            }

            if rec.code_mode == "temporary" && rec.expires_at_ms.is_some_and(|e| now_ms > e) {
                return Ok(AssistVerifyOutcome::CodeExpired);
            }

            if code_trimmed.is_empty() || !ct_str_eq(&candidate_hash, &rec.code_hash) {
                let next_fails = rec.failed_attempts.saturating_add(1);
                let next_lock = if next_fails >= ASSIST_MAX_FAILED_ATTEMPTS {
                    Some(now_ms.saturating_add(ASSIST_LOCKOUT_MS))
                } else {
                    None
                };
                tx.execute(
                    r#"
                    UPDATE remote_assist_devices
                    SET failed_attempts = ?1,
                        locked_until_ms = ?2,
                        updated_at_ms = ?3
                    WHERE user_id = ?4 AND device_id = ?5
                    "#,
                    params![next_fails, next_lock, now_ms, rec.user_id as i64, &rec.device_id],
                )
                .map_db()?;
                let remaining = (ASSIST_MAX_FAILED_ATTEMPTS - next_fails).max(0) as u32;
                return Ok(AssistVerifyOutcome::InvalidCode {
                    remaining_attempts: remaining,
                    locked_until_ms: next_lock,
                });
            }

            tx.execute(
                r#"
                UPDATE remote_assist_devices
                SET failed_attempts = 0,
                    locked_until_ms = NULL,
                    updated_at_ms = ?1
                WHERE user_id = ?2 AND device_id = ?3
                "#,
                params![now_ms, rec.user_id as i64, &rec.device_id],
            )
            .map_db()?;
            rec.failed_attempts = 0;
            rec.locked_until_ms = None;
            Ok(AssistVerifyOutcome::Authorized(rec))
        })
    }

    pub fn insert_remote_session(&self, sess: &RemoteSessionRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO remote_sessions (
                    session_id, owner_user_id, controller_user_id, controller_kind,
                    controller_name, controller_platform, controller_id, target_node_id,
                    target_device_id, mode, active_tool, relay_mode, rtt_ms, fps,
                    temp_relay_token_hash, started_at_ms, last_activity_ms,
                    ended_at_ms, end_reason
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
                ON CONFLICT(session_id) DO UPDATE SET
                    last_activity_ms = excluded.last_activity_ms,
                    active_tool = excluded.active_tool,
                    relay_mode = excluded.relay_mode,
                    rtt_ms = excluded.rtt_ms,
                    fps = excluded.fps,
                    ended_at_ms = excluded.ended_at_ms,
                    end_reason = excluded.end_reason
                "#,
                params![
                    sess.session_id,
                    sess.owner_user_id as i64,
                    sess.controller_user_id.map(|u| u as i64),
                    sess.controller_kind,
                    sess.controller_name,
                    sess.controller_platform,
                    sess.controller_id,
                    sess.target_node_id,
                    sess.target_device_id,
                    sess.mode,
                    sess.active_tool,
                    sess.relay_mode,
                    sess.rtt_ms.map(|v| v as i64),
                    sess.fps.map(|v| v as i64),
                    sess.temp_relay_token_hash,
                    sess.started_at_ms,
                    sess.last_activity_ms,
                    sess.ended_at_ms,
                    sess.end_reason,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn update_remote_session_activity(
        &self,
        session_id: &str,
        active_tool: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE remote_sessions SET last_activity_ms = ?1, active_tool = ?2 WHERE session_id = ?3 AND ended_at_ms IS NULL",
                params![now_ms, active_tool, session_id],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn end_remote_session(
        &self,
        owner_user_id: u64,
        session_id: &str,
        reason: &str,
        now_ms: i64,
    ) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let changed = conn
                .execute(
                    r#"
                    UPDATE remote_sessions
                    SET ended_at_ms = ?1, end_reason = ?2
                    WHERE session_id = ?3 AND owner_user_id = ?4 AND ended_at_ms IS NULL
                    "#,
                    params![now_ms, reason, session_id.trim(), owner_user_id as i64],
                )
                .map_db()?;
            Ok(changed > 0)
        })
    }

    pub fn end_all_remote_sessions(
        &self,
        owner_user_id: u64,
        reason: &str,
        now_ms: i64,
    ) -> Result<Vec<String>> {
        self.pool.with_transaction(|tx| {
            let mut stmt = tx
                .prepare(
                    "SELECT session_id FROM remote_sessions WHERE owner_user_id = ?1 AND ended_at_ms IS NULL",
                )
                .map_db()?;
            let ids: Vec<String> = stmt
                .query_map(params![owner_user_id as i64], |r| r.get(0))
                .map_db()?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_db()?;
            drop(stmt);

            tx.execute(
                "UPDATE remote_sessions SET ended_at_ms = ?1, end_reason = ?2 WHERE owner_user_id = ?3 AND ended_at_ms IS NULL",
                params![now_ms, reason, owner_user_id as i64],
            )
            .map_db()?;
            Ok(ids)
        })
    }

    pub fn list_active_remote_sessions(&self, owner_user_id: u64) -> Result<Vec<RemoteSessionRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT session_id, owner_user_id, controller_user_id, controller_kind,
                           controller_name, controller_platform, controller_id, target_node_id,
                           target_device_id, mode, active_tool, relay_mode, rtt_ms, fps,
                           temp_relay_token_hash, started_at_ms, last_activity_ms,
                           ended_at_ms, end_reason
                    FROM remote_sessions
                    WHERE owner_user_id = ?1 AND ended_at_ms IS NULL
                    ORDER BY started_at_ms DESC
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![owner_user_id as i64], map_remote_session_row)
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn verify_assist_relay_token(
        &self,
        raw_token: &str,
        session_id: &str,
    ) -> Result<Option<RemoteSessionRecord>> {
        let trimmed = raw_token.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let candidate_hash = sha256_hex(trimmed);
        self.pool.with_reader(|conn| {
            let row = conn
                .query_row(
                    r#"
                    SELECT session_id, owner_user_id, controller_user_id, controller_kind,
                           controller_name, controller_platform, controller_id, target_node_id,
                           target_device_id, mode, active_tool, relay_mode, rtt_ms, fps,
                           temp_relay_token_hash, started_at_ms, last_activity_ms,
                           ended_at_ms, end_reason
                    FROM remote_sessions
                    WHERE session_id = ?1 AND ended_at_ms IS NULL
                    LIMIT 1
                    "#,
                    params![session_id.trim()],
                    map_remote_session_row,
                )
                .optional()
                .map_db()?;
            let Some(rec) = row else {
                return Ok(None);
            };
            let Some(ref stored_hash) = rec.temp_relay_token_hash else {
                return Ok(None);
            };
            if !ct_str_eq(&candidate_hash, stored_hash) {
                return Ok(None);
            }
            Ok(Some(rec))
        })
    }
}
