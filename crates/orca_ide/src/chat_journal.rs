//! Event-Sourced SQLite WAL Chat Journal & Chat<->Terminal Continuation (`F12`).
//!
//! Implements `journal_domain_schema_001` with monotonic `epoch`/`sequence`/`fence`
//! ordering, partial-unique single-current submission enforcement
//! (`WHERE current = 1`), `(operation_id, caller_key)` idempotent command receipts,
//! `provider_handle_chain` lineage across `/clear`, fork, and `Cmd+Shift+J`
//! `Terminal <-> Chat` flips, and active-CLI-owner resume protection.

use std::path::Path;

use orca_core::LOCAL_EXECUTION_HOST_ID;
use orca_core::workspace_layout::TabViewMode;
use parking_lot::Mutex;
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{OrcaIdeRuntimeError, Result, canonical_json_sha256, now_epoch_ms};

/// Schema identifier & user_version for the chat journal domain.
pub const JOURNAL_DOMAIN_SCHEMA_ID: &str = "journal_domain_schema_001";
pub const JOURNAL_SCHEMA_VERSION: u32 = 1;
const DEFAULT_RECEIPT_TTL_MS: i64 = 7 * 86_400_000;

const JOURNAL_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_session_records (
    session_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    view_mode TEXT NOT NULL,
    active_cli_owner INTEGER NOT NULL DEFAULT 0,
    provider_handle_chain TEXT NOT NULL DEFAULT '[]',
    adopted_native_id TEXT,
    parent_session_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_sessions_workspace
    ON agent_session_records(workspace_id, updated_at_ms DESC);

CREATE TABLE IF NOT EXISTS agent_session_command_receipts (
    operation_id TEXT NOT NULL,
    caller_key TEXT NOT NULL,
    session_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    response_json TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (operation_id, caller_key)
);

CREATE TABLE IF NOT EXISTS journal_chat_state (
    session_id TEXT PRIMARY KEY REFERENCES agent_session_records(session_id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL DEFAULT 1,
    last_sequence INTEGER NOT NULL DEFAULT 0,
    highest_fence INTEGER NOT NULL DEFAULT 0,
    context_boundary INTEGER NOT NULL DEFAULT 0,
    queue_revision INTEGER NOT NULL DEFAULT 0,
    draft_text TEXT NOT NULL DEFAULT '',
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS journal_items (
    item_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_session_records(session_id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    item_kind TEXT NOT NULL,
    provider_item_id TEXT,
    payload_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_journal_items_session_seq
    ON journal_items(session_id, sequence ASC);

CREATE TABLE IF NOT EXISTS journal_item_tombstones (
    item_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES agent_session_records(session_id) ON DELETE CASCADE,
    deleted_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, item_id)
);

CREATE TABLE IF NOT EXISTS journal_item_aliases (
    alias_id TEXT NOT NULL,
    canonical_item_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES agent_session_records(session_id) ON DELETE CASCADE,
    PRIMARY KEY (session_id, alias_id)
);

CREATE TABLE IF NOT EXISTS journal_submission_attempts (
    attempt_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_session_records(session_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    prompt_text TEXT NOT NULL,
    attachments_json TEXT NOT NULL DEFAULT '[]',
    current INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_journal_submission_current
    ON journal_submission_attempts(session_id) WHERE current = 1;

CREATE TABLE IF NOT EXISTS journal_send_receipts (
    receipt_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_session_records(session_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS journal_events (
    event_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES agent_session_records(session_id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL,
    epoch INTEGER NOT NULL,
    event_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_journal_events_session_seq
    ON journal_events(session_id, sequence ASC);
"#;

/// Persisted `agent_session_records` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionRecord {
    pub session_id: String,
    pub workspace_id: String,
    pub host_id: String,
    pub agent_id: String,
    pub view_mode: TabViewMode,
    pub active_cli_owner: bool,
    pub provider_handle_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Persisted `journal_chat_state` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalChatState {
    pub session_id: String,
    pub epoch: i64,
    pub last_sequence: i64,
    pub highest_fence: i64,
    pub context_boundary: i64,
    pub queue_revision: i64,
    pub draft_text: String,
    pub updated_at_ms: i64,
}

/// Hydrated snapshot combining session record, chat state, and any active submission attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSessionSnapshot {
    pub session: AgentSessionRecord,
    pub chat_state: JournalChatState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_submission: Option<SubmissionAttemptRecord>,
}

/// Persisted `journal_items` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalItemRecord {
    pub item_id: String,
    pub session_id: String,
    pub sequence: i64,
    pub epoch: i64,
    pub item_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_item_id: Option<String>,
    pub payload_json: serde_json::Value,
    pub created_at_ms: i64,
}

/// Lifecycle state of a queued submission attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionStatus {
    Queued,
    Steered,
    Sent,
    Canceled,
}

impl SubmissionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Steered => "steered",
            Self::Sent => "sent",
            Self::Canceled => "canceled",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "queued" => Ok(Self::Queued),
            "steered" => Ok(Self::Steered),
            "sent" => Ok(Self::Sent),
            "canceled" => Ok(Self::Canceled),
            other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "unknown submission status '{other}'"
            ))),
        }
    }
}

/// Action applied to an active submission attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionControlAction {
    Steer,
    Cancel,
}

/// Persisted `journal_submission_attempts` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionAttemptRecord {
    pub attempt_id: String,
    pub session_id: String,
    pub operation_id: String,
    pub prompt_text: String,
    pub attachments: Vec<serde_json::Value>,
    pub current: bool,
    pub status: SubmissionStatus,
    pub created_at_ms: i64,
}

/// Persisted `journal_send_receipts` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendReceiptRecord {
    pub receipt_id: String,
    pub session_id: String,
    pub operation_id: String,
    pub sequence: i64,
    pub created_at_ms: i64,
}

/// Persisted `journal_events` row for deterministic reconnect replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEventRecord {
    pub event_id: String,
    pub session_id: String,
    pub sequence: i64,
    pub epoch: i64,
    pub event_kind: String,
    pub payload_json: serde_json::Value,
    pub created_at_ms: i64,
}

/// Request to create an agent chat/terminal session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentSessionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    pub agent_id: String,
    #[serde(default)]
    pub view_mode: TabViewMode,
    #[serde(default)]
    pub active_cli_owner: bool,
    #[serde(default)]
    pub provider_handle_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Request to append a transcript item into a session journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendJournalItemRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub item_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_item_id: Option<String>,
    pub payload_json: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Request to enqueue a prompt submission with `(operation_id, caller_key)` idempotency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueSubmissionRequest {
    pub session_id: String,
    pub operation_id: String,
    pub caller_key: String,
    pub prompt_text: String,
    #[serde(default)]
    pub attachments: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Thread-safe (`Send + Sync`), WAL-backed SQLite Chat Journal store (`journal_domain_schema_001`).
pub struct ChatJournalStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for ChatJournalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatJournalStore").finish()
    }
}

fn view_mode_to_str(mode: TabViewMode) -> &'static str {
    match mode {
        TabViewMode::Terminal => "terminal",
        TabViewMode::Chat => "chat",
    }
}

fn str_to_view_mode(raw: &str) -> Result<TabViewMode> {
    match raw {
        "terminal" => Ok(TabViewMode::Terminal),
        "chat" => Ok(TabViewMode::Chat),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown tab view mode '{other}'"
        ))),
    }
}

fn init_journal_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    conn.execute_batch(JOURNAL_SCHEMA_SQL)?;
    conn.pragma_update(None, "user_version", JOURNAL_SCHEMA_VERSION)?;
    Ok(())
}

impl ChatJournalStore {
    /// Opens (or creates) a durable WAL SQLite chat journal database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        init_journal_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Opens an isolated in-memory SQLite chat journal store.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_journal_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Creates a new agent session + initial `journal_chat_state` (epoch 1, sequence 0).
    pub fn create_session(&self, req: CreateAgentSessionRequest) -> Result<AgentSessionSnapshot> {
        if req.workspace_id.trim().is_empty() || req.agent_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "workspace_id and agent_id must be non-empty",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let session_id = req
            .session_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("sess-{}", Uuid::new_v4()));
        let host_id = req
            .host_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| LOCAL_EXECUTION_HOST_ID.to_string());
        let chain_json = serde_json::to_string(&req.provider_handle_chain)?;

        let conn = self.conn.lock();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM agent_session_records WHERE session_id = ?1",
                params![&session_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if exists {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "session '{session_id}' already exists"
            )));
        }

        conn.execute(
            "INSERT INTO agent_session_records
             (session_id, workspace_id, host_id, agent_id, view_mode, active_cli_owner,
              provider_handle_chain, adopted_native_id, parent_session_id, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?9)",
            params![
                &session_id,
                req.workspace_id.trim(),
                &host_id,
                req.agent_id.trim(),
                view_mode_to_str(req.view_mode),
                i32::from(req.active_cli_owner),
                &chain_json,
                &req.adopted_native_id,
                now_ms
            ],
        )?;
        conn.execute(
            "INSERT INTO journal_chat_state
             (session_id, epoch, last_sequence, highest_fence, context_boundary,
              queue_revision, draft_text, updated_at_ms)
             VALUES (?1, 1, 0, 0, 0, 0, '', ?2)",
            params![&session_id, now_ms],
        )?;
        Self::record_event(
            &conn,
            &session_id,
            0,
            1,
            "session_created",
            &serde_json::json!({
                "agent_id": req.agent_id.trim(),
                "view_mode": view_mode_to_str(req.view_mode),
            }),
            now_ms,
        )?;
        Self::load_snapshot(&conn, &session_id)
    }

    /// Fetches the full session snapshot (record + chat state + current submission attempt).
    pub fn get_session(&self, session_id: &str) -> Result<AgentSessionSnapshot> {
        let conn = self.conn.lock();
        Self::load_snapshot(&conn, session_id)
    }

    /// Lists agent session records, optionally filtered by `workspace_id`.
    pub fn list_sessions(&self, workspace_id: Option<&str>) -> Result<Vec<AgentSessionRecord>> {
        let conn = self.conn.lock();
        let rows = if let Some(ws) = workspace_id {
            let mut stmt = conn.prepare(
                "SELECT session_id, workspace_id, host_id, agent_id, view_mode, active_cli_owner,
                        provider_handle_chain, adopted_native_id, parent_session_id,
                        created_at_ms, updated_at_ms
                 FROM agent_session_records
                 WHERE workspace_id = ?1
                 ORDER BY updated_at_ms DESC, session_id ASC",
            )?;
            stmt.query_map(params![ws], Self::map_session_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT session_id, workspace_id, host_id, agent_id, view_mode, active_cli_owner,
                        provider_handle_chain, adopted_native_id, parent_session_id,
                        created_at_ms, updated_at_ms
                 FROM agent_session_records
                 ORDER BY updated_at_ms DESC, session_id ASC",
            )?;
            stmt.query_map([], Self::map_session_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(Self::decode_session_row(row)?);
        }
        Ok(out)
    }

    /// Sets or clears `active_cli_owner` on a session (guards `resume_in_new_session`).
    pub fn set_active_cli_owner(
        &self,
        session_id: &str,
        active: bool,
        now_ms: Option<i64>,
    ) -> Result<AgentSessionRecord> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let snap = Self::load_snapshot(&conn, session_id)?;
        conn.execute(
            "UPDATE agent_session_records SET active_cli_owner = ?1, updated_at_ms = ?2
             WHERE session_id = ?3",
            params![i32::from(active), now_ms, session_id],
        )?;
        Self::record_event(
            &conn,
            session_id,
            snap.chat_state.last_sequence,
            snap.chat_state.epoch,
            "active_cli_owner_changed",
            &serde_json::json!({ "active_cli_owner": active }),
            now_ms,
        )?;
        Ok(Self::load_snapshot(&conn, session_id)?.session)
    }
}

impl ChatJournalStore {
    /// Appends a journal item with monotonic sequence and fence update.
    pub fn append_item(&self, req: AppendJournalItemRequest) -> Result<JournalItemRecord> {
        if req.item_kind.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "item_kind cannot be empty",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let item_id = req
            .item_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("item-{}", Uuid::new_v4()));
        let payload_str = serde_json::to_string(&req.payload_json)?;

        let conn = self.conn.lock();
        let snap = Self::load_snapshot(&conn, &req.session_id)?;
        let next_seq = snap.chat_state.last_sequence + 1;
        let next_fence = snap.chat_state.highest_fence.max(next_seq);
        let epoch = snap.chat_state.epoch;

        conn.execute(
            "INSERT INTO journal_items
             (item_id, session_id, sequence, epoch, item_kind, provider_item_id, payload_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &item_id,
                &req.session_id,
                next_seq,
                epoch,
                req.item_kind.trim(),
                &req.provider_item_id,
                &payload_str,
                now_ms
            ],
        )?;
        conn.execute(
            "UPDATE journal_chat_state
             SET last_sequence = ?1, highest_fence = ?2, updated_at_ms = ?3
             WHERE session_id = ?4",
            params![next_seq, next_fence, now_ms, &req.session_id],
        )?;
        conn.execute(
            "UPDATE agent_session_records SET updated_at_ms = ?1 WHERE session_id = ?2",
            params![now_ms, &req.session_id],
        )?;
        Self::record_event(
            &conn,
            &req.session_id,
            next_seq,
            epoch,
            "item_appended",
            &serde_json::json!({
                "item_id": &item_id,
                "item_kind": req.item_kind.trim(),
                "sequence": next_seq,
            }),
            now_ms,
        )?;

        Ok(JournalItemRecord {
            item_id,
            session_id: req.session_id,
            sequence: next_seq,
            epoch,
            item_kind: req.item_kind.trim().to_string(),
            provider_item_id: req.provider_item_id,
            payload_json: req.payload_json,
            created_at_ms: now_ms,
        })
    }

    /// Lists journal items for a session in sequence order (excluding tombstones by default).
    pub fn list_items(
        &self,
        session_id: &str,
        include_tombstoned: bool,
    ) -> Result<Vec<JournalItemRecord>> {
        let conn = self.conn.lock();
        let _snap = Self::load_snapshot(&conn, session_id)?;
        let sql = if include_tombstoned {
            "SELECT item_id, session_id, sequence, epoch, item_kind, provider_item_id,
                    payload_json, created_at_ms
             FROM journal_items
             WHERE session_id = ?1
             ORDER BY sequence ASC"
        } else {
            "SELECT i.item_id, i.session_id, i.sequence, i.epoch, i.item_kind, i.provider_item_id,
                    i.payload_json, i.created_at_ms
             FROM journal_items i
             LEFT JOIN journal_item_tombstones t
               ON t.session_id = i.session_id AND t.item_id = i.item_id
             WHERE i.session_id = ?1 AND t.item_id IS NULL
             ORDER BY i.sequence ASC"
        };
        let mut stmt = conn.prepare(sql)?;
        let raw = stmt
            .query_map(params![session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut items = Vec::with_capacity(raw.len());
        for r in raw {
            items.push(JournalItemRecord {
                item_id: r.0,
                session_id: r.1,
                sequence: r.2,
                epoch: r.3,
                item_kind: r.4,
                provider_item_id: r.5,
                payload_json: serde_json::from_str(&r.6)?,
                created_at_ms: r.7,
            });
        }
        Ok(items)
    }

    /// Marks an item as tombstoned without rewriting historical sequences.
    pub fn tombstone_item(
        &self,
        session_id: &str,
        item_id: &str,
        now_ms: Option<i64>,
    ) -> Result<()> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let snap = Self::load_snapshot(&conn, session_id)?;
        conn.execute(
            "INSERT OR REPLACE INTO journal_item_tombstones (item_id, session_id, deleted_at_ms)
             VALUES (?1, ?2, ?3)",
            params![item_id, session_id, now_ms],
        )?;
        Self::record_event(
            &conn,
            session_id,
            snap.chat_state.last_sequence,
            snap.chat_state.epoch,
            "item_tombstoned",
            &serde_json::json!({ "item_id": item_id }),
            now_ms,
        )?;
        Ok(())
    }

    /// Maps a temporary or provider alias ID to a canonical `journal_items.item_id`.
    pub fn add_item_alias(
        &self,
        session_id: &str,
        alias_id: &str,
        canonical_item_id: &str,
    ) -> Result<()> {
        if alias_id.trim().is_empty() || canonical_item_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "alias_id and canonical_item_id must be non-empty",
            ));
        }
        let conn = self.conn.lock();
        let _snap = Self::load_snapshot(&conn, session_id)?;
        conn.execute(
            "INSERT OR REPLACE INTO journal_item_aliases (alias_id, canonical_item_id, session_id)
             VALUES (?1, ?2, ?3)",
            params![alias_id.trim(), canonical_item_id.trim(), session_id],
        )?;
        Ok(())
    }

    /// Resolves an item alias to its canonical item ID if present.
    pub fn resolve_item_alias(&self, session_id: &str, alias_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let resolved: Option<String> = conn
            .query_row(
                "SELECT canonical_item_id FROM journal_item_aliases
                 WHERE session_id = ?1 AND alias_id = ?2",
                params![session_id, alias_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(resolved)
    }

    /// Saves unsent draft text into `journal_chat_state.draft_text` so reconnects preserve it.
    pub fn save_draft(
        &self,
        session_id: &str,
        draft_text: &str,
        now_ms: Option<i64>,
    ) -> Result<JournalChatState> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let _snap = Self::load_snapshot(&conn, session_id)?;
        conn.execute(
            "UPDATE journal_chat_state SET draft_text = ?1, updated_at_ms = ?2
             WHERE session_id = ?3",
            params![draft_text, now_ms, session_id],
        )?;
        Ok(Self::load_snapshot(&conn, session_id)?.chat_state)
    }
}

impl ChatJournalStore {
    /// Enqueues a submission attempt with strict `(operation_id, caller_key)` idempotency via
    /// `agent_session_command_receipts` and enforces the SQLite partial unique index
    /// `idx_journal_submission_current` (`WHERE current = 1`).
    pub fn enqueue_submission(
        &self,
        req: EnqueueSubmissionRequest,
    ) -> Result<(SubmissionAttemptRecord, bool)> {
        if req.operation_id.trim().is_empty() || req.caller_key.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "operation_id and caller_key must be non-empty",
            ));
        }
        if req.prompt_text.trim().is_empty() && req.attachments.is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "submission must include prompt_text or attachments",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let payload_hash = canonical_json_sha256(&serde_json::json!({
            "session_id": &req.session_id,
            "prompt_text": &req.prompt_text,
            "attachments": &req.attachments,
        }));

        let conn = self.conn.lock();
        let existing_receipt: Option<(String, String)> = conn
            .query_row(
                "SELECT payload_hash, response_json FROM agent_session_command_receipts
                 WHERE operation_id = ?1 AND caller_key = ?2",
                params![&req.operation_id, &req.caller_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((stored_hash, stored_json)) = existing_receipt {
            if stored_hash != payload_hash {
                return Err(OrcaIdeRuntimeError::conflict(format!(
                    "command receipt ({}, {}) reused with different payload",
                    req.operation_id, req.caller_key
                )));
            }
            let cached: SubmissionAttemptRecord = serde_json::from_str(&stored_json)?;
            return Ok((cached, true));
        }

        let snap = Self::load_snapshot(&conn, &req.session_id)?;
        let attempt_id = format!("sub-{}", Uuid::new_v4());
        let attachments_json = serde_json::to_string(&req.attachments)?;

        let insert_res = conn.execute(
            "INSERT INTO journal_submission_attempts
             (attempt_id, session_id, operation_id, prompt_text, attachments_json,
              current, status, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, 'queued', ?6)",
            params![
                &attempt_id,
                &req.session_id,
                &req.operation_id,
                &req.prompt_text,
                &attachments_json,
                now_ms
            ],
        );
        if let Err(err) = insert_res {
            if let rusqlite::Error::SqliteFailure(sqlite_err, _) = &err
                && sqlite_err.code == ErrorCode::ConstraintViolation
            {
                return Err(OrcaIdeRuntimeError::conflict(format!(
                    "session '{}' already has a current submission attempt",
                    req.session_id
                )));
            }
            return Err(err.into());
        }

        conn.execute(
            "UPDATE journal_chat_state
             SET queue_revision = queue_revision + 1, updated_at_ms = ?1
             WHERE session_id = ?2",
            params![now_ms, &req.session_id],
        )?;

        let record = SubmissionAttemptRecord {
            attempt_id: attempt_id.clone(),
            session_id: req.session_id.clone(),
            operation_id: req.operation_id.clone(),
            prompt_text: req.prompt_text,
            attachments: req.attachments,
            current: true,
            status: SubmissionStatus::Queued,
            created_at_ms: now_ms,
        };
        let response_json = serde_json::to_string(&record)?;
        conn.execute(
            "INSERT INTO agent_session_command_receipts
             (operation_id, caller_key, session_id, payload_hash, response_json, expires_at, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &req.operation_id,
                &req.caller_key,
                &req.session_id,
                &payload_hash,
                &response_json,
                now_ms.saturating_add(DEFAULT_RECEIPT_TTL_MS),
                now_ms
            ],
        )?;
        Self::record_event(
            &conn,
            &req.session_id,
            snap.chat_state.last_sequence,
            snap.chat_state.epoch,
            "submission_enqueued",
            &serde_json::json!({
                "attempt_id": &attempt_id,
                "operation_id": &req.operation_id,
            }),
            now_ms,
        )?;

        Ok((record, false))
    }

    /// Steers (updates prompt in-flight) or cancels an active submission attempt.
    /// Canceling sets `current = 0`, releasing the partial-unique slot for subsequent submissions.
    pub fn steer_or_cancel_submission(
        &self,
        session_id: &str,
        attempt_id: &str,
        action: SubmissionControlAction,
        new_prompt: Option<String>,
        now_ms: Option<i64>,
    ) -> Result<SubmissionAttemptRecord> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let snap = Self::load_snapshot(&conn, session_id)?;
        let mut attempt = Self::load_attempt(&conn, session_id, attempt_id)?;
        if !attempt.current {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "submission attempt '{attempt_id}' is no longer current"
            )));
        }

        match action {
            SubmissionControlAction::Steer => {
                if let Some(prompt) = new_prompt {
                    attempt.prompt_text = prompt;
                }
                attempt.status = SubmissionStatus::Steered;
                conn.execute(
                    "UPDATE journal_submission_attempts
                     SET prompt_text = ?1, status = 'steered'
                     WHERE attempt_id = ?2 AND session_id = ?3",
                    params![&attempt.prompt_text, attempt_id, session_id],
                )?;
            }
            SubmissionControlAction::Cancel => {
                attempt.status = SubmissionStatus::Canceled;
                attempt.current = false;
                conn.execute(
                    "UPDATE journal_submission_attempts
                     SET current = 0, status = 'canceled'
                     WHERE attempt_id = ?1 AND session_id = ?2",
                    params![attempt_id, session_id],
                )?;
            }
        }
        conn.execute(
            "UPDATE journal_chat_state
             SET queue_revision = queue_revision + 1, updated_at_ms = ?1
             WHERE session_id = ?2",
            params![now_ms, session_id],
        )?;
        Self::record_event(
            &conn,
            session_id,
            snap.chat_state.last_sequence,
            snap.chat_state.epoch,
            match action {
                SubmissionControlAction::Steer => "submission_steered",
                SubmissionControlAction::Cancel => "submission_canceled",
            },
            &serde_json::json!({ "attempt_id": attempt_id }),
            now_ms,
        )?;
        Ok(attempt)
    }

    /// Marks the active submission attempt as `Sent`, releases `current = 0`, increments
    /// `last_sequence`, clears matching draft text, and writes a durable `journal_send_receipts` row.
    pub fn complete_submission(
        &self,
        session_id: &str,
        attempt_id: &str,
        now_ms: Option<i64>,
    ) -> Result<SendReceiptRecord> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let snap = Self::load_snapshot(&conn, session_id)?;
        let attempt = Self::load_attempt(&conn, session_id, attempt_id)?;
        if !attempt.current {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "submission attempt '{attempt_id}' is not current"
            )));
        }

        let next_seq = snap.chat_state.last_sequence + 1;
        let next_fence = snap.chat_state.highest_fence.max(next_seq);
        let receipt_id = format!("rcpt-{}", Uuid::new_v4());

        conn.execute(
            "UPDATE journal_submission_attempts
             SET current = 0, status = 'sent'
             WHERE attempt_id = ?1 AND session_id = ?2",
            params![attempt_id, session_id],
        )?;
        conn.execute(
            "INSERT INTO journal_send_receipts
             (receipt_id, session_id, operation_id, sequence, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&receipt_id, session_id, &attempt.operation_id, next_seq, now_ms],
        )?;
        conn.execute(
            "UPDATE journal_chat_state
             SET last_sequence = ?1, highest_fence = ?2, queue_revision = queue_revision + 1,
                 draft_text = CASE WHEN draft_text = ?3 THEN '' ELSE draft_text END,
                 updated_at_ms = ?4
             WHERE session_id = ?5",
            params![next_seq, next_fence, &attempt.prompt_text, now_ms, session_id],
        )?;
        Self::record_event(
            &conn,
            session_id,
            next_seq,
            snap.chat_state.epoch,
            "submission_completed",
            &serde_json::json!({
                "attempt_id": attempt_id,
                "receipt_id": &receipt_id,
                "sequence": next_seq,
            }),
            now_ms,
        )?;

        Ok(SendReceiptRecord {
            receipt_id,
            session_id: session_id.to_string(),
            operation_id: attempt.operation_id,
            sequence: next_seq,
            created_at_ms: now_ms,
        })
    }
}

impl ChatJournalStore {
    /// Records a `/clear` context boundary: bumps `epoch`, updates `context_boundary`,
    /// appends `new_provider_handle` to `provider_handle_chain`, and emits a journal event.
    pub fn clear_context_boundary(
        &self,
        session_id: &str,
        new_provider_handle: Option<String>,
        now_ms: Option<i64>,
    ) -> Result<AgentSessionSnapshot> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let snap = Self::load_snapshot(&conn, session_id)?;
        let next_epoch = snap.chat_state.epoch + 1;
        let boundary = snap.chat_state.last_sequence;

        let mut chain = snap.session.provider_handle_chain;
        if let Some(handle) = new_provider_handle
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            && chain.last().map(String::as_str) != Some(handle.as_str())
        {
            chain.push(handle);
        }
        let chain_json = serde_json::to_string(&chain)?;

        conn.execute(
            "UPDATE journal_chat_state
             SET epoch = ?1, context_boundary = ?2, updated_at_ms = ?3
             WHERE session_id = ?4",
            params![next_epoch, boundary, now_ms, session_id],
        )?;
        conn.execute(
            "UPDATE agent_session_records
             SET provider_handle_chain = ?1, updated_at_ms = ?2
             WHERE session_id = ?3",
            params![&chain_json, now_ms, session_id],
        )?;
        Self::record_event(
            &conn,
            session_id,
            boundary,
            next_epoch,
            "context_boundary_cleared",
            &serde_json::json!({
                "epoch": next_epoch,
                "context_boundary": boundary,
                "provider_handle_chain": &chain,
            }),
            now_ms,
        )?;
        Self::load_snapshot(&conn, session_id)
    }

    /// Toggles `TabViewMode::Terminal <-> TabViewMode::Chat` in-place (`Cmd+Shift+J`),
    /// preserving the entire `provider_handle_chain` (and appending an optional newly adopted handle).
    pub fn toggle_view_mode(
        &self,
        session_id: &str,
        new_provider_handle: Option<String>,
        now_ms: Option<i64>,
    ) -> Result<AgentSessionSnapshot> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let snap = Self::load_snapshot(&conn, session_id)?;
        let next_mode = match snap.session.view_mode {
            TabViewMode::Terminal => TabViewMode::Chat,
            TabViewMode::Chat => TabViewMode::Terminal,
        };
        let mut chain = snap.session.provider_handle_chain;
        if let Some(handle) = new_provider_handle
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            && chain.last().map(String::as_str) != Some(handle.as_str())
        {
            chain.push(handle);
        }
        let chain_json = serde_json::to_string(&chain)?;

        conn.execute(
            "UPDATE agent_session_records
             SET view_mode = ?1, provider_handle_chain = ?2, updated_at_ms = ?3
             WHERE session_id = ?4",
            params![view_mode_to_str(next_mode), &chain_json, now_ms, session_id],
        )?;
        Self::record_event(
            &conn,
            session_id,
            snap.chat_state.last_sequence,
            snap.chat_state.epoch,
            "view_mode_toggled",
            &serde_json::json!({
                "view_mode": view_mode_to_str(next_mode),
                "provider_handle_chain": &chain,
            }),
            now_ms,
        )?;
        Self::load_snapshot(&conn, session_id)
    }

    /// Implements `Resume in New Chat | Terminal`, preserving `provider_handle_chain` lineage.
    /// MUST refuse with `OrcaIdeRuntimeError::ActiveCliOwner` whenever `active_cli_owner == true`
    /// until the active CLI process is stopped/exited!
    pub fn resume_in_new_session(
        &self,
        source_session_id: &str,
        target_view_mode: TabViewMode,
        new_session_id: Option<String>,
        now_ms: Option<i64>,
    ) -> Result<AgentSessionSnapshot> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let src = Self::load_snapshot(&conn, source_session_id)?;
        if src.session.active_cli_owner {
            return Err(OrcaIdeRuntimeError::active_cli_owner(format!(
                "cannot resume session '{}' while an active CLI owner is still running; stop or exit the CLI first",
                source_session_id
            )));
        }

        let target_id = new_session_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("sess-{}", Uuid::new_v4()));
        let chain_json = serde_json::to_string(&src.session.provider_handle_chain)?;

        conn.execute(
            "INSERT INTO agent_session_records
             (session_id, workspace_id, host_id, agent_id, view_mode, active_cli_owner,
              provider_handle_chain, adopted_native_id, parent_session_id, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?9)",
            params![
                &target_id,
                &src.session.workspace_id,
                &src.session.host_id,
                &src.session.agent_id,
                view_mode_to_str(target_view_mode),
                &chain_json,
                &src.session.adopted_native_id,
                source_session_id,
                now_ms
            ],
        )?;
        conn.execute(
            "INSERT INTO journal_chat_state
             (session_id, epoch, last_sequence, highest_fence, context_boundary,
              queue_revision, draft_text, updated_at_ms)
             VALUES (?1, 1, 0, 0, 0, 0, '', ?2)",
            params![&target_id, now_ms],
        )?;
        Self::record_event(
            &conn,
            &target_id,
            0,
            1,
            "session_resumed",
            &serde_json::json!({
                "source_session_id": source_session_id,
                "view_mode": view_mode_to_str(target_view_mode),
                "provider_handle_chain": &src.session.provider_handle_chain,
            }),
            now_ms,
        )?;
        Self::load_snapshot(&conn, &target_id)
    }

    /// Forks a session (optionally up to `up_to_sequence`), cloning non-tombstoned transcript
    /// items and `provider_handle_chain` into a new independent session.
    pub fn fork_session(
        &self,
        source_session_id: &str,
        up_to_sequence: Option<i64>,
        new_session_id: Option<String>,
        now_ms: Option<i64>,
    ) -> Result<AgentSessionSnapshot> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let src = Self::load_snapshot(&conn, source_session_id)?;
        let fork_id = new_session_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("sess-fork-{}", Uuid::new_v4()));
        let chain_json = serde_json::to_string(&src.session.provider_handle_chain)?;

        conn.execute(
            "INSERT INTO agent_session_records
             (session_id, workspace_id, host_id, agent_id, view_mode, active_cli_owner,
              provider_handle_chain, adopted_native_id, parent_session_id, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?9)",
            params![
                &fork_id,
                &src.session.workspace_id,
                &src.session.host_id,
                &src.session.agent_id,
                view_mode_to_str(src.session.view_mode),
                &chain_json,
                &src.session.adopted_native_id,
                source_session_id,
                now_ms
            ],
        )?;

        let max_seq = up_to_sequence.unwrap_or(i64::MAX);
        let mut stmt = conn.prepare(
            "SELECT i.sequence, i.epoch, i.item_kind, i.provider_item_id, i.payload_json, i.created_at_ms
             FROM journal_items i
             LEFT JOIN journal_item_tombstones t
               ON t.session_id = i.session_id AND t.item_id = i.item_id
             WHERE i.session_id = ?1 AND t.item_id IS NULL AND i.sequence <= ?2
             ORDER BY i.sequence ASC",
        )?;
        let items = stmt
            .query_map(params![source_session_id, max_seq], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut last_seq = 0_i64;
        let mut max_epoch = 1_i64;
        for (idx, (_orig_seq, epoch, kind, provider_item_id, payload_json, created_at)) in
            items.into_iter().enumerate()
        {
            let seq = i64::try_from(idx + 1).unwrap_or(1);
            last_seq = seq;
            max_epoch = max_epoch.max(epoch);
            let new_item_id = format!("item-{}", Uuid::new_v4());
            conn.execute(
                "INSERT INTO journal_items
                 (item_id, session_id, sequence, epoch, item_kind, provider_item_id, payload_json, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    &new_item_id,
                    &fork_id,
                    seq,
                    epoch,
                    &kind,
                    &provider_item_id,
                    &payload_json,
                    created_at
                ],
            )?;
        }

        conn.execute(
            "INSERT INTO journal_chat_state
             (session_id, epoch, last_sequence, highest_fence, context_boundary,
              queue_revision, draft_text, updated_at_ms)
             VALUES (?1, ?2, ?3, ?3, 0, 0, '', ?4)",
            params![&fork_id, max_epoch, last_seq, now_ms],
        )?;
        Self::record_event(
            &conn,
            &fork_id,
            last_seq,
            max_epoch,
            "session_forked",
            &serde_json::json!({
                "source_session_id": source_session_id,
                "copied_items": last_seq,
            }),
            now_ms,
        )?;
        Self::load_snapshot(&conn, &fork_id)
    }

    /// Replays durable journal events strictly after `after_sequence` in deterministic order.
    pub fn replay_events_since(
        &self,
        session_id: &str,
        after_sequence: i64,
        limit: Option<usize>,
    ) -> Result<Vec<JournalEventRecord>> {
        let conn = self.conn.lock();
        let _snap = Self::load_snapshot(&conn, session_id)?;
        let cap = i64::try_from(limit.unwrap_or(1_000)).unwrap_or(1_000);
        let mut stmt = conn.prepare(
            "SELECT event_id, session_id, sequence, epoch, event_kind, payload_json, created_at_ms
             FROM journal_events
             WHERE session_id = ?1 AND sequence > ?2
             ORDER BY sequence ASC, rowid ASC
             LIMIT ?3",
        )?;
        let raw = stmt
            .query_map(params![session_id, after_sequence, cap], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut events = Vec::with_capacity(raw.len());
        for r in raw {
            events.push(JournalEventRecord {
                event_id: r.0,
                session_id: r.1,
                sequence: r.2,
                epoch: r.3,
                event_kind: r.4,
                payload_json: serde_json::from_str(&r.5)?,
                created_at_ms: r.6,
            });
        }
        Ok(events)
    }
}

impl ChatJournalStore {
    fn record_event(
        conn: &Connection,
        session_id: &str,
        sequence: i64,
        epoch: i64,
        event_kind: &str,
        payload: &serde_json::Value,
        now_ms: i64,
    ) -> Result<()> {
        let event_id = format!("ev-{}", Uuid::new_v4());
        let payload_str = serde_json::to_string(payload)?;
        conn.execute(
            "INSERT INTO journal_events
             (event_id, session_id, sequence, epoch, event_kind, payload_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![&event_id, session_id, sequence, epoch, event_kind, &payload_str, now_ms],
        )?;
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn map_session_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<(
        String,
        String,
        String,
        String,
        String,
        i64,
        String,
        Option<String>,
        Option<String>,
        i64,
        i64,
    )> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
        ))
    }

    #[allow(clippy::type_complexity)]
    fn decode_session_row(
        row: (
            String,
            String,
            String,
            String,
            String,
            i64,
            String,
            Option<String>,
            Option<String>,
            i64,
            i64,
        ),
    ) -> Result<AgentSessionRecord> {
        let chain: Vec<String> = serde_json::from_str(&row.6)?;
        Ok(AgentSessionRecord {
            session_id: row.0,
            workspace_id: row.1,
            host_id: row.2,
            agent_id: row.3,
            view_mode: str_to_view_mode(&row.4)?,
            active_cli_owner: row.5 != 0,
            provider_handle_chain: chain,
            adopted_native_id: row.7,
            parent_session_id: row.8,
            created_at_ms: row.9,
            updated_at_ms: row.10,
        })
    }

    fn load_attempt(
        conn: &Connection,
        session_id: &str,
        attempt_id: &str,
    ) -> Result<SubmissionAttemptRecord> {
        let row = conn
            .query_row(
                "SELECT attempt_id, session_id, operation_id, prompt_text, attachments_json,
                        current, status, created_at_ms
                 FROM journal_submission_attempts
                 WHERE session_id = ?1 AND attempt_id = ?2",
                params![session_id, attempt_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!(
                    "unknown submission attempt '{attempt_id}' for session '{session_id}'"
                ))
            })?;
        Ok(SubmissionAttemptRecord {
            attempt_id: row.0,
            session_id: row.1,
            operation_id: row.2,
            prompt_text: row.3,
            attachments: serde_json::from_str(&row.4)?,
            current: row.5 != 0,
            status: SubmissionStatus::parse(&row.6)?,
            created_at_ms: row.7,
        })
    }

    fn load_snapshot(conn: &Connection, session_id: &str) -> Result<AgentSessionSnapshot> {
        let raw_session = conn
            .query_row(
                "SELECT session_id, workspace_id, host_id, agent_id, view_mode, active_cli_owner,
                        provider_handle_chain, adopted_native_id, parent_session_id,
                        created_at_ms, updated_at_ms
                 FROM agent_session_records WHERE session_id = ?1",
                params![session_id],
                Self::map_session_row,
            )
            .optional()?
            .ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!("unknown agent session '{session_id}'"))
            })?;
        let session = Self::decode_session_row(raw_session)?;

        let chat_state = conn.query_row(
            "SELECT session_id, epoch, last_sequence, highest_fence, context_boundary,
                    queue_revision, draft_text, updated_at_ms
             FROM journal_chat_state WHERE session_id = ?1",
            params![session_id],
            |row| {
                Ok(JournalChatState {
                    session_id: row.get(0)?,
                    epoch: row.get(1)?,
                    last_sequence: row.get(2)?,
                    highest_fence: row.get(3)?,
                    context_boundary: row.get(4)?,
                    queue_revision: row.get(5)?,
                    draft_text: row.get(6)?,
                    updated_at_ms: row.get(7)?,
                })
            },
        )?;

        let current_submission = conn
            .query_row(
                "SELECT attempt_id, session_id, operation_id, prompt_text, attachments_json,
                        current, status, created_at_ms
                 FROM journal_submission_attempts
                 WHERE session_id = ?1 AND current = 1",
                params![session_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;

        let current_submission = match current_submission {
            Some(row) => Some(SubmissionAttemptRecord {
                attempt_id: row.0,
                session_id: row.1,
                operation_id: row.2,
                prompt_text: row.3,
                attachments: serde_json::from_str(&row.4)?,
                current: row.5 != 0,
                status: SubmissionStatus::parse(&row.6)?,
                created_at_ms: row.7,
            }),
            None => None,
        };

        Ok(AgentSessionSnapshot {
            session,
            chat_state,
            current_submission,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_is_send_sync_and_enforces_partial_unique_current_submission() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ChatJournalStore>();

        let dir = tempfile::tempdir().expect("tempdir");
        let db = ChatJournalStore::open(dir.path().join("journal.sqlite")).expect("open");

        let snap = db
            .create_session(CreateAgentSessionRequest {
                session_id: Some("sess-1".into()),
                workspace_id: "ws-1".into(),
                host_id: None,
                agent_id: "claude".into(),
                view_mode: TabViewMode::Chat,
                active_cli_owner: false,
                provider_handle_chain: vec!["claude-h1".into()],
                adopted_native_id: None,
                now_ms: Some(1_000),
            })
            .expect("create_session");
        assert_eq!(snap.chat_state.epoch, 1);

        // First submission succeeds and claims `current = 1`.
        let (sub1, replayed1) = db
            .enqueue_submission(EnqueueSubmissionRequest {
                session_id: "sess-1".into(),
                operation_id: "op-1".into(),
                caller_key: "desktop-a".into(),
                prompt_text: "Implement feature".into(),
                attachments: vec![],
                now_ms: Some(1_010),
            })
            .expect("enqueue 1");
        assert!(!replayed1);
        assert!(sub1.current);

        // Exact replay of `(operation_id, caller_key)` returns cached attempt idempotently.
        let (sub1_replay, replayed2) = db
            .enqueue_submission(EnqueueSubmissionRequest {
                session_id: "sess-1".into(),
                operation_id: "op-1".into(),
                caller_key: "desktop-a".into(),
                prompt_text: "Implement feature".into(),
                attachments: vec![],
                now_ms: Some(1_011),
            })
            .expect("replay 1");
        assert!(replayed2);
        assert_eq!(sub1_replay.attempt_id, sub1.attempt_id);

        // Second distinct submission while first is still current hits partial unique index.
        let err = db.enqueue_submission(EnqueueSubmissionRequest {
            session_id: "sess-1".into(),
            operation_id: "op-2".into(),
            caller_key: "desktop-a".into(),
            prompt_text: "Competing prompt".into(),
            attachments: vec![],
            now_ms: Some(1_012),
        });
        assert_eq!(err.expect_err("partial unique conflict").code_str(), "conflict");

        // Steer keeps current = 1, then complete releases slot (`current = 0`).
        let steered = db
            .steer_or_cancel_submission(
                "sess-1",
                &sub1.attempt_id,
                SubmissionControlAction::Steer,
                Some("Implement feature with tests".into()),
                Some(1_015),
            )
            .expect("steer");
        assert_eq!(steered.status, SubmissionStatus::Steered);
        assert!(steered.current);

        let receipt = db
            .complete_submission("sess-1", &sub1.attempt_id, Some(1_020))
            .expect("complete");
        assert_eq!(receipt.sequence, 1);

        // Now a new submission succeeds!
        let (sub2, _) = db
            .enqueue_submission(EnqueueSubmissionRequest {
                session_id: "sess-1".into(),
                operation_id: "op-3".into(),
                caller_key: "desktop-a".into(),
                prompt_text: "Next turn".into(),
                attachments: vec![],
                now_ms: Some(1_030),
            })
            .expect("enqueue 2");
        assert!(sub2.current);
        let canceled = db
            .steer_or_cancel_submission(
                "sess-1",
                &sub2.attempt_id,
                SubmissionControlAction::Cancel,
                None,
                Some(1_035),
            )
            .expect("cancel");
        assert!(!canceled.current);
        assert_eq!(canceled.status, SubmissionStatus::Canceled);
    }

    #[test]
    fn provider_handle_chain_flip_resume_active_owner_guard_and_fork() {
        let db = ChatJournalStore::in_memory().expect("in_memory");
        db.create_session(CreateAgentSessionRequest {
            session_id: Some("sess-cli".into()),
            workspace_id: "ws-main".into(),
            host_id: Some("local".into()),
            agent_id: "codex".into(),
            view_mode: TabViewMode::Terminal,
            active_cli_owner: true,
            provider_handle_chain: vec!["codex-h1".into()],
            adopted_native_id: Some("native-1".into()),
            now_ms: Some(2_000),
        })
        .expect("create");

        // Append items, alias, tombstone, and save draft.
        let i1 = db
            .append_item(AppendJournalItemRequest {
                session_id: "sess-cli".into(),
                item_id: Some("it-1".into()),
                item_kind: "user_message".into(),
                provider_item_id: Some("p-1".into()),
                payload_json: serde_json::json!({"text": "hello"}),
                now_ms: Some(2_001),
            })
            .expect("item 1");
        let i2 = db
            .append_item(AppendJournalItemRequest {
                session_id: "sess-cli".into(),
                item_id: Some("it-2".into()),
                item_kind: "assistant_message".into(),
                provider_item_id: None,
                payload_json: serde_json::json!({"text": "world"}),
                now_ms: Some(2_002),
            })
            .expect("item 2");
        db.add_item_alias("sess-cli", "temp-1", &i1.item_id)
            .expect("alias");
        assert_eq!(
            db.resolve_item_alias("sess-cli", "temp-1").expect("resolve"),
            Some("it-1".to_string())
        );
        db.tombstone_item("sess-cli", &i2.item_id, Some(2_003))
            .expect("tombstone");
        assert_eq!(db.list_items("sess-cli", false).expect("items").len(), 1);
        db.save_draft("sess-cli", "unfinished draft", Some(2_004))
            .expect("draft");

        // `/clear` bumps epoch and appends new provider handle to chain.
        let cleared = db
            .clear_context_boundary("sess-cli", Some("codex-h2".into()), Some(2_010))
            .expect("clear");
        assert_eq!(cleared.chat_state.epoch, 2);
        assert_eq!(cleared.chat_state.context_boundary, 2);
        assert_eq!(
            cleared.session.provider_handle_chain,
            vec!["codex-h1", "codex-h2"]
        );

        // `Cmd+Shift+J` in-place Terminal -> Chat flip preserves handle chain.
        let flipped = db
            .toggle_view_mode("sess-cli", None, Some(2_020))
            .expect("flip");
        assert_eq!(flipped.session.view_mode, TabViewMode::Chat);
        assert_eq!(
            flipped.session.provider_handle_chain,
            vec!["codex-h1", "codex-h2"]
        );
        assert_eq!(flipped.chat_state.draft_text, "unfinished draft");

        // Resume MUST refuse while `active_cli_owner == true`!
        let refused = db.resume_in_new_session("sess-cli", TabViewMode::Chat, None, Some(2_030));
        assert_eq!(
            refused.expect_err("active owner refused").code_str(),
            "active_cli_owner"
        );

        // Once CLI exits (`active_cli_owner = false`), resume succeeds preserving chain.
        db.set_active_cli_owner("sess-cli", false, Some(2_031))
            .expect("clear owner");
        let resumed = db
            .resume_in_new_session(
                "sess-cli",
                TabViewMode::Terminal,
                Some("sess-resumed".into()),
                Some(2_032),
            )
            .expect("resumed");
        assert_eq!(resumed.session.view_mode, TabViewMode::Terminal);
        assert_eq!(
            resumed.session.provider_handle_chain,
            vec!["codex-h1", "codex-h2"]
        );
        assert_eq!(
            resumed.session.parent_session_id.as_deref(),
            Some("sess-cli")
        );

        // Fork clones non-tombstoned items and chain.
        let forked = db
            .fork_session("sess-cli", None, Some("sess-fork".into()), Some(2_040))
            .expect("fork");
        assert_eq!(forked.chat_state.last_sequence, 1);
        assert_eq!(db.list_items("sess-fork", false).expect("fork items").len(), 1);

        // Deterministic event replay.
        let events = db.replay_events_since("sess-cli", 0, None).expect("replay");
        assert!(!events.is_empty());
        assert!(events.windows(2).all(|w| w[0].sequence <= w[1].sequence));
    }
}
